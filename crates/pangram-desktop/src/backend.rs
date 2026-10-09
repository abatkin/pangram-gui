//! Thin CXX-Qt adapter exposing the core service to QML.
//!
//! Core events arrive on runtime threads. They are converted to UI payloads there (including
//! result interpretation, which can be expensive for long documents) and then queued onto the Qt
//! thread, where they only update properties.

#[cxx_qt::bridge]
pub mod qobject {
    unsafe extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
        include!("cxx-qt-lib/qstringlist.h");
        type QStringList = cxx_qt_lib::QStringList;
    }

    #[auto_cxx_name]
    extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[qproperty(bool, ready)]
        #[qproperty(bool, has_api_key)]
        #[qproperty(bool, key_persisted)]
        #[qproperty(QString, key_note)]
        #[qproperty(QStringList, models)]
        #[qproperty(QString, selected_model)]
        #[qproperty(bool, models_loading)]
        #[qproperty(QString, models_error)]
        #[qproperty(QString, history_json)]
        #[qproperty(i32, saved_count)]
        #[qproperty(bool, history_disk_available)]
        #[qproperty(bool, save_history)]
        #[qproperty(bool, remember_key)]
        #[qproperty(bool, minimize_to_tray)]
        #[qproperty(bool, busy)]
        #[qproperty(QString, current_id)]
        #[qproperty(QString, current_json)]
        #[qproperty(i32, current_revision)]
        #[qproperty(QString, progress)]
        #[qproperty(f64, usd_per_credit)]
        #[qproperty(QString, usage_json)]
        #[qproperty(QString, usage_period)]
        #[qproperty(QString, history_path)]
        type Backend = super::BackendRust;
    }

    #[auto_cxx_name]
    extern "RustQt" {
        #[qinvokable]
        fn initialize(self: Pin<&mut Backend>);

        #[qinvokable]
        fn set_api_key(self: Pin<&mut Backend>, key: &QString, remember: bool);

        #[qinvokable]
        fn forget_api_key(self: Pin<&mut Backend>);

        #[qinvokable]
        fn refresh_models(self: Pin<&mut Backend>);

        #[qinvokable]
        fn select_model(self: Pin<&mut Backend>, model: &QString);

        /// Returns the new scan's ID, or an empty string (with a notice) if it could not start.
        #[qinvokable]
        fn analyze(
            self: Pin<&mut Backend>,
            title: &QString,
            text: &QString,
            source_id: &QString,
        ) -> QString;

        #[qinvokable]
        fn open_scan(self: Pin<&mut Backend>, id: &QString);

        #[qinvokable]
        fn clear_current(self: Pin<&mut Backend>);

        #[qinvokable]
        fn stop_polling(self: Pin<&mut Backend>, id: &QString);

        #[qinvokable]
        fn resume_polling(self: Pin<&mut Backend>, id: &QString);

        #[qinvokable]
        fn delete_scan(self: Pin<&mut Backend>, id: &QString);

        #[qinvokable]
        fn delete_all_scans(self: Pin<&mut Backend>);

        #[qinvokable]
        fn set_history_enabled(self: Pin<&mut Backend>, enabled: bool);

        #[qinvokable]
        fn set_tray_enabled(self: Pin<&mut Backend>, enabled: bool);

        #[qinvokable]
        fn search_history(self: Pin<&mut Backend>, query: &QString);

        /// Highlighted, escaped rich text for the current result.
        #[qinvokable]
        fn result_html(
            self: &Backend,
            ai: &QString,
            assisted: &QString,
            human: &QString,
        ) -> QString;

        /// Section index at a UTF-16 position in the analyzed text, or -1.
        #[qinvokable]
        fn section_at(self: &Backend, position: i32) -> i32;

        #[qinvokable]
        fn summary_text(self: &Backend) -> QString;

        #[qinvokable]
        fn save_usd_per_credit(self: Pin<&mut Backend>, usd: f64);

        /// Loads usage grouped by "day", "week" or "month" into `usageJson`.
        #[qinvokable]
        fn request_usage(self: Pin<&mut Backend>, period: &QString);

        #[qinvokable]
        fn clear_usage(self: Pin<&mut Backend>);

        /// Credits a draft of `words` words would cost with `model` (an estimate).
        #[qinvokable]
        fn estimate_credits(self: &Backend, words: i32, model: &QString) -> i32;

        /// JSON object with the current scan's request, submission response and final
        /// response, each formatted for display. The API key is never included.
        #[qinvokable]
        fn raw_details(self: &Backend) -> QString;

        /// True in debug builds; gates development aids such as `--qml-script`.
        #[qinvokable]
        fn developer_build(self: &Backend) -> bool;

        #[qsignal]
        fn notice(self: Pin<&mut Backend>, level: QString, message: QString);

        /// The displayed scan was deleted.
        #[qsignal]
        fn current_cleared(self: Pin<&mut Backend>);

        /// Another launch asked this instance to show its window.
        #[qsignal]
        fn activation_requested(self: Pin<&mut Backend>);

        /// A scan, displayed or not, just ended as "completed", "failed" or "submissionUnknown".
        /// The fractions are -1 when unknown.
        #[qsignal]
        fn scan_finished(
            self: Pin<&mut Backend>,
            state: QString,
            fraction_ai: f64,
            fraction_ai_assisted: f64,
            fraction_human: f64,
        );
    }

    impl cxx_qt::Threading for Backend {}
}

