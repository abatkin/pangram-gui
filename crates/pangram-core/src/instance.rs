//! One running app per data directory.
//!
//! The first instance holds a lock and listens on a Unix socket. A later launch finds the lock
//! taken, asks the running instance to show its window (passing on its Wayland activation token,
//! so the compositor lets that window take focus) and exits. The lock is an `flock`, released by
//! the kernel when its holder exits, so a crash never leaves a stale lock; a stale socket file is
//! replaced by the next instance.

use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::unix::fs::DirBuilderExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::settings::APP_DIR;

const ACTIVATE: &str = "activate";
const ACK: &str = "ok";
/// Activation tokens are short opaque strings; longer requests are not ours.
const MAX_REQUEST: u64 = 1024;
/// How long a second launch waits for the running instance to listen and answer.
const RESPONSE_TIMEOUT: Duration = Duration::from_secs(3);

/// Base path (without extension) of the lock and socket for a data directory. They live in
/// `$XDG_RUNTIME_DIR/pangram-desktop/`, named by a hash of the data directory, or in the data
/// directory itself when there is no runtime directory.
pub fn location(data_dir: &Path) -> PathBuf {
    match std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
    {
        Some(runtime) => runtime.join(APP_DIR).join(format!(
            "{:016x}",
            fnv1a(data_dir.as_os_str().as_encoded_bytes())
        )),
        None => data_dir.join("instance"),
    }
}

/// Stable across builds, unlike `DefaultHasher`, so different versions agree on the path.
fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3)
    })
}

pub enum Acquired {
    /// No other instance runs. Keep this alive for the life of the process and call `listen`.
    Primary(Primary),
    /// The running instance was asked to show itself; this process should exit.
    Forwarded,
    /// Another instance holds the lock but didn't answer.
    NoResponse(io::Error),
}

pub struct Primary {
    // Held, never read: dropping it releases the lock.
    _lock: File,
    listener: UnixListener,
}

/// Becomes the primary instance, or hands `token` to the running one. Errors mean the check
/// itself couldn't be made (for example, an unwritable directory).
pub fn acquire(base: &Path, token: Option<&str>) -> io::Result<Acquired> {
    acquire_with(base, token, RESPONSE_TIMEOUT)
}

fn acquire_with(base: &Path, token: Option<&str>, timeout: Duration) -> io::Result<Acquired> {
    if let Some(dir) = base.parent() {
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)?;
    }
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(base.with_extension("lock"))?;
    let socket = base.with_extension("sock");
    match lock.try_lock() {
        Ok(()) => {
            // Left behind by an instance that exited; nothing else can be listening on it.
            match fs::remove_file(&socket) {
                Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
                _ => {}
            }
            let listener = UnixListener::bind(&socket)?;
            Ok(Acquired::Primary(Primary {
                _lock: lock,
                listener,
            }))
        }
        Err(fs::TryLockError::WouldBlock) => Ok(match forward(&socket, token, timeout) {
            Ok(()) => Acquired::Forwarded,
            Err(e) => Acquired::NoResponse(e),
        }),
        Err(fs::TryLockError::Error(e)) => Err(e),
    }
}

fn forward(socket: &Path, token: Option<&str>, timeout: Duration) -> io::Result<()> {
    let deadline = Instant::now() + timeout;
    // The running instance may still be starting: it binds the socket just after locking.
    let mut stream = loop {
        match UnixStream::connect(socket) {
            Ok(s) => break s,
            Err(e) if Instant::now() >= deadline => return Err(e),
            Err(_) => std::thread::sleep(Duration::from_millis(100)),
        }
    };
    let remaining = deadline
        .saturating_duration_since(Instant::now())
        .max(Duration::from_millis(500));
    stream.set_read_timeout(Some(remaining))?;
    stream.set_write_timeout(Some(remaining))?;
    let request = match token.filter(|t| valid_token(t)) {
        Some(t) => format!("{ACTIVATE} {t}\n"),
        None => format!("{ACTIVATE}\n"),
    };
    stream.write_all(request.as_bytes())?;
    let mut reply = String::new();
    BufReader::new(stream)
        .take(MAX_REQUEST)
        .read_line(&mut reply)?;
    if reply.trim_end() == ACK {
        Ok(())
    } else {
        Err(io::Error::other(
            "unexpected reply from the running instance",
        ))
    }
}

