//! `PangramClient` against a local mock server. No real API calls.

use std::time::Duration;

use pangram_core::api::{ApiError, ApiKey, ClientOptions, PangramClient, SubmitError, TaskPoll};
use wiremock::matchers::{body_json, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn client(uri: &str) -> PangramClient {
    PangramClient::new(
        ApiKey::new("test-key").unwrap(),
        ClientOptions {
            base_url: uri.to_owned(),
            connect_timeout: Duration::from_secs(2),
            read_timeout: Duration::from_millis(500),
            submit_timeout: Duration::from_millis(500),
        },
    )
    .unwrap()
}

#[tokio::test]
async fn submit_sends_explicit_model_and_private_flag() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/task"))
        .and(header("x-api-key", "test-key"))
        .and(body_json(serde_json::json!({
            "text": "Hello 😀",
            "model": "pangram-4",
            "public_dashboard_link": false
        })))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(serde_json::json!({"task_id": "abc"})),
        )
        .expect(1)
        .mount(&server)
        .await;
    assert_eq!(
        client(&server.uri())
            .submit("Hello 😀", "pangram-4")
            .await
            .unwrap()
            .task_id,
        "abc"
    );
}

#[tokio::test]
async fn submit_errors_are_classified() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(402))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(504))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"status": "ok"})))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    let c = client(&server.uri());
    assert_eq!(
        c.submit("x", "default").await,
        Err(SubmitError::Rejected(ApiError::InsufficientCredits))
    );
    assert!(matches!(
        c.submit("x", "default").await,
        Err(SubmitError::OutcomeUnknown(_))
    ));
    assert!(matches!(
        c.submit("x", "default").await,
        Err(SubmitError::OutcomeUnknown(_))
    ));
}

#[tokio::test]
async fn submit_timeout_is_an_unknown_outcome() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({"task_id": "late"}))
                .set_delay(Duration::from_secs(2)),
        )
        .expect(1)
        .mount(&server)
        .await;
    assert!(matches!(
        client(&server.uri()).submit("x", "default").await,
        Err(SubmitError::OutcomeUnknown(_))
    ));
}

#[tokio::test]
async fn refused_connection_is_a_definite_failure() {
    let uri = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        format!("http://{}", listener.local_addr().unwrap())
    };
    assert!(matches!(
        client(&uri).submit("x", "default").await,
        Err(SubmitError::Rejected(ApiError::Network(_)))
    ));
}

#[tokio::test]
async fn polling_and_catalog_errors() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/task/t%2F1"))
        .respond_with(ResponseTemplate::new(429).insert_header("Retry-After", "3"))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/task/t2"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(r#"{"task_id":"t2","stage":"STAGE_PREPROCESSING"}"#),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/models"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&server)
        .await;
    let c = client(&server.uri());
    // Task IDs are path-encoded.
    assert_eq!(
        c.get_task("t/1").await,
        Err(ApiError::RateLimited {
            retry_after: Some(Duration::from_secs(3))
        })
    );
    assert_eq!(
        c.get_task("t2").await,
        Ok(TaskPoll::Pending {
            stage: Some("STAGE_PREPROCESSING".into())
        })
    );
    assert_eq!(c.list_models().await, Err(ApiError::Unauthorized));
}