use std::pin::Pin;
use std::sync::{Arc, Mutex, OnceLock};

use cxx_qt::{CxxQtType, Threading};
use cxx_qt_lib::{QString, QStringList};
use pangram_core::analysis::{Analysis, HighlightPalette, describe_normalization};
use pangram_core::api::{ClientOptions, DetectionResult, task_request_body};
use pangram_core::cost;
use pangram_core::credentials::CredentialStore;
use pangram_core::instance;
use pangram_core::service::{Config, Event, NoticeLevel, PollPolicy, Service};
use pangram_core::settings::Paths;
use pangram_core::storage::{ScanRecord, ScanState};
use serde::Serialize;

pub fn runtime() -> &'static tokio::runtime::Runtime {
    static RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RT.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .max_blocking_threads(4)
            .thread_name("pangram-worker")
            .enable_all()
            .build()
            .expect("failed to start background runtime")
    })
}

static INSTANCE: Mutex<Option<instance::Primary>> = Mutex::new(None);

/// Keeps the single-instance lock until the backend starts serving activation requests.
pub fn set_instance(primary: instance::Primary) {
    *INSTANCE.lock().unwrap_or_else(|e| e.into_inner()) = Some(primary);
}

#[derive(Default)]
pub struct BackendRust {
    ready: bool,
    has_api_key: bool,
    key_persisted: bool,
    key_note: QString,
    models: QStringList,
    selected_model: QString,
    models_loading: bool,
    models_error: QString,
    history_json: QString,
    saved_count: i32,
    history_disk_available: bool,
    save_history: bool,
    remember_key: bool,
    minimize_to_tray: bool,
    busy: bool,
    current_id: QString,
    current_json: QString,
    current_revision: i32,
    progress: QString,
    usd_per_credit: f64,
    usage_json: QString,
    usage_period: QString,
    history_path: QString,

    service: Option<Service>,
    base_url: String,
    current: Option<CurrentScan>,
}

struct CurrentScan {
    record: ScanRecord,
    analysis: Option<Arc<Analysis>>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ScanView<'a> {
    id: &'a str,
    title: &'a str,
    state: ScanState,
    error: Option<&'a str>,
    created_at: i64,
    updated_at: i64,
    input_text: &'a str,
    requested_model: &'a str,
    returned_version: Option<&'a str>,
    task_id: Option<&'a str>,
    source_scan_id: Option<&'a str>,
    /// How Pangram changed the submitted text, if it did.
    normalization_note: Option<String>,
    billed_words: Option<i64>,
    credits: Option<i64>,
    cost_usd: Option<f64>,
    result: Option<&'a Analysis>,
    result_error: Option<String>,
}

