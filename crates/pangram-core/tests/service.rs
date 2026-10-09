//! Orchestration tests against a local mock server. No real API calls.

use std::sync::Mutex;
use std::time::Duration;

use pangram_core::api::ClientOptions;
use pangram_core::credentials::CredentialStore;
use pangram_core::service::{Config, Event, PollPolicy, Service};
use pangram_core::settings::{Paths, Settings};
use pangram_core::storage::{Db, ScanRecord, ScanState};
use tokio::sync::mpsc;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const SUCCESS: &str = r#"{"stage":"STAGE_SUCCESS","text":"Hello world.","version":"4.0",
  "headline":"AI Detected","prediction_short":"AI","fraction_ai":1.0,"fraction_ai_assisted":0.0,
  "fraction_human":0.0,"windows":[{"text":"Hello world.","label":"AI-Generated","confidence":"High",
  "start_index":0,"end_index":12}]}"#;

struct Harness {
    service: Service,
    events: mpsc::UnboundedReceiver<Event>,
    paths: Paths,
    _dir: tempfile::TempDir,
}

fn paths_in(dir: &tempfile::TempDir) -> Paths {
    Paths {
        config_dir: dir.path().join("config"),
        data_dir: dir.path().join("data"),
    }
}

fn start_with(server: &MockServer, dir: tempfile::TempDir, submit_timeout: Duration) -> Harness {
    start_custom(
        server,
        dir,
        submit_timeout,
        CredentialStore::Memory(Mutex::new(Some("test-key".into()))),
    )
}

fn start_custom(
    server: &MockServer,
    dir: tempfile::TempDir,
    submit_timeout: Duration,
    credentials: CredentialStore,
) -> Harness {
    let paths = paths_in(&dir);
    let (tx, events) = mpsc::unbounded_channel();
    let service = Service::start(
        Config {
            paths: paths.clone(),
            client: ClientOptions {
                base_url: server.uri(),
                connect_timeout: Duration::from_secs(2),
                read_timeout: Duration::from_secs(2),
                submit_timeout,
            },
            poll: PollPolicy {
                first_delay: Duration::from_millis(10),
                max_interval: Duration::from_millis(30),
                error_base: Duration::from_millis(10),
                max_error_delay: Duration::from_millis(50),
                max_consecutive_errors: 3,
            },
            credentials,
        },
        tokio::runtime::Handle::current(),
        move |e| {
            let _ = tx.send(e);
        },
    );
    Harness {
        service,
        events,
        paths,
        _dir: dir,
    }
}

fn start(server: &MockServer) -> Harness {
    start_with(server, tempfile::tempdir().unwrap(), Duration::from_secs(2))
}

impl Harness {
    async fn wait_for<T>(&mut self, mut f: impl FnMut(&Event) -> Option<T>) -> T {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let e = self.events.recv().await.expect("service dropped");
                if let Some(v) = f(&e) {
                    return v;
                }
            }
        })
        .await
        .expect("timed out waiting for event")
    }

    async fn wait_scan(&mut self, id: &str, state: ScanState) -> ScanRecord {
        self.wait_for(|e| match e {
            Event::Scan(r) if r.id == id && r.state == state => Some(r.clone()),
            _ => None,
        })
        .await
    }

    async fn wait_models(&mut self) {
        self.wait_for(|e| match e {
            Event::Models(m) if m.selected.is_some() && !m.loading => Some(()),
            _ => None,
        })
        .await
    }

    /// Asserts no event matching `f` arrives within `window`.
    async fn assert_quiet(&mut self, window: Duration, mut f: impl FnMut(&Event) -> bool) {
        let deadline = tokio::time::Instant::now() + window;
        while let Ok(Some(e)) = tokio::time::timeout_at(deadline, self.events.recv()).await {
            assert!(!f(&e), "unexpected event: {e:?}");
        }
    }

    fn stored(&self, id: &str) -> Option<ScanRecord> {
        Db::open(&self.paths.history_db()).unwrap().get(id).unwrap()
    }
}

