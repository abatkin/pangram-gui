//! Scan orchestration: credentials, model catalog, submission, polling, recovery and history.
//!
//! All network and storage work runs on a Tokio runtime. The UI receives [`Event`]s through a
//! sink callback (called from runtime threads) and issues commands that return immediately.
//!
//! Invariants:
//! * Only one scan may be submitting or polling at a time.
//! * A POST whose outcome is unknown is never resubmitted automatically.
//! * Every record write carries the history generation current when its operation began, and is
//!   checked under the repository lock; deleting all history bumps the generation under the same
//!   lock, so no earlier operation can write a deleted scan back.
//! * Each poll loop owns a generation number; once its entry in `pollers` is removed or replaced
//!   (stop, delete, resume) any response it was waiting for is discarded unseen.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tokio::runtime::Handle;
use tokio::sync::watch;

use crate::api::{
    ApiError, ApiKey, ClientOptions, DetectionResult, PangramClient, SubmitError, TaskPoll,
    choose_model,
};
use crate::cost;
use crate::credentials::CredentialStore;
use crate::settings::{Paths, Settings};
use crate::storage::{
    Db, Repository, SaveOutcome, ScanRecord, ScanState, ScanSummary, StorageError, UsagePeriod,
    UsageRecord, UsageRow,
};

const HISTORY_LIMIT: usize = 500;

const UNKNOWN_AFTER_RESTART: &str = "The app closed before Pangram confirmed this submission, so it \
    may or may not have been received. It was not resubmitted automatically, to avoid a possible \
    duplicate charge. Use Edit and rescan to submit it again.";
const NOT_RESUBMITTED: &str = "It was not resubmitted automatically, to avoid a possible duplicate \
    charge. Use Edit and rescan to submit it again.";
const STOPPED: &str = "Polling stopped. Pangram may still finish (and bill) this scan; resume to \
    fetch the result.";

#[derive(Debug, Clone)]
pub struct PollPolicy {
    pub first_delay: Duration,
    pub max_interval: Duration,
    pub error_base: Duration,
    pub max_error_delay: Duration,
    /// Consecutive failed GETs before polling pauses.
    pub max_consecutive_errors: u32,
}

impl Default for PollPolicy {
    fn default() -> Self {
        Self {
            first_delay: Duration::from_millis(1000),
            max_interval: Duration::from_secs(5),
            error_base: Duration::from_secs(2),
            max_error_delay: Duration::from_secs(60),
            max_consecutive_errors: 8,
        }
    }
}

impl PollPolicy {
    fn error_delay(&self, consecutive_errors: u32, server_hint: Option<Duration>) -> Duration {
        let exp = self
            .error_base
            .saturating_mul(1u32 << consecutive_errors.saturating_sub(1).min(10));
        // The server's Retry-After is never shortened; only local backoff is capped.
        server_hint.unwrap_or(exp.min(self.max_error_delay))
    }
}