/// A scan prepared off the UI thread.
struct PreparedScan {
    record: ScanRecord,
    analysis: Option<Arc<Analysis>>,
    json: String,
}

fn prepare(record: ScanRecord) -> PreparedScan {
    let mut result_error = None;
    let analysis = match (&record.state, &record.raw_response) {
        (ScanState::Completed, Some(raw)) => match DetectionResult::from_json_str(raw) {
            Ok(result) => Some(Arc::new(Analysis::from_result(&result))),
            Err(e) => {
                result_error = Some(format!("The stored result could not be read: {e}"));
                None
            }
        },
        _ => None,
    };
    let view = ScanView {
        id: &record.id,
        title: &record.title,
        state: record.state,
        error: record.error.as_deref(),
        created_at: record.created_at,
        updated_at: record.updated_at,
        input_text: &record.input_text,
        requested_model: &record.requested_model,
        returned_version: record.returned_version.as_deref(),
        task_id: record.task_id.as_deref(),
        source_scan_id: record.source_scan_id.as_deref(),
        normalization_note: record
            .returned_text
            .as_deref()
            .and_then(|t| describe_normalization(&record.input_text, t)),
        billed_words: record.billed_words,
        credits: record.credits,
        cost_usd: record.cost_usd,
        result: analysis.as_deref(),
        result_error,
    };
    let json = serde_json::to_string(&view).unwrap_or_default();
    PreparedScan {
        record,
        analysis,
        json,
    }
}

enum UiUpdate {
    Event(Event),
    Scan(PreparedScan),
    Opened(PreparedScan),
    History {
        json: String,
        saved: usize,
        disk: bool,
    },
}

impl UiUpdate {
    fn from_event(event: Event) -> Self {
        match event {
            Event::Scan(record) => Self::Scan(prepare(record)),
            Event::Opened(record) => Self::Opened(prepare(record)),
            Event::History(h) => Self::History {
                json: serde_json::to_string(&h.items).unwrap_or_else(|_| "[]".to_owned()),
                saved: h.saved_count,
                disk: h.disk_available,
            },
            other => Self::Event(other),
        }
    }
}

impl qobject::Backend {
    fn service(&self) -> Option<&Service> {
        self.rust().service.as_ref()
    }

    fn emit_notice(self: Pin<&mut Self>, level: NoticeLevel, message: &str) {
        let level = match level {
            NoticeLevel::Info => "info",
            NoticeLevel::Warning => "warning",
            NoticeLevel::Error => "error",
        };
        self.notice(QString::from(level), QString::from(message));
    }

