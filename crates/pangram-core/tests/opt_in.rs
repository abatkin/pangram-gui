//! Opt-in checks against real external services. All are `#[ignore]`d so routine test runs never
//! touch the keyring or spend API credits.
//!
//! ```sh
//! # Read-only Secret Service lookup of a test item:
//! cargo test -p pangram-core --test opt_in secret_service_lookup -- --ignored
//! # Writes, reads back and deletes a test item (not the app's key):
//! cargo test -p pangram-core --test opt_in secret_service_round_trip -- --ignored
//! # Submits ONE short scan to the real API (spends credits). Uses PANGRAM_API_KEY, or else the
//! # key the desktop app stored in the keyring:
//! PANGRAM_LIVE_TEST=1 cargo test -p pangram-core --test opt_in live_api -- --ignored --nocapture
//! ```

use std::time::{Duration, Instant};

use pangram_core::analysis::Analysis;
use pangram_core::api::{ApiKey, ClientOptions, PangramClient, TaskPoll, choose_model};
use pangram_core::credentials::CredentialStore;

fn test_store() -> CredentialStore {
    CredentialStore::SecretService {
        kind: "pangram-api-key-selftest".to_owned(),
    }
}

#[tokio::test]
#[ignore = "talks to the session Secret Service"]
async fn secret_service_lookup() {
    assert!(
        test_store()
            .load()
            .await
            .expect("Secret Service reachable")
            .is_none()
    );
}

#[tokio::test]
#[ignore = "writes a test item to the session keyring"]
async fn secret_service_round_trip() {
    let store = test_store();
    let key = ApiKey::new("selftest-not-a-real-key").unwrap();
    store.save(&key).await.unwrap();
    assert_eq!(store.load().await.unwrap(), Some(key));
    store.delete().await.unwrap();
    assert_eq!(store.load().await.unwrap(), None);
}

/// Resolves the open API questions: index units for non-ASCII text, whether input is normalised,
/// and that a mixed-script text validates end to end.
#[tokio::test]
#[ignore = "spends Pangram API credits"]
async fn live_api() {
    if std::env::var("PANGRAM_LIVE_TEST").as_deref() != Ok("1") {
        eprintln!("set PANGRAM_LIVE_TEST=1 to run");
        return;
    }
    let key = match std::env::var("PANGRAM_API_KEY") {
        Ok(k) => ApiKey::new(&k).expect("PANGRAM_API_KEY is blank"),
        Err(_) => CredentialStore::secret_service()
            .load()
            .await
            .expect("keyring")
            .expect("set PANGRAM_API_KEY or save a key in the app"),
    };
    let client = PangramClient::new(key, ClientOptions::default()).unwrap();
    let models = client.list_models().await.expect("list models");
    eprintln!("models: {models:?}");
    let model = choose_model(&models, None).expect("a model");

    // Non-BMP characters (emoji), combining marks, CJK, curly quotes and CRLF line endings.
    let text = "The committee met on Tuesday to review the “annual” budget 📊 and staffing plans.\r\n\
        Several members—including Zoë and José—raised concerns about the timeline.\r\n\r\n\
        会議は火曜日に行われました。 The final vote is expected next month, pending further review 🗳️.";
    let task_id = client.submit(text, &model).await.expect("submit").task_id;
    eprintln!("task: {task_id}");
    let deadline = Instant::now() + Duration::from_secs(180);
    let result = loop {
        assert!(Instant::now() < deadline, "task did not finish in time");
        match client.get_task(&task_id).await.expect("poll") {
            TaskPoll::Pending { stage } => {
                eprintln!("stage: {stage:?}");
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
            TaskPoll::Failed { message, .. } => panic!("task failed: {message}"),
            TaskPoll::Succeeded { result, .. } => break result,
        }
    };
    let analysis = Analysis::from_result(&result);
    eprintln!("version: {:?}", result.version);
    eprintln!("returned text identical to input: {}", result.text == text);
    eprintln!("windows: {}", result.windows.len());
    eprintln!("index unit: {:?}", analysis.index_unit);
    eprintln!("highlights valid: {}", analysis.highlights_valid);
    assert!(analysis.highlights_valid, "window ranges did not validate");
    // Observed 2026-10-09: offsets count Unicode code points.
    assert_eq!(
        analysis.index_unit,
        Some(pangram_core::analysis::IndexUnit::CodePoints)
    );
}