pub struct Config {
    pub paths: Paths,
    pub client: ClientOptions,
    pub poll: PollPolicy,
    pub credentials: CredentialStore,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoticeLevel {
    Info,
    Warning,
    Error,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CredentialStatus {
    pub has_key: bool,
    /// Stored in the Secret Service (as opposed to session-only).
    pub persisted: bool,
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ModelStatus {
    pub models: Vec<String>,
    pub selected: Option<String>,
    pub loading: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct HistoryStatus {
    pub items: Vec<ScanSummary>,
    pub saved_count: usize,
    pub disk_available: bool,
}

#[derive(Debug, Clone)]
pub enum Event {
    Ready,
    Settings(Settings),
    Credentials(CredentialStatus),
    Models(ModelStatus),
    History(HistoryStatus),
    /// A scan's stored record changed.
    Scan(ScanRecord),
    /// Transient progress for a polling scan (not persisted).
    Progress {
        id: String,
        message: String,
    },
    /// Response to [`Service::open`].
    Opened(ScanRecord),
    /// `None` means all scans were deleted.
    Deleted {
        id: Option<String>,
    },
    Busy(bool),
    Notice {
        level: NoticeLevel,
        message: String,
    },
    /// Response to [`Service::usage_summary`], refreshed whenever usage changes.
    Usage {
        period: String,
        rows: Vec<UsageRow>,
    },
}

#[derive(Default)]
struct State {
    settings: Settings,
    key: Option<ApiKey>,
    key_persisted: bool,
    key_note: Option<String>,
    client: Option<Arc<PangramClient>>,
    models: Vec<String>,
    selected_model: Option<String>,
    models_loading: bool,
    models_error: Option<String>,
    model_generation: u64,
    pollers: HashMap<String, u64>,
    submitting: Option<String>,
    next_generation: u64,
    history_query: String,
    deleted: HashSet<String>,
    usage_period: Option<UsagePeriod>,
    /// Incremented when all history is deleted; see the module docs.
    history_generation: u64,
    /// Server notices already shown this session.
    shown_notices: HashSet<String>,
}

impl State {
    fn busy(&self) -> bool {
        self.submitting.is_some() || !self.pollers.is_empty()
    }

    fn register_poller(&mut self, id: &str) -> u64 {
        self.next_generation += 1;
        self.pollers.insert(id.to_owned(), self.next_generation);
        self.next_generation
    }

    fn credential_status(&self) -> CredentialStatus {
        CredentialStatus {
            has_key: self.key.is_some(),
            persisted: self.key_persisted,
            note: self.key_note.clone(),
        }
    }

    fn model_status(&self) -> ModelStatus {
        ModelStatus {
            models: self.models.clone(),
            selected: self.selected_model.clone(),
            loading: self.models_loading,
            error: self.models_error.clone(),
        }
    }
}

type Sink = Box<dyn Fn(Event) + Send + Sync>;

struct Inner {
    config: Config,
    rt: Handle,
    sink: Sink,
    state: Mutex<State>,
    repo: Mutex<Option<Repository>>,
    ready: watch::Sender<bool>,
    /// Serializes settings snapshots and file writes.
    settings_write: tokio::sync::Mutex<()>,
}

impl Inner {
    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Whether a write for `id`, begun in `history_generation`, may still be applied. Call with
    /// the repository lock held so deletions can't interleave.
    fn may_write(&self, id: &str, history_generation: u64) -> bool {
        let st = self.state();
        !st.deleted.contains(id) && st.history_generation == history_generation
    }
}

#[derive(Clone)]
pub struct Service {
    inner: Arc<Inner>,
}

pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Uses the given title, or the first non-empty line of the text.
pub fn derive_title(title: &str, text: &str) -> String {
    let title = title.trim();
    let source = if title.is_empty() {
        text.lines()
            .map(str::trim)
            .find(|l| !l.is_empty())
            .unwrap_or("Untitled")
    } else {
        title
    };
    const MAX: usize = 80;
    if source.chars().count() > MAX {
        format!(
            "{}…",
            source.chars().take(MAX).collect::<String>().trim_end()
        )
    } else {
        source.to_owned()
    }
}

impl Service {
    /// Starts the service; initialisation (settings, history, keyring, recovery) runs in the
    /// background and ends with [`Event::Ready`].
    pub fn start(config: Config, rt: Handle, sink: impl Fn(Event) + Send + Sync + 'static) -> Self {
        let service = Self {
            inner: Arc::new(Inner {
                config,
                rt,
                sink: Box::new(sink),
                state: Mutex::new(State::default()),
                repo: Mutex::new(None),
                ready: watch::channel(false).0,
                settings_write: tokio::sync::Mutex::new(()),
            }),
        };
        let s = service.clone();
        service.spawn(async move { s.init().await });
        service
    }

    fn spawn(&self, fut: impl std::future::Future<Output = ()> + Send + 'static) {
        self.inner.rt.spawn(fut);
    }

    fn emit(&self, event: Event) {
        (self.inner.sink)(event);
    }

    fn notice(&self, level: NoticeLevel, message: impl Into<String>) {
        self.emit(Event::Notice {
            level,
            message: message.into(),
        });
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.inner.state()
    }

    fn emit_busy(&self) {
        let busy = self.state().busy();
        self.emit(Event::Busy(busy));
    }

    async fn wait_ready(&self) {
        let mut rx = self.inner.ready.subscribe();
        let _ = rx.wait_for(|ready| *ready).await;
    }

    /// Runs blocking repository work off the async threads.
    async fn with_repo<T: Send + 'static>(
        &self,
        f: impl FnOnce(&mut Repository) -> T + Send + 'static,
    ) -> Option<T> {
        self.wait_ready().await;
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || {
            let mut guard = inner.repo.lock().unwrap_or_else(|e| e.into_inner());
            guard.as_mut().map(f)
        })
        .await
        .ok()
        .flatten()
    }

    /// Persists the current settings. Callers change `state.settings` synchronously first; the
    /// snapshot is taken under the write lock, so a later call always writes newer settings.
    async fn save_settings(&self) {
        let _write = self.inner.settings_write.lock().await;
        let settings = self.state().settings.clone();
        let path = self.inner.config.paths.settings_file();
        let to_write = settings.clone();
        let saved = tokio::task::spawn_blocking(move || to_write.save(&path)).await;
        if let Ok(Err(e)) | Err(e) = saved.map_err(std::io::Error::other) {
            self.notice(NoticeLevel::Warning, format!("Couldn't save settings: {e}"));
        }
        self.emit(Event::Settings(settings));
    }

    async fn init(self) {
        let paths = self.inner.config.paths.clone();
        let (settings, disk) = tokio::task::spawn_blocking(move || {
            (
                Settings::load(&paths.settings_file()),
                Db::open(&paths.history_db()),
            )
        })
        .await
        .expect("settings/history initialisation panicked");

        let (disk, disk_error) = match disk {
            Ok(db) => (Some(db), None),
            Err(e) => (None, Some(e)),
        };
        let repo = Repository::new(disk, settings.save_history)
            .expect("in-memory SQLite database must open");
        *self.inner.repo.lock().unwrap() = Some(repo);
        let remember = settings.remember_key;
        self.state().settings = settings.clone();
        self.emit(Event::Settings(settings));
        if let Some(e) = disk_error {
            self.notice(
                NoticeLevel::Error,
                format!(
                    "Couldn't open scan history ({e}). Scans will be kept for this session only."
                ),
            );
        }
        self.inner.ready.send_replace(true);
        self.refresh_history().await;

        if remember {
            match self.inner.config.credentials.load().await {
                Ok(Some(key)) => {
                    self.install_key(key, true, None);
                }
                Ok(None) => {}
                Err(e) => {
                    self.state().key_note = Some(format!(
                        "The system keyring is unavailable ({}). A key entered now is kept for this session only.",
                        e.0
                    ));
                }
            }
        }
        let status = self.state().credential_status();
        self.emit(Event::Credentials(status));
        self.refresh_models();
        self.recover().await;
        self.emit(Event::Ready);
    }

    fn install_key(&self, key: ApiKey, persisted: bool, note: Option<String>) {
        let client = PangramClient::new(key.clone(), self.inner.config.client.clone());
        let mut st = self.state();
        match client {
            Ok(c) => {
                st.key = Some(key);
                st.client = Some(Arc::new(c));
                st.key_persisted = persisted;
                st.key_note = note;
            }
            Err(e) => {
                st.key = None;
                st.client = None;
                st.key_persisted = false;
                st.key_note = Some(e.to_string());
            }
        }
        // A new key may see a different catalog.
        st.models.clear();
        st.selected_model = None;
        st.models_error = None;
    }

    async fn recover(&self) {
        let history_generation = self.state().history_generation;
        let Some(Ok(unfinished)) = self.with_repo(|r| r.unfinished()).await else {
            return;
        };
        for mut rec in unfinished {
            match (rec.state, rec.task_id.clone()) {
                (ScanState::Polling, Some(task_id)) => {
                    let generation_id = self.state().register_poller(&rec.id);
                    let s = self.clone();
                    self.spawn(async move { s.poll_loop(rec, task_id, generation_id).await });
                }
                _ => {
                    rec.state = ScanState::SubmissionUnknown;
                    rec.error = Some(UNKNOWN_AFTER_RESTART.to_owned());
                    rec.updated_at = now_ms();
                    self.save(&rec, history_generation).await;
                    self.emit(Event::Scan(rec));
                }
            }
        }
        self.emit_busy();
        self.refresh_history().await;
    }

    // ----- credentials and models -------------------------------------------------------------

    pub fn set_api_key(&self, key: &str, remember: bool) {
        let Some(key) = ApiKey::new(key) else {
            self.notice(NoticeLevel::Warning, "Enter an API key.");
            return;
        };
        self.state().settings.remember_key = remember;
        let s = self.clone();
        self.spawn(async move {
            s.save_settings().await;
            let store = &s.inner.config.credentials;
            let (persisted, note) = if remember {
                match store.save(&key).await {
                    Ok(()) => (true, None),
                    Err(e) => (
                        false,
                        Some(format!(
                            "Couldn't store the key in the system keyring ({}). It is kept for this session only.",
                            e.0
                        )),
                    ),
                }
            } else {
                // Don't leave a previously remembered key behind; say so if that fails.
                let note = store.delete().await.err().map(|e| {
                    format!(
                        "Couldn't remove the previously remembered key from the system keyring ({}). It may still be stored there.",
                        e.0
                    )
                });
                (false, note)
            };
            s.install_key(key, persisted, note);
            let status = s.state().credential_status();
            s.emit(Event::Credentials(status));
            s.refresh_models();
        });
    }

    pub fn forget_api_key(&self) {
        let s = self.clone();
        self.spawn(async move {
            let deleted = s.inner.config.credentials.delete().await;
            {
                let mut st = s.state();
                st.key = None;
                st.client = None;
                st.key_persisted = false;
                st.key_note = deleted
                    .err()
                    .map(|e| format!("Couldn't remove the key from the system keyring ({}).", e.0));
                st.models.clear();
                st.selected_model = None;
                st.model_generation += 1;
                st.models_loading = false;
                st.models_error = None;
            }
            let (creds, models) = {
                let st = s.state();
                (st.credential_status(), st.model_status())
            };
            s.emit(Event::Credentials(creds));
            s.emit(Event::Models(models));
        });
    }

    pub fn refresh_models(&self) {
        let (client, generation, status) = {
            let mut st = self.state();
            st.model_generation += 1;
            st.models_loading = st.client.is_some();
            st.models_error = None;
            (st.client.clone(), st.model_generation, st.model_status())
        };
        self.emit(Event::Models(status));
        let Some(client) = client else { return };
        let s = self.clone();
        self.spawn(async move {
            let result = client.list_models().await;
            let status = {
                let mut st = s.state();
                if st.model_generation != generation {
                    return; // superseded by a newer refresh or key change
                }
                st.models_loading = false;
                match result {
                    Ok(models) => {
                        st.selected_model = choose_model(&models, st.settings.model.as_deref());
                        st.models_error = models
                            .is_empty()
                            .then(|| "No models are available for this API key.".to_owned());
                        st.models = models;
                    }
                    Err(e) => st.models_error = Some(format!("Couldn't load models: {e}")),
                }
                st.model_status()
            };
            s.emit(Event::Models(status));
        });
    }

    pub fn select_model(&self, model: &str) {
        let status = {
            let mut st = self.state();
            if !st.models.iter().any(|m| m == model) {
                return;
            }
            st.selected_model = Some(model.to_owned());
            st.settings.model = Some(model.to_owned());
            st.model_status()
        };
        self.emit(Event::Models(status));
        let s = self.clone();
        self.spawn(async move { s.save_settings().await });
    }

    // ----- scans ------------------------------------------------------------------------------

    /// Snapshots the text and starts a scan, returning its local ID.
    pub fn analyze(
        &self,
        title: &str,
        text: &str,
        source_scan_id: Option<String>,
    ) -> Result<String, String> {
        if text.trim().is_empty() {
            return Err("Enter some text to analyze.".to_owned());
        }
        let (client, model, id, history_generation, usd_per_credit) = {
            let mut st = self.state();
            let client = st
                .client
                .clone()
                .ok_or("Add your Pangram API key in Settings first.")?;
            let model = st.selected_model.clone().ok_or(if st.models_loading {
                "The model list is still loading."
            } else {
                "No model is available. Refresh the model list in Settings."
            })?;
            if st.busy() {
                return Err("Another scan is still in progress.".to_owned());
            }
            let id = uuid::Uuid::new_v4().to_string();
            st.submitting = Some(id.clone());
            (
                client,
                model,
                id,
                st.history_generation,
                st.settings.usd_per_credit,
            )
        };
        let now = now_ms();
        let rec = ScanRecord {
            id: id.clone(),
            created_at: now,
            updated_at: now,
            title: derive_title(title, text),
            input_text: text.to_owned(),
            requested_model: model,
            task_id: None,
            state: ScanState::Submitting,
            error: None,
            returned_text: None,
            returned_version: None,
            prediction_short: None,
            fraction_ai: None,
            raw_response: None,
            source_scan_id,
            submit_response: None,
            billed_words: None,
            credits: None,
            cost_usd: None,
            usd_per_credit: Some(usd_per_credit),
        };
        self.emit(Event::Busy(true));
        self.emit(Event::Scan(rec.clone()));
        let s = self.clone();
        self.spawn(async move { s.submit_flow(rec, client, history_generation).await });
        Ok(id)
    }

    async fn submit_flow(
        self,
        mut rec: ScanRecord,
        client: Arc<PangramClient>,
        history_generation: u64,
    ) {
        // Persist the snapshot before anything is sent.
        self.save(&rec, history_generation).await;
        self.refresh_history().await;

        let result = client.submit(&rec.input_text, &rec.requested_model).await;
        rec.updated_at = now_ms();
        let mut poll = None;
        {
            let mut st = self.state();
            st.submitting = None;
            match &result {
                Ok(submission) => {
                    let task_id = &submission.task_id;
                    rec.task_id = Some(task_id.clone());
                    rec.submit_response = Some(submission.raw.clone());
                    rec.state = ScanState::Polling;
                    if !st.deleted.contains(&rec.id) && st.history_generation == history_generation
                    {
                        poll = Some((task_id.clone(), st.register_poller(&rec.id)));
                    }
                }
                Err(SubmitError::Rejected(e)) => {
                    rec.state = ScanState::Failed;
                    rec.error = Some(e.to_string());
                }
                Err(SubmitError::OutcomeUnknown(msg)) => {
                    rec.state = ScanState::SubmissionUnknown;
                    rec.error = Some(format!(
                        "Pangram may or may not have received this scan ({msg}). {NOT_RESUBMITTED}"
                    ));
                }
            }
        }
        // Persist the task ID immediately so a restart can resume polling.
        self.save(&rec, history_generation).await;
        self.emit(Event::Scan(rec.clone()));
        if let Ok(Some(notice)) = result.as_ref().map(|s| s.notice.clone()) {
            // e.g. announcements about default model changes; show each once per session.
            if self.state().shown_notices.insert(notice.clone()) {
                self.notice(NoticeLevel::Info, format!("Pangram: {notice}"));
            }
        }
        if let Err(SubmitError::Rejected(e)) = &result {
            self.notice(NoticeLevel::Error, e.to_string());
            if e.suggests_model_refresh() {
                self.refresh_models();
            }
        }
        if let Some((task_id, generation_id)) = poll {
            let s = self.clone();
            self.spawn(async move { s.poll_loop(rec, task_id, generation_id).await });
        }
        self.emit_busy();
        self.refresh_history().await;
    }

    fn is_current(&self, id: &str, generation_id: u64) -> bool {
        self.state().pollers.get(id) == Some(&generation_id)
    }

    /// Removes the poller if it is still `generation_id`; the caller then owns the record's next
    /// write, made with the returned history generation.
    fn claim(&self, id: &str, generation_id: u64) -> Option<u64> {
        let mut st = self.state();
        if st.pollers.get(id) == Some(&generation_id) {
            st.pollers.remove(id);
            Some(st.history_generation)
        } else {
            None
        }
    }

    /// Applies the final update unless the scan was stopped, resumed or deleted meanwhile.
    /// Returns the stored record if this call won.
    async fn finish(
        &self,
        mut rec: ScanRecord,
        generation_id: u64,
        update: impl FnOnce(&mut ScanRecord),
    ) -> Option<ScanRecord> {
        let history_generation = self.claim(&rec.id, generation_id)?;
        update(&mut rec);
        rec.updated_at = now_ms();
        // A completed scan and its charge are stored together.
        let usage = usage_for(&rec);
        match usage.clone() {
            Some(usage) => self.save_completed(&rec, usage, history_generation).await,
            None => self.save(&rec, history_generation).await,
        }
        self.emit(Event::Scan(rec.clone()));
        self.emit_busy();
        self.refresh_history().await;
        if usage.is_some() {
            self.refresh_usage().await;
        }
        Some(rec)
    }

    async fn pause(&self, rec: ScanRecord, generation_id: u64, message: String) {
        self.finish(rec, generation_id, |r| {
            r.state = ScanState::Paused;
            r.error = Some(message);
        })
        .await;
    }

    async fn poll_loop(self, rec: ScanRecord, task_id: String, generation_id: u64) {
        let policy = self.inner.config.poll.clone();
        let mut interval = policy.first_delay;
        let mut delay = policy.first_delay;
        let mut errors = 0u32;
        loop {
            tokio::time::sleep(delay).await;
            if !self.is_current(&rec.id, generation_id) {
                return;
            }
            let client = self.state().client.clone();
            let Some(client) = client else {
                self.pause(
                    rec,
                    generation_id,
                    "Polling paused: no API key is set. Add it in Settings, then resume."
                        .to_owned(),
                )
                .await;
                return;
            };
            let outcome = client.get_task(&task_id).await;
            // Anything that happened to this scan while the request was in flight wins.
            if !self.is_current(&rec.id, generation_id) {
                return;
            }
            match outcome {
                Ok(TaskPoll::Pending { stage }) => {
                    errors = 0;
                    let message = match stage.as_deref() {
                        Some(s) => format!("Waiting for Pangram ({})…", humanize_stage(s)),
                        None => "Waiting for Pangram…".to_owned(),
                    };
                    self.emit(Event::Progress {
                        id: rec.id.clone(),
                        message,
                    });
                    interval = (interval * 3 / 2).min(policy.max_interval);
                    delay = interval;
                }
                Ok(TaskPoll::Succeeded { raw, result }) => {
                    // Scans from before price snapshots fall back to the current price.
                    let price = rec
                        .usd_per_credit
                        .unwrap_or_else(|| self.state().settings.usd_per_credit);
                    self.finish(rec, generation_id, |r| {
                        apply_success(r, raw, &result, price)
                    })
                    .await;
                    return;
                }
                Ok(TaskPoll::Failed { raw, message }) => {
                    self.finish(rec, generation_id, |r| {
                        r.state = ScanState::Failed;
                        r.error = Some(message);
                        r.raw_response = Some(raw);
                    })
                    .await;
                    return;
                }
                Err(ApiError::NotFound) => {
                    self.finish(rec, generation_id, |r| {
                        r.state = ScanState::Failed;
                        r.error = Some(ApiError::NotFound.to_string());
                    })
                    .await;
                    return;
                }
                Err(e) if e.is_transient() => {
                    errors += 1;
                    if errors >= policy.max_consecutive_errors {
                        self.pause(
                            rec,
                            generation_id,
                            format!(
                                "Polling paused after repeated errors. {e} Resume to try again."
                            ),
                        )
                        .await;
                        return;
                    }
                    delay = policy.error_delay(errors, e.retry_after());
                    self.emit(Event::Progress {
                        id: rec.id.clone(),
                        message: format!("{e} Retrying in {} s…", delay.as_secs().max(1)),
                    });
                }
                Err(e) => {
                    self.pause(rec, generation_id, format!("Polling paused. {e}"))
                        .await;
                    return;
                }
            }
        }
    }

    /// Stops polling locally. This does not cancel the task on Pangram's side.
    pub fn stop_polling(&self, id: &str) {
        let id = id.to_owned();
        let s = self.clone();
        self.spawn(async move {
            let history_generation = {
                let mut st = s.state();
                if st.pollers.remove(&id).is_none() {
                    return;
                }
                st.history_generation
            };
            if let Some(Ok(Some(mut rec))) = s
                .with_repo({
                    let id = id.clone();
                    move |r| r.get(&id)
                })
                .await
            {
                rec.state = ScanState::Paused;
                rec.error = Some(STOPPED.to_owned());
                rec.updated_at = now_ms();
                s.save(&rec, history_generation).await;
                s.emit(Event::Scan(rec));
            }
            s.emit_busy();
            s.refresh_history().await;
        });
    }

    /// Resumes polling a scan that has a known task ID. Never resubmits.
    pub fn resume(&self, id: &str) {
        let id = id.to_owned();
        let s = self.clone();
        self.spawn(async move {
            let history_generation = s.state().history_generation;
            let Some(Ok(Some(mut rec))) = s.with_repo({
                let id = id.clone();
                move |r| r.get(&id)
            }).await else {
                return;
            };
            let Some(task_id) = rec.task_id.clone() else {
                s.notice(
                    NoticeLevel::Warning,
                    "This scan has no Pangram task ID, so it can't be resumed. Use Edit and rescan to submit it again.",
                );
                return;
            };
            if !matches!(rec.state, ScanState::Paused | ScanState::Polling) {
                return;
            }
            let generation_id = {
                let mut st = s.state();
                // Deleted while the record was being read.
                if st.pollers.contains_key(&id)
                    || st.deleted.contains(&id)
                    || st.history_generation != history_generation
                {
                    return;
                }
                if st.busy() {
                    drop(st);
                    s.notice(NoticeLevel::Warning, "Another scan is still in progress.");
                    return;
                }
                st.register_poller(&id)
            };
            rec.state = ScanState::Polling;
            rec.error = None;
            rec.updated_at = now_ms();
            s.save(&rec, history_generation).await;
            s.emit(Event::Scan(rec.clone()));
            s.emit_busy();
            s.refresh_history().await;
            s.poll_loop(rec, task_id, generation_id).await;
        });
    }

    pub fn open(&self, id: &str) {
        let id = id.to_owned();
        let s = self.clone();
        self.spawn(async move {
            match s.with_repo(move |r| r.get(&id)).await {
                Some(Ok(Some(rec))) => s.emit(Event::Opened(rec)),
                Some(Err(e)) => s.notice(NoticeLevel::Error, e.to_string()),
                _ => s.notice(NoticeLevel::Warning, "That scan is no longer in history."),
            }
        });
    }

    /// Deletes the local copy only; Pangram's copy is unaffected.
    pub fn delete(&self, id: &str) {
        let id = id.to_owned();
        let s = self.clone();
        self.spawn(async move {
            let inner = s.inner.clone();
            let result = s
                .with_repo({
                    let id = id.clone();
                    move |r| {
                        // Under the repository lock: no pending write can land after this.
                        {
                            let mut st = inner.state();
                            st.pollers.remove(&id);
                            st.deleted.insert(id.clone());
                        }
                        r.delete(&id)
                    }
                })
                .await;
            if let Some(Err(e)) = result {
                s.notice(NoticeLevel::Error, format!("Couldn't delete the scan: {e}"));
            }
            s.emit(Event::Deleted { id: Some(id) });
            s.emit_busy();
            s.refresh_history().await;
        });
    }

    pub fn delete_all(&self) {
        let s = self.clone();
        self.spawn(async move {
            let inner = s.inner.clone();
            let deleted = s
                .with_repo(move |r| {
                    // Under the repository lock: every write begun before this is invalidated.
                    {
                        let mut st = inner.state();
                        st.history_generation += 1;
                        st.pollers.clear();
                    }
                    r.delete_all()
                })
                .await;
            match deleted {
                Some(Ok(n)) => s.notice(
                    NoticeLevel::Info,
                    format!("Deleted {n} local scan{}.", if n == 1 { "" } else { "s" }),
                ),
                Some(Err(e)) => {
                    s.notice(NoticeLevel::Error, format!("Couldn't delete history: {e}"))
                }
                None => {}
            }
            s.emit(Event::Deleted { id: None });
            s.emit_busy();
            s.refresh_history().await;
        });
    }

    pub fn set_save_history(&self, enabled: bool) {
        self.state().settings.save_history = enabled;
        let s = self.clone();
        self.spawn(async move {
            // Read the setting under the repository lock so the latest toggle always wins.
            let inner = s.inner.clone();
            s.with_repo(move |r| r.set_save_to_disk(inner.state().settings.save_history))
                .await;
            s.save_settings().await;
            s.refresh_history().await;
        });
    }

    /// Price used for new scans' cost estimates. Recorded costs keep the price at the time.
    pub fn set_usd_per_credit(&self, usd: f64) {
        if !usd.is_finite() || usd < 0.0 {
            return;
        }
        self.state().settings.usd_per_credit = usd;
        let s = self.clone();
        self.spawn(async move { s.save_settings().await });
    }

    pub fn set_minimize_to_tray(&self, enabled: bool) {
        self.state().settings.minimize_to_tray = enabled;
        let s = self.clone();
        self.spawn(async move { s.save_settings().await });
    }

    /// Requests usage grouped by `"day"`, `"week"` or `"month"`; later changes re-emit it.
    pub fn usage_summary(&self, period: &str) {
        let Some(period) = UsagePeriod::parse(period) else {
            return;
        };
        self.state().usage_period = Some(period);
        let s = self.clone();
        self.spawn(async move { s.refresh_usage().await });
    }

    pub fn clear_usage(&self) {
        let s = self.clone();
        self.spawn(async move {
            if let Some(Err(e)) = s.with_repo(|r| r.clear_usage()).await {
                s.notice(
                    NoticeLevel::Error,
                    format!("Couldn't clear usage records: {e}"),
                );
            }
            s.refresh_usage().await;
        });
    }

    async fn refresh_usage(&self) {
        let Some(period) = self.state().usage_period else {
            return;
        };
        match self.with_repo(move |r| r.usage_summary(period)).await {
            Some(Ok(rows)) => self.emit(Event::Usage {
                period: match period {
                    UsagePeriod::Day => "day",
                    UsagePeriod::Week => "week",
                    UsagePeriod::Month => "month",
                }
                .to_owned(),
                rows,
            }),
            Some(Err(e)) => self.notice(NoticeLevel::Error, format!("Couldn't read usage: {e}")),
            None => {}
        }
    }

    pub fn search_history(&self, query: &str) {
        self.state().history_query = query.to_owned();
        let s = self.clone();
        self.spawn(async move { s.refresh_history().await });
    }

    async fn refresh_history(&self) {
        let query = self.state().history_query.clone();
        let result = self
            .with_repo(move |r| {
                r.list(&query, HISTORY_LIMIT)
                    .map(|items| (items, r.saved_count(), r.has_disk()))
            })
            .await;
        match result {
            Some(Ok((items, saved_count, disk_available))) => {
                self.emit(Event::History(HistoryStatus {
                    items,
                    saved_count,
                    disk_available,
                }))
            }
            Some(Err(e)) => self.notice(NoticeLevel::Error, format!("Couldn't read history: {e}")),
            None => {}
        }
    }

    /// Saves a record unless it was deleted since its operation began.
    async fn save(&self, rec: &ScanRecord, history_generation: u64) {
        let r = rec.clone();
        let inner = self.inner.clone();
        let outcome = self
            .with_repo(move |repo| {
                if !inner.may_write(&r.id, history_generation) {
                    return Ok(None);
                }
                repo.save(&r).map(Some)
            })
            .await;
        self.report_save(outcome);
    }

    /// Saves a completed scan together with its usage row.
    async fn save_completed(&self, rec: &ScanRecord, usage: UsageRecord, history_generation: u64) {
        let r = rec.clone();
        let inner = self.inner.clone();
        let outcome = self
            .with_repo(move |repo| {
                if !inner.may_write(&r.id, history_generation) {
                    return Ok(None);
                }
                repo.save_completed(&r, &usage).map(Some)
            })
            .await;
        self.report_save(outcome);
    }

    fn report_save(&self, outcome: Option<Result<Option<SaveOutcome>, StorageError>>) {
        match outcome {
            Some(Ok(Some(SaveOutcome::SessionAfterError(e)))) => self.notice(
                NoticeLevel::Error,
                format!(
                    "Couldn't save this scan to history ({e}). It is kept for this session; copy the result if you need it."
                ),
            ),
            Some(Ok(Some(SaveOutcome::UsageSessionAfterError(e)))) => self.notice(
                NoticeLevel::Warning,
                format!(
                    "The result is kept, but its cost couldn't be recorded on disk ({e}). It counts in Usage and cost for this session only."
                ),
            ),
            Some(Err(e)) => self.notice(NoticeLevel::Error, format!("Couldn't record this scan: {e}")),
            _ => {}
        }
    }
}

/// The usage row for a scan that completed with a recorded charge.
fn usage_for(rec: &ScanRecord) -> Option<UsageRecord> {
    if rec.state != ScanState::Completed {
        return None;
    }
    Some(UsageRecord {
        scan_id: rec.id.clone(),
        created_at: rec.created_at,
        model: rec.requested_model.clone(),
        version: rec.returned_version.clone(),
        words: rec.billed_words?,
        credits: rec.credits?,
        cost_usd: rec.cost_usd?,
    })
}

fn apply_success(rec: &mut ScanRecord, raw: String, result: &DetectionResult, usd_per_credit: f64) {
    let charge = cost::charge_for_result(result, &rec.requested_model, usd_per_credit);
    rec.billed_words = Some(charge.words);
    rec.credits = Some(charge.credits);
    rec.cost_usd = Some(charge.usd);
    rec.state = ScanState::Completed;
    rec.error = None;
    rec.returned_text = Some(result.text.clone());
    rec.returned_version = result.version.clone();
    rec.prediction_short = result.prediction_short.clone();
    rec.fraction_ai = result.fraction_ai;
    rec.raw_response = Some(raw);
}

fn humanize_stage(stage: &str) -> String {
    stage
        .strip_prefix("STAGE_")
        .unwrap_or(stage)
        .to_ascii_lowercase()
        .replace('_', " ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles_fall_back_to_first_line() {
        assert_eq!(derive_title("  Mine ", "text"), "Mine");
        assert_eq!(derive_title("", "\n\n  First line \nsecond"), "First line");
        assert_eq!(derive_title("", "   "), "Untitled");
        let long = "x".repeat(200);
        assert_eq!(derive_title("", &long).chars().count(), 81);
    }

    #[test]
    fn error_backoff_is_capped_and_honours_hint() {
        let p = PollPolicy::default();
        assert_eq!(p.error_delay(1, None), Duration::from_secs(2));
        assert_eq!(p.error_delay(3, None), Duration::from_secs(8));
        assert_eq!(p.error_delay(30, None), Duration::from_secs(60));
        assert_eq!(
            p.error_delay(1, Some(Duration::from_secs(9))),
            Duration::from_secs(9)
        );
        assert_eq!(
            p.error_delay(1, Some(Duration::from_secs(900))),
            Duration::from_secs(900),
            "a server-requested delay is never shortened"
        );
    }

    #[test]
    fn stages_are_readable() {
        assert_eq!(humanize_stage("STAGE_PREPROCESSING"), "preprocessing");
    }
}