    pub fn initialize(mut self: Pin<&mut Self>) {
        if self.service().is_some() {
            return;
        }
        let qt_thread = self.qt_thread();
        let base_url = std::env::var("PANGRAM_API_BASE")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| ClientOptions::default().base_url);
        let paths = Paths::from_env();
        self.as_mut()
            .set_history_path(QString::from(paths.history_db().to_string_lossy().as_ref()));
        self.as_mut().rust_mut().base_url = base_url.clone();
        let config = Config {
            paths,
            client: ClientOptions {
                base_url,
                ..ClientOptions::default()
            },
            poll: PollPolicy::default(),
            // `memory` keeps keys out of the system keyring (for tests and UI smoke runs).
            credentials: match std::env::var("PANGRAM_CREDENTIAL_STORE").as_deref() {
                Ok("memory") => CredentialStore::Memory(Default::default()),
                _ => CredentialStore::secret_service(),
            },
        };
        if let Some(primary) = INSTANCE.lock().unwrap_or_else(|e| e.into_inner()).take() {
            let gui = self.qt_thread();
            primary.listen(move |token| {
                let _ = gui.queue(move |backend| backend.activate(token));
            });
        }
        let service = Service::start(config, runtime().handle().clone(), move |event| {
            let update = UiUpdate::from_event(event);
            // Fails only when the QObject is gone (application shutting down).
            let _ = qt_thread.queue(move |backend| backend.apply(update));
        });
        self.as_mut().rust_mut().service = Some(service);
    }

    fn activate(self: Pin<&mut Self>, token: Option<String>) {
        if let Some(token) = token {
            // Qt's Wayland plugin activates the window with this token on the next
            // requestActivate() and then unsets it; Qt's tray code sets it the same way.
            // SAFETY: Qt reads it on this (GUI) thread. Rust threads (HTTP and D-Bus clients)
            // read the environment under the same std lock as set_var.
            unsafe { std::env::set_var("XDG_ACTIVATION_TOKEN", token) };
        }
        self.activation_requested();
    }

    fn show(mut self: Pin<&mut Self>, scan: PreparedScan) {
        // Progress messages describe one state; drop them when the state moves on.
        let state_changed =
            self.rust().current.as_ref().is_none_or(|c| {
                c.record.id != scan.record.id || c.record.state != scan.record.state
            });
        if state_changed {
            self.as_mut().set_progress(QString::default());
        }
        self.as_mut().set_current_id(QString::from(&scan.record.id));
        self.as_mut().set_current_json(QString::from(&scan.json));
        self.as_mut().rust_mut().current = Some(CurrentScan {
            record: scan.record,
            analysis: scan.analysis,
        });
        let revision = self.current_revision().wrapping_add(1);
        self.set_current_revision(revision);
    }

    fn apply(mut self: Pin<&mut Self>, update: UiUpdate) {
        match update {
            UiUpdate::Scan(scan) => {
                // The service sends these states only when a scan reaches them; reopening a
                // scan arrives as `Opened`.
                if matches!(
                    scan.record.state,
                    ScanState::Completed | ScanState::Failed | ScanState::SubmissionUnknown
                ) {
                    let fraction = |f: Option<f64>| f.unwrap_or(-1.0);
                    let a = scan.analysis.as_deref();
                    self.as_mut().scan_finished(
                        QString::from(scan.record.state.as_str()),
                        fraction(a.and_then(|a| a.fraction_ai)),
                        fraction(a.and_then(|a| a.fraction_ai_assisted)),
                        fraction(a.and_then(|a| a.fraction_human)),
                    );
                }
                if self.current_id().to_string() == scan.record.id {
                    self.show(scan);
                }
            }
            UiUpdate::Opened(scan) => {
                // Only show it if it is still the scan the user asked for.
                if self.current_id().to_string() == scan.record.id {
                    self.as_mut().set_progress(QString::default());
                    self.show(scan);
                }
            }
            UiUpdate::History { json, saved, disk } => {
                self.as_mut().set_history_json(QString::from(&json));
                self.as_mut()
                    .set_saved_count(saved.min(i32::MAX as usize) as i32);
                self.set_history_disk_available(disk);
            }
            UiUpdate::Event(event) => self.apply_event(event),
        }
    }

    fn apply_event(mut self: Pin<&mut Self>, event: Event) {
        match event {
            Event::Ready => self.set_ready(true),
            Event::Settings(s) => {
                self.as_mut().set_save_history(s.save_history);
                self.as_mut().set_usd_per_credit(s.usd_per_credit);
                self.as_mut().set_minimize_to_tray(s.minimize_to_tray);
                self.set_remember_key(s.remember_key);
            }
            Event::Credentials(c) => {
                self.as_mut().set_has_api_key(c.has_key);
                self.as_mut().set_key_persisted(c.persisted);
                self.set_key_note(QString::from(c.note.as_deref().unwrap_or("")));
            }
            Event::Models(m) => {
                let list: QStringList = m.models.iter().map(QString::from).collect();
                self.as_mut().set_models(list);
                self.as_mut()
                    .set_selected_model(QString::from(m.selected.as_deref().unwrap_or("")));
                self.as_mut().set_models_loading(m.loading);
                self.set_models_error(QString::from(m.error.as_deref().unwrap_or("")));
            }
            Event::Progress { id, message } => {
                if self.current_id().to_string() == id {
                    self.set_progress(QString::from(&message));
                }
            }
            Event::Deleted { id } => {
                let current = self.current_id().to_string();
                if !current.is_empty() && id.as_ref().is_none_or(|d| *d == current) {
                    self.as_mut().clear_current();
                    self.current_cleared();
                }
            }
            Event::Busy(busy) => self.set_busy(busy),
            Event::Notice { level, message } => self.emit_notice(level, &message),
            Event::Usage { period, rows } => {
                self.as_mut().set_usage_period(QString::from(&period));
                let json = serde_json::to_string(&rows).unwrap_or_else(|_| "[]".to_owned());
                self.set_usage_json(QString::from(&json));
            }
            // Converted before queueing.
            Event::Scan(_) | Event::Opened(_) | Event::History(_) => {}
        }
    }

    pub fn set_api_key(self: Pin<&mut Self>, key: &QString, remember: bool) {
        if let Some(s) = self.service() {
            s.set_api_key(&key.to_string(), remember);
        }
    }

    pub fn forget_api_key(self: Pin<&mut Self>) {
        if let Some(s) = self.service() {
            s.forget_api_key();
        }
    }

    pub fn refresh_models(self: Pin<&mut Self>) {
        if let Some(s) = self.service() {
            s.refresh_models();
        }
    }

    pub fn select_model(self: Pin<&mut Self>, model: &QString) {
        if let Some(s) = self.service() {
            s.select_model(&model.to_string());
        }
    }

    pub fn analyze(
        mut self: Pin<&mut Self>,
        title: &QString,
        text: &QString,
        source_id: &QString,
    ) -> QString {
        let Some(service) = self.service().cloned() else {
            return QString::default();
        };
        let source = Some(source_id.to_string()).filter(|s| !s.is_empty());
        match service.analyze(&title.to_string(), &text.to_string(), source) {
            Ok(id) => {
                // Events for this scan are queued behind this call, so they will match.
                let id = QString::from(&id);
                self.as_mut().rust_mut().current = None;
                self.as_mut().set_current_json(QString::default());
                self.as_mut().set_current_id(id.clone());
                id
            }
            Err(message) => {
                self.emit_notice(NoticeLevel::Warning, &message);
                QString::default()
            }
        }
    }

    pub fn open_scan(mut self: Pin<&mut Self>, id: &QString) {
        let Some(service) = self.service().cloned() else {
            return;
        };
        if *self.current_id() != *id {
            self.as_mut().rust_mut().current = None;
            self.as_mut().set_current_json(QString::default());
            self.as_mut().set_progress(QString::default());
            self.as_mut().set_current_id(id.clone());
        }
        service.open(&id.to_string());
    }

    pub fn clear_current(mut self: Pin<&mut Self>) {
        self.as_mut().rust_mut().current = None;
        self.as_mut().set_current_id(QString::default());
        self.as_mut().set_current_json(QString::default());
        self.as_mut().set_progress(QString::default());
        let revision = self.current_revision().wrapping_add(1);
        self.set_current_revision(revision);
    }

    pub fn stop_polling(self: Pin<&mut Self>, id: &QString) {
        if let Some(s) = self.service() {
            s.stop_polling(&id.to_string());
        }
    }

    pub fn resume_polling(self: Pin<&mut Self>, id: &QString) {
        if let Some(s) = self.service() {
            s.resume(&id.to_string());
        }
    }

    pub fn delete_scan(self: Pin<&mut Self>, id: &QString) {
        if let Some(s) = self.service() {
            s.delete(&id.to_string());
        }
    }

    pub fn delete_all_scans(self: Pin<&mut Self>) {
        if let Some(s) = self.service() {
            s.delete_all();
        }
    }

    pub fn set_history_enabled(self: Pin<&mut Self>, enabled: bool) {
        if let Some(s) = self.service() {
            s.set_save_history(enabled);
        }
    }

    pub fn set_tray_enabled(self: Pin<&mut Self>, enabled: bool) {
        if let Some(s) = self.service() {
            s.set_minimize_to_tray(enabled);
        }
    }

    pub fn search_history(self: Pin<&mut Self>, query: &QString) {
        if let Some(s) = self.service() {
            s.search_history(&query.to_string());
        }
    }

    fn analysis(&self) -> Option<&Analysis> {
        self.rust().current.as_ref()?.analysis.as_deref()
    }

    pub fn result_html(&self, ai: &QString, assisted: &QString, human: &QString) -> QString {
        let Some(analysis) = self.analysis() else {
            return QString::default();
        };
        let palette = HighlightPalette {
            ai: ai.to_string(),
            assisted: assisted.to_string(),
            human: human.to_string(),
        };
        QString::from(&analysis.to_html(&palette))
    }

    pub fn section_at(&self, position: i32) -> i32 {
        let Ok(pos) = usize::try_from(position) else {
            return -1;
        };
        self.analysis()
            .and_then(|a| a.section_at(pos))
            .map_or(-1, |i| i as i32)
    }

    pub fn summary_text(&self) -> QString {
        let Some(current) = self.rust().current.as_ref() else {
            return QString::default();
        };
        let Some(analysis) = current.analysis.as_deref() else {
            return QString::default();
        };
        QString::from(
            &analysis.summary_text(&current.record.title, &current.record.requested_model),
        )
    }

    pub fn save_usd_per_credit(self: Pin<&mut Self>, usd: f64) {
        if let Some(s) = self.service() {
            s.set_usd_per_credit(usd);
        }
    }

    pub fn request_usage(self: Pin<&mut Self>, period: &QString) {
        if let Some(s) = self.service() {
            s.usage_summary(&period.to_string());
        }
    }

    pub fn clear_usage(self: Pin<&mut Self>) {
        if let Some(s) = self.service() {
            s.clear_usage();
        }
    }

    pub fn estimate_credits(&self, words: i32, model: &QString) -> i32 {
        let credits = cost::credits(
            words.max(0).into(),
            cost::words_per_credit(&model.to_string()),
        );
        credits.min(i32::MAX.into()) as i32
    }

    pub fn raw_details(&self) -> QString {
        let Some(current) = self.rust().current.as_ref() else {
            return QString::default();
        };
        let r = &current.record;
        let base = self.rust().base_url.trim_end_matches('/');
        let pretty = |raw: &str| {
            serde_json::from_str::<serde_json::Value>(raw)
                .and_then(|v| serde_json::to_string_pretty(&v))
                .unwrap_or_else(|_| raw.to_owned())
        };
        let body = task_request_body(&r.input_text, &r.requested_model);
        let request = format!(
            "POST {base}/task\nx-api-key: (hidden)\ncontent-type: application/json\n\n{}",
            serde_json::to_string_pretty(&body).unwrap_or_default()
        );
        let submit_response = match &r.submit_response {
            Some(raw) => format!("POST {base}/task\n\n{}", pretty(raw)),
            None if r.task_id.is_some() => format!(
                "Not recorded for this scan (saved by an earlier version).\n\ntask_id: {}",
                r.task_id.as_deref().unwrap_or_default()
            ),
            None => "No response was received.".to_owned(),
        };
        let response = match (&r.raw_response, &r.task_id) {
            (Some(raw), Some(task)) => format!("GET {base}/task/{task}\n\n{}", pretty(raw)),
            (Some(raw), None) => pretty(raw),
            (None, _) => "No final result yet.".to_owned(),
        };
        let json = serde_json::json!({
            "request": request,
            "submitResponse": submit_response,
            "response": response,
        });
        QString::from(&json.to_string())
    }

    pub fn developer_build(&self) -> bool {
        cfg!(debug_assertions)
    }
}
