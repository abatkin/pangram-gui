# Pangram desktop

A lightweight Linux desktop client for [Pangram](https://www.pangram.com/) AI-text detection,
built with Rust, Qt 6 Quick Controls/QML and [CXX-Qt](https://kdab.github.io/cxx-qt/book/).

> **Unofficial.** This is an independent, community-built frontend for Pangram's public API. It
> isn't affiliated with, endorsed by or supported by Pangram Labs. You need your own Pangram API
> key, and Pangram bills your account for the scans you run. Pangram's name is used only to say
> which service the app talks to.

Features:

- Paste or edit plain text and analyze it only when you ask (Check for AI, or Ctrl+Enter).
- Results: highlighted, selectable analyzed text; AI / AI-assisted / human proportions with a
  legend; per-section label, confidence, scores and model-specific details.
- Copy the analyzed text, any selection, or a plain-text result summary.
- See the exact request and responses for any scan (Raw data), with the API key hidden.
- Cost estimates: a per-draft estimate, the billed words/credits/cost of each scan, and a
  **Usage and cost** window that totals estimated spend by day, week or month.
- Local scan history: search, reopen without new API calls, “Edit and rescan”, delete one or all.
- API key in the system keyring (Secret Service), or for the session only.
- Polling resumes after a restart. A submission whose outcome is uncertain is never resent
  automatically.

## Requirements (Fedora 44)

Build:

```sh
sudo dnf install gcc-c++ pkgconf-pkg-config openssl-devel \
    qt6-qtbase-devel qt6-qtdeclarative-devel qt6-qtwayland
```

The Rust toolchain is pinned to 1.98.1 in `rust-toolchain.toml`; rustup installs it automatically.
Other pinned versions: CXX-Qt 0.10.0, tested against Qt 6.11.2.

Runtime:

- Qt 6 base, declarative (Quick/QML/Controls) and Wayland libraries, plus OpenSSL. These are
  installed with the packages above.
- Optional: `kf6-qqc2-desktop-style` for native KDE styling. Without it the app uses Fusion.
- A Secret Service provider to remember the API key: KWallet's `ksecretd` on Plasma, or GNOME
  Keyring. Without one, the key is kept for the session only. It is never written to disk.

## Prebuilt binaries

[GitHub Releases](../../releases) has Linux x86_64 builds, made on Fedora 44 against Qt 6.11.
They use the system's Qt (6.11 or newer) and OpenSSL 3, so install the runtime packages above
first. Unpack into `~/.local` (which must have `~/.local/bin` on your `PATH`):

```sh
sha256sum -c pangram-desktop-*.tar.gz.sha256
tar -xzf pangram-desktop-*.tar.gz -C ~/.local --strip-components=1
```

To remove it, delete `~/.local/bin/pangram-desktop` and the `net.batkin.pangram-desktop` desktop
entry and icon under `~/.local/share`.

## Build, run, install

```sh
cargo run -p pangram-desktop              # debug build
scripts/install-local.sh                  # release build into ~/.local (binary, desktop entry, icon)
scripts/install-local.sh --uninstall
```

After installing, start **Pangram** from the application menu. On first launch, Settings opens
so you can paste your API key.

The app runs as a native Wayland client. Its Wayland app ID, `net.batkin.pangram-desktop`, matches
the desktop entry. Run with `QT_QPA_PLATFORM=xcb` to force X11.

## Using it

| Shortcut | Action |
| --- | --- |
| Ctrl+Enter | Check the draft for AI |
| Ctrl+N | New check (asks before discarding an unsubmitted draft) |
| Ctrl+F | Find in the document (Enter / Shift+Enter: next / previous, Esc: close) |
| F9 | Show or hide the history sidebar |
| Ctrl+, | Settings |
| Tab | Move between panes; the editors don't insert tab characters |
| Up / Down in Sections | Select a section and highlight it in the text |

Click anywhere in the analyzed text to inspect its section. The percentages are the share of the
text in each category. They are not the probability that the whole document is AI-written.

Pangram may normalize the submitted text (quotes, for example). The app stores the submitted
text and the returned text separately, and highlights the returned text. If the returned ranges
don't match the returned text, highlights are withheld and each section's text is shown in the
results pane instead.

When the window is narrower than about 1050 px, results move below the document.

### Costs

The API doesn't report charges, so the app estimates them from Pangram's published pricing
(checked 2026-10-09): Pangram 4 bills one credit per 100 words, rounded up per scan, and API
credits cost $0.05 (the batch API, not used here, is 20% cheaper). Billed words come from
Pangram's own per-section `word_count`, which runs a little higher than a simple word count. Each
completed scan records its words, credits and cost at the price set in Settings at the time.
Usage records hold no text and are kept when scans are deleted (Usage and cost → Clear usage
records removes them). Your Pangram dashboard is the authority on what you were charged.

## Data and privacy

| What | Where |
| --- | --- |
| Settings | `$XDG_CONFIG_HOME/pangram-desktop/settings.json` (`~/.config/…`) |
| History and usage | `$XDG_DATA_HOME/pangram-desktop/history.sqlite3` (`~/.local/share/…`), a SQLite database (`sqlite3` can open it). Settings shows the path. |
| API key | Secret Service item `application=net.batkin.pangram-desktop, kind=pangram-api-key` |

- Scans are sent with `public_dashboard_link: false`.
- History stores the submitted text, the returned text, model, version, task ID, state and the
  raw response. Turn off **Save scans to local history** to keep new scans in memory only.
- Deleting local history doesn't delete Pangram's copies.
- The app logs no keys, document text or response bodies.
- Stopping polling, or quitting, doesn't cancel a scan on Pangram's side. Pangram may still bill it.

## Development

```text
crates/pangram-core      Qt-independent: api (client), analysis (ranges/highlights), storage
                         (SQLite), settings, credentials, service (orchestration)
crates/pangram-desktop   CXX-Qt adapter (src/backend.rs), main.rs, QML views (qml/)
data/                    Desktop entry and icon
scripts/                 install-local.sh, ui-smoke.sh, package.sh (release tarball)
.github/                 CI and Release workflows; actions/setup installs the build dependencies
```

Core events are turned into UI payloads on worker threads, then queued to the Qt thread. The QML
side only binds properties and calls commands.

Tests never spend API credits:

```sh
cargo test --workspace                     # unit tests + mock-server integration tests
QT_QPA_PLATFORM=offscreen scripts/ui-smoke.sh [screenshot-dir]   # end-to-end UI smoke test
scripts/ui-smoke.sh                        # same, as a native Wayland window
```

The UI smoke test starts a local mock API (`cargo run -p pangram-core --example mock_server`). It
uses throwaway settings and history, plus an in-memory key store
(`PANGRAM_CREDENTIAL_STORE=memory`). It drives the real window, including keyboard input:

- draft → submit → poll → result
- range/rendering consistency with emoji, CJK, tabs and HTML-like text
- section selection and find
- copying across highlights
- edit and rescan, failure display, reopening history, deletion
- a 100k-character document

### CI and releases

GitHub Actions builds in a `fedora:44` container, since Ubuntu runners don't ship Qt 6.11. On every
push to `main` and every pull request, [CI](.github/workflows/ci.yml) runs `cargo fmt --check`,
Clippy with `-D warnings`, the tests and the UI smoke test (offscreen). It uploads screenshots if
the smoke test fails. The opt-in tests below never run in CI.

[Release](.github/workflows/release.yml) is run by hand from the Actions tab, and only from
`main`. It releases the `[workspace.package]` version in `Cargo.toml` as tag `v<version>`, so bump
and commit that first; it stops if the tag already exists. It runs CI and builds
`scripts/package.sh`'s tarball and checksum. Then it creates the tag and a GitHub release with
generated notes. Versions with a suffix (`0.2.0-rc.1`) are marked as prereleases, and a "draft"
input holds the release back for review.

To run the app against the mock yourself:

```sh
cargo run -p pangram-core --example mock_server        # in one terminal
PANGRAM_API_BASE=http://127.0.0.1:8765 PANGRAM_CREDENTIAL_STORE=memory cargo run -p pangram-desktop
```

Opt-in checks against real services (see `crates/pangram-core/tests/opt_in.rs`). The live ones
use `PANGRAM_API_KEY` or, if unset, the key the app saved in the keyring:

```sh
cargo test -p pangram-core --test opt_in secret_service -- --ignored    # keyring (test item only)
PANGRAM_LIVE_TEST=1 cargo test -p pangram-core --test opt_in live_api -- --ignored --nocapture   # ONE real scan
cargo run -p pangram-core --example live_probe -- models                # free: list models + headers
cargo run -p pangram-core --example live_probe -- scan FILE [MODEL]     # ONE real scan, prints headers
```

### Implementation notes

- **Highlight index units.** The API documentation doesn't say what unit the offsets use. A live
  probe (emoji, combining accents, CJK, math letters, CRLF) showed **Unicode code points**. The
  client still accepts code points, UTF-16 or UTF-8 bytes, whichever reproduces every window's
  text, and otherwise withholds highlights.
- **Normalization.** Pangram removes carriage returns, collapses blank lines and strips
  indentation at the start of lines; other characters (including double spaces, emoji and
  curly quotes) came back unchanged. The results pane says what changed.
- **Undocumented fields.** `POST /task` can return a `notice` (on 2026-10-09: "As of October 1st,
  2026, the default model selection is Pangram 4.0…"); the app shows it once per session and
  keeps the raw response. Polling also reports `STAGE_POSTPROCESSING`. No headers carry credit
  or rate-limit information.
- **Input limits.** These aren't documented for the API, so the app doesn't enforce any. Pangram's
  400/413/422 errors are shown as returned.
- **Linker.** `qt-build-utils` picks GNU gold when the system `ld` is bfd, and gold is deprecated.
  `.cargo/config.toml` requests lld, which is Rust's bundled default linker for this target.
- **Measurements** (release build, Plasma Wayland): 15 MB binary. About 190 MB RSS / 85 MB PSS at
  idle with the KDE style; most of that is shared Qt/KDE libraries. A 100k-character result with
  943 sections renders its highlights in about 8 ms.