async fn mount_models(server: &MockServer) {
    Mock::given(method("GET"))
        .and(path("/models"))
        .respond_with(
            ResponseTemplate::new(200).set_body_string(r#"{"models":["pangram-4","default"]}"#),
        )
        .mount(server)
        .await;
}

async fn mount_submit(server: &MockServer, task_id: &str, expect: u64) {
    Mock::given(method("POST"))
        .and(path("/task"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(serde_json::json!({ "task_id": task_id })),
        )
        .expect(expect)
        .mount(server)
        .await;
}

fn pending() -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_string(r#"{"task_id":"t1","stage":"STAGE_PREPROCESSING"}"#)
}

#[tokio::test(flavor = "multi_thread")]
async fn completes_a_scan_and_persists_it() {
    let server = MockServer::start().await;
    mount_models(&server).await;
    mount_submit(&server, "t1", 1).await;
    Mock::given(method("GET"))
        .and(path("/task/t1"))
        .respond_with(pending())
        .up_to_n_times(2)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/task/t1"))
        .respond_with(ResponseTemplate::new(200).set_body_string(SUCCESS))
        .mount(&server)
        .await;

    let mut h = start(&server);
    h.wait_models().await;
    let id = h.service.analyze("", "Hello   world.", None).unwrap();
    let submitting = h.wait_scan(&id, ScanState::Submitting).await;
    assert_eq!(submitting.requested_model, "default");
    assert_eq!(submitting.title, "Hello   world.");
    let polling = h.wait_scan(&id, ScanState::Polling).await;
    assert_eq!(polling.task_id.as_deref(), Some("t1"));
    h.wait_for(|e| matches!(e, Event::Progress { id: p, .. } if *p == id).then_some(()))
        .await;
    let done = h.wait_scan(&id, ScanState::Completed).await;
    assert_eq!(done.input_text, "Hello   world.");
    assert_eq!(done.returned_text.as_deref(), Some("Hello world."));
    assert_eq!(done.returned_version.as_deref(), Some("4.0"));
    assert_eq!(done.prediction_short.as_deref(), Some("AI"));
    h.wait_for(|e| matches!(e, Event::Busy(false)).then_some(()))
        .await;

    // Reopening reads local history only.
    h.service.open(&id);
    let opened = h
        .wait_for(|e| match e {
            Event::Opened(r) => Some(r.clone()),
            _ => None,
        })
        .await;
    assert_eq!(opened.raw_response.as_deref(), Some(SUCCESS));
    assert_eq!(h.stored(&id).unwrap().state, ScanState::Completed);
}

#[tokio::test(flavor = "multi_thread")]
async fn terminal_failure_is_recorded() {
    let server = MockServer::start().await;
    mount_models(&server).await;
    mount_submit(&server, "t1", 1).await;
    Mock::given(method("GET"))
        .and(path("/task/t1"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"{"stage":"STAGE_FAILED","text":"","headline":"Preprocessing error: no valid text","windows":[]}"#,
        ))
        .mount(&server)
        .await;
    let mut h = start(&server);
    h.wait_models().await;
    let id = h.service.analyze("t", "?!", None).unwrap();
    let failed = h.wait_scan(&id, ScanState::Failed).await;
    assert_eq!(
        failed.error.as_deref(),
        Some("Preprocessing error: no valid text")
    );
    assert!(failed.raw_response.is_some());
}

#[tokio::test(flavor = "multi_thread")]
async fn transient_poll_errors_are_retried() {
    let server = MockServer::start().await;
    mount_models(&server).await;
    mount_submit(&server, "t1", 1).await;
    Mock::given(method("GET"))
        .and(path("/task/t1"))
        .respond_with(ResponseTemplate::new(500))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/task/t1"))
        .respond_with(ResponseTemplate::new(429).insert_header("Retry-After", "0"))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/task/t1"))
        .respond_with(ResponseTemplate::new(200).set_body_string(SUCCESS))
        .mount(&server)
        .await;
    let mut h = start(&server);
    h.wait_models().await;
    let id = h.service.analyze("", "Hello world.", None).unwrap();
    h.wait_scan(&id, ScanState::Completed).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn persistent_poll_errors_pause_and_resume_works() {
    let server = MockServer::start().await;
    mount_models(&server).await;
    mount_submit(&server, "t1", 1).await;
    Mock::given(method("GET"))
        .and(path("/task/t1"))
        .respond_with(ResponseTemplate::new(502))
        .up_to_n_times(3)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/task/t1"))
        .respond_with(ResponseTemplate::new(200).set_body_string(SUCCESS))
        .mount(&server)
        .await;
    let mut h = start(&server);
    h.wait_models().await;
    let id = h.service.analyze("", "Hello world.", None).unwrap();
    let paused = h.wait_scan(&id, ScanState::Paused).await;
    assert!(paused.error.unwrap().contains("repeated errors"));
    h.service.resume(&id);
    h.wait_scan(&id, ScanState::Completed).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn uncertain_post_is_never_resubmitted() {
    let server = MockServer::start().await;
    mount_models(&server).await;
    Mock::given(method("POST"))
        .and(path("/task"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({"task_id": "late"}))
                .set_delay(Duration::from_millis(800)),
        )
        .expect(1)
        .mount(&server)
        .await;
    let mut h = start_with(
        &server,
        tempfile::tempdir().unwrap(),
        Duration::from_millis(200),
    );
    h.wait_models().await;
    let id = h.service.analyze("", "Hello", None).unwrap();
    let unknown = h.wait_scan(&id, ScanState::SubmissionUnknown).await;
    assert!(unknown.error.unwrap().contains("not resubmitted"));
    assert_eq!(h.stored(&id).unwrap().state, ScanState::SubmissionUnknown);
    // Resuming without a task ID must not submit again.
    h.service.resume(&id);
    h.wait_for(|e| matches!(e, Event::Notice { .. }).then_some(()))
        .await;
    tokio::time::sleep(Duration::from_millis(900)).await;
    // `expect(1)` is verified when the server drops.
}

#[tokio::test(flavor = "multi_thread")]
async fn stale_poll_response_after_stop_is_ignored() {
    let server = MockServer::start().await;
    mount_models(&server).await;
    mount_submit(&server, "t1", 1).await;
    Mock::given(method("GET"))
        .and(path("/task/t1"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(SUCCESS)
                .set_delay(Duration::from_millis(400)),
        )
        .mount(&server)
        .await;
    let mut h = start(&server);
    h.wait_models().await;
    let id = h.service.analyze("", "Hello world.", None).unwrap();
    h.wait_scan(&id, ScanState::Polling).await;
    tokio::time::sleep(Duration::from_millis(100)).await; // the GET is now in flight
    h.service.stop_polling(&id);
    let paused = h.wait_scan(&id, ScanState::Paused).await;
    assert!(paused.error.unwrap().contains("Polling stopped"));
    h.assert_quiet(
        Duration::from_millis(700),
        |e| matches!(e, Event::Scan(r) if r.state == ScanState::Completed),
    )
    .await;
    assert_eq!(h.stored(&id).unwrap().state, ScanState::Paused);
}

#[tokio::test(flavor = "multi_thread")]
async fn only_one_active_scan() {
    let server = MockServer::start().await;
    mount_models(&server).await;
    mount_submit(&server, "t1", 1).await;
    Mock::given(method("GET"))
        .and(path("/task/t1"))
        .respond_with(pending())
        .mount(&server)
        .await;
    let mut h = start(&server);
    h.wait_models().await;
    let id = h.service.analyze("", "one", None).unwrap();
    assert!(h.service.analyze("", "two", None).is_err());
    h.wait_scan(&id, ScanState::Polling).await;
    assert!(h.service.analyze("", "two", None).is_err());
    h.service.stop_polling(&id);
    h.wait_for(|e| matches!(e, Event::Busy(false)).then_some(()))
        .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn restart_recovers_known_tasks_without_resubmitting() {
    let server = MockServer::start().await;
    mount_models(&server).await;
    mount_submit(&server, "never", 0).await;
    Mock::given(method("GET"))
        .and(path("/task/t9"))
        .respond_with(ResponseTemplate::new(200).set_body_string(SUCCESS))
        .mount(&server)
        .await;

    let dir = tempfile::tempdir().unwrap();
    {
        let db = Db::open(&paths_in(&dir).history_db()).unwrap();
        let base = ScanRecord {
            id: "known".into(),
            created_at: 1,
            updated_at: 1,
            title: "Known".into(),
            input_text: "Hello world.".into(),
            requested_model: "default".into(),
            task_id: Some("t9".into()),
            state: ScanState::Polling,
            error: None,
            returned_text: None,
            returned_version: None,
            prediction_short: None,
            fraction_ai: None,
            raw_response: None,
            source_scan_id: None,
            submit_response: None,
            billed_words: None,
            credits: None,
            cost_usd: None,
            usd_per_credit: None,
        };
        db.upsert(&base).unwrap();
        db.upsert(&ScanRecord {
            id: "unsent".into(),
            task_id: None,
            state: ScanState::Submitting,
            ..base
        })
        .unwrap();
    }
    let mut h = start_with(&server, dir, Duration::from_secs(2));
    let unsent = h.wait_scan("unsent", ScanState::SubmissionUnknown).await;
    assert!(unsent.error.unwrap().contains("not resubmitted"));
    h.wait_scan("known", ScanState::Completed).await;
    assert_eq!(
        h.stored("known").unwrap().returned_text.as_deref(),
        Some("Hello world.")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn disabled_history_keeps_scans_in_memory_only() {
    let server = MockServer::start().await;
    mount_models(&server).await;
    mount_submit(&server, "t1", 1).await;
    Mock::given(method("GET"))
        .and(path("/task/t1"))
        .respond_with(ResponseTemplate::new(200).set_body_string(SUCCESS))
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().unwrap();
    Settings {
        save_history: false,
        ..Default::default()
    }
    .save(&paths_in(&dir).settings_file())
    .unwrap();
    let mut h = start_with(&server, dir, Duration::from_secs(2));
    h.wait_models().await;
    let id = h.service.analyze("", "Hello world.", None).unwrap();
    h.wait_scan(&id, ScanState::Completed).await;
    let history = h
        .wait_for(|e| match e {
            Event::History(hs)
                if hs
                    .items
                    .iter()
                    .any(|i| i.id == id && i.state == ScanState::Completed) =>
            {
                Some(hs.clone())
            }
            _ => None,
        })
        .await;
    assert!(!history.items[0].saved);
    assert_eq!(history.saved_count, 0);
    assert!(h.stored(&id).is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn model_rejection_fails_scan_and_refreshes_catalog() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/models"))
        .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"models":["default"]}"#))
        .expect(2)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/task"))
        .respond_with(
            ResponseTemplate::new(403).set_body_string(r#"{"detail":"Model not enabled"}"#),
        )
        .expect(1)
        .mount(&server)
        .await;
    let mut h = start(&server);
    h.wait_models().await;
    let id = h.service.analyze("", "Hello", None).unwrap();
    let failed = h.wait_scan(&id, ScanState::Failed).await;
    assert!(failed.error.unwrap().contains("Model not enabled"));
    h.wait_models().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn deleting_a_polling_scan_stops_it_and_it_stays_deleted() {
    let server = MockServer::start().await;
    mount_models(&server).await;
    mount_submit(&server, "t1", 1).await;
    Mock::given(method("GET"))
        .and(path("/task/t1"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(SUCCESS)
                .set_delay(Duration::from_millis(300)),
        )
        .mount(&server)
        .await;
    let mut h = start(&server);
    h.wait_models().await;
    let id = h.service.analyze("", "Hello", None).unwrap();
    h.wait_scan(&id, ScanState::Polling).await;
    h.service.delete(&id);
    h.wait_for(|e| matches!(e, Event::Deleted { id: Some(d) } if *d == id).then_some(()))
        .await;
    h.assert_quiet(
        Duration::from_millis(500),
        |e| matches!(e, Event::Scan(r) if r.id == id),
    )
    .await;
    assert!(h.stored(&id).is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn records_charges_usage_and_server_notices() {
    let server = MockServer::start().await;
    mount_models(&server).await;
    Mock::given(method("POST"))
        .and(path("/task"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"{"notice":{"message":"Default is now Pangram 4.0."},"task_id":"t1"}"#,
        ))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/task/t1"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"{"stage":"STAGE_SUCCESS","text":"Hello world.","version":"4.0","windows":[
                {"text":"Hello world.","label":"AI-Generated","start_index":0,"end_index":12,"word_count":172}]}"#,
        ))
        .mount(&server)
        .await;
    let mut h = start(&server);
    h.wait_models().await;
    h.service.set_usd_per_credit(0.04);
    h.wait_for(|e| matches!(e, Event::Settings(s) if s.usd_per_credit == 0.04).then_some(()))
        .await;
    h.service.usage_summary("month");
    let id = h.service.analyze("", "Hello world.", None).unwrap();
    h.wait_for(|e| match e {
        Event::Notice { message, .. } if message.contains("Default is now Pangram 4.0.") => {
            Some(())
        }
        _ => None,
    })
    .await;
    let done = h.wait_scan(&id, ScanState::Completed).await;
    assert_eq!((done.billed_words, done.credits), (Some(172), Some(2)));
    assert!((done.cost_usd.unwrap() - 0.08).abs() < 1e-9);
    assert!(done.submit_response.unwrap().contains("notice"));
    let rows = h
        .wait_for(|e| match e {
            Event::Usage { period, rows } if period == "month" && !rows.is_empty() => {
                Some(rows.clone())
            }
            _ => None,
        })
        .await;
    assert_eq!((rows[0].scans, rows[0].credits), (1, 2));
    // Usage outlives the scan itself.
    h.service.delete(&id);
    h.wait_for(|e| matches!(e, Event::Deleted { .. }).then_some(()))
        .await;
    h.service.usage_summary("day");
    let rows = h
        .wait_for(|e| match e {
            Event::Usage { period, rows } if period == "day" => Some(rows.clone()),
            _ => None,
        })
        .await;
    assert_eq!(rows.len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn deleting_all_history_wins_over_pending_updates() {
    // Reproduces a race where stop_polling had claimed a scan and then wrote it back after
    // delete_all had removed it.
    let server = MockServer::start().await;
    mount_models(&server).await;
    Mock::given(method("POST"))
        .and(path("/task"))
        .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"task_id":"t1"}"#))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/task/t1"))
        .respond_with(pending())
        .mount(&server)
        .await;
    let mut h = start(&server);
    h.wait_models().await;
    for round in 0..25 {
        let id = h.service.analyze("", "Hello", None).unwrap();
        h.wait_scan(&id, ScanState::Polling).await;
        h.service.stop_polling(&id);
        h.service.delete_all();
        h.wait_for(|e| matches!(e, Event::Deleted { id: None }).then_some(()))
            .await;
        // Let any in-flight write finish.
        h.wait_for(|e| matches!(e, Event::Busy(false)).then_some(()))
            .await;
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert!(
            h.stored(&id).is_none(),
            "round {round}: deleted scan was written back"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn rapid_settings_changes_persist_the_latest_value() {
    let server = MockServer::start().await;
    mount_models(&server).await;
    let mut h = start(&server);
    h.wait_models().await;
    for i in 1..=40 {
        h.service.set_usd_per_credit(f64::from(i) / 100.0);
        h.service.set_save_history(i % 2 == 0);
        h.service.set_minimize_to_tray(i % 2 == 1);
    }
    // The last write emits the final settings.
    h.wait_for(|e| match e {
        Event::Settings(s)
            if (s.usd_per_credit - 0.40).abs() < 1e-9 && s.save_history && !s.minimize_to_tray =>
        {
            Some(())
        }
        Event::Notice { message, .. } if message.contains("Couldn't save settings") => {
            panic!("{message}")
        }
        _ => None,
    })
    .await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    let saved = Settings::load(&h.paths.settings_file());
    assert!((saved.usd_per_credit - 0.40).abs() < 1e-9, "{saved:?}");
    assert!(saved.save_history);
    assert!(!saved.minimize_to_tray);
}

#[tokio::test(flavor = "multi_thread")]
async fn failing_to_forget_a_remembered_key_is_reported() {
    let server = MockServer::start().await;
    mount_models(&server).await;
    let mut h = start_custom(
        &server,
        tempfile::tempdir().unwrap(),
        Duration::from_secs(2),
        CredentialStore::Unavailable("keyring locked".into()),
    );
    h.wait_for(|e| matches!(e, Event::Ready).then_some(()))
        .await;
    h.service.set_api_key("new-key", false);
    let status = h
        .wait_for(|e| match e {
            Event::Credentials(c) if c.has_key => Some(c.clone()),
            _ => None,
        })
        .await;
    assert!(!status.persisted);
    let note = status.note.unwrap();
    assert!(
        note.contains("Couldn't remove") && note.contains("keyring locked"),
        "{note}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn cost_uses_the_price_when_the_scan_was_submitted() {
    let server = MockServer::start().await;
    mount_models(&server).await;
    mount_submit(&server, "t1", 1).await;
    Mock::given(method("GET"))
        .and(path("/task/t1"))
        .respond_with(pending())
        .up_to_n_times(3)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/task/t1"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"{"stage":"STAGE_SUCCESS","text":"Hello world.","version":"4.0","windows":[
                {"text":"Hello world.","label":"AI-Generated","start_index":0,"end_index":12,"word_count":250}]}"#,
        ))
        .mount(&server)
        .await;
    let mut h = start(&server);
    h.wait_models().await;
    let id = h.service.analyze("", "Hello world.", None).unwrap();
    let submitted = h.wait_scan(&id, ScanState::Submitting).await;
    assert_eq!(submitted.usd_per_credit, Some(0.05));
    h.wait_scan(&id, ScanState::Polling).await;
    h.service.set_usd_per_credit(1.0);
    let done = h.wait_scan(&id, ScanState::Completed).await;
    assert_eq!(done.credits, Some(3));
    assert!(
        (done.cost_usd.unwrap() - 0.15).abs() < 1e-9,
        "{:?}",
        done.cost_usd
    );
}