fn valid_token(t: &str) -> bool {
    !t.is_empty() && t.len() < 512 && t.bytes().all(|b| b.is_ascii_graphic())
}

/// Parses one request line: `Some(token)` for an activation request.
fn parse(line: &str) -> Option<Option<String>> {
    let line = line.strip_suffix('\n')?;
    let rest = line.strip_prefix(ACTIVATE)?;
    match rest.strip_prefix(' ') {
        Some(t) if valid_token(t) => Some(Some(t.to_owned())),
        Some(_) => Some(None),
        None if rest.is_empty() => Some(None),
        None => None,
    }
}

impl Primary {
    /// Serves activation requests on a background thread for the rest of the process. Calls
    /// `on_activate` with the requester's activation token, if it had one.
    pub fn listen(self, on_activate: impl Fn(Option<String>) + Send + 'static) {
        let spawned = std::thread::Builder::new()
            .name("pangram-instance".into())
            .spawn(move || {
                let _lock = self._lock;
                for stream in self.listener.incoming() {
                    let Ok(stream) = stream else { continue };
                    if let Some(token) = serve(stream) {
                        on_activate(token);
                    }
                }
            });
        // If the thread can't start, the lock is released with it and a later launch runs as
        // its own instance: the behaviour without single-instance support.
        drop(spawned);
    }
}

fn serve(mut stream: UnixStream) -> Option<Option<String>> {
    stream.set_read_timeout(Some(RESPONSE_TIMEOUT)).ok()?;
    stream.set_write_timeout(Some(RESPONSE_TIMEOUT)).ok()?;
    let mut line = String::new();
    BufReader::new(&stream)
        .take(MAX_REQUEST)
        .read_line(&mut line)
        .ok()?;
    let token = parse(&line)?;
    stream.write_all(format!("{ACK}\n").as_bytes()).ok()?;
    Some(token)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    fn primary(base: &Path) -> Primary {
        match acquire(base, None).unwrap() {
            Acquired::Primary(p) => p,
            _ => panic!("expected to become the primary instance"),
        }
    }

    #[test]
    fn second_launch_activates_the_first() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("sub/inst");
        let (tx, rx) = mpsc::channel();
        primary(&base).listen(move |token| tx.send(token).unwrap());

        assert!(matches!(
            acquire(&base, Some("tok-123_ABC")).unwrap(),
            Acquired::Forwarded
        ));
        assert_eq!(rx.recv().unwrap().as_deref(), Some("tok-123_ABC"));
        // A missing or malformed token still activates, without one.
        assert!(matches!(acquire(&base, None).unwrap(), Acquired::Forwarded));
        assert_eq!(rx.recv().unwrap(), None);
        assert!(matches!(
            acquire(&base, Some("bad token\nactivate x")).unwrap(),
            Acquired::Forwarded
        ));
        assert_eq!(rx.recv().unwrap(), None);
    }

    #[test]
    fn stale_socket_is_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("inst");
        drop(primary(&base));
        assert!(base.with_extension("sock").exists());
        let (tx, rx) = mpsc::channel();
        primary(&base).listen(move |token| tx.send(token).unwrap());
        assert!(matches!(acquire(&base, None).unwrap(), Acquired::Forwarded));
        assert_eq!(rx.recv().unwrap(), None);
    }

    #[test]
    fn silent_holder_is_reported() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("inst");
        // Holds the lock but never accepts.
        let _held = primary(&base);
        let result = acquire_with(&base, None, Duration::from_millis(300)).unwrap();
        assert!(matches!(result, Acquired::NoResponse(_)));
    }

    #[test]
    fn parses_requests() {
        assert_eq!(parse("activate\n"), Some(None));
        assert_eq!(parse("activate abc\n"), Some(Some("abc".into())));
        assert_eq!(parse("activate a b\n"), Some(None));
        assert_eq!(parse("activated\n"), None);
        assert_eq!(parse("activate"), None);
        assert_eq!(parse("hello\n"), None);
    }

    #[test]
    fn location_depends_on_the_data_dir() {
        assert_ne!(
            location(Path::new("/a/pangram-desktop")),
            location(Path::new("/b/pangram-desktop"))
        );
        assert_eq!(fnv1a(b"a"), 0xaf63_dc4c_8601_ec8c);
    }
}
