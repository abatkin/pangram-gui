//! Pangram REST client: authentication, transport, polling responses and typed errors.
//!
//! Nothing in this module logs request or response bodies, or the API key.

use std::fmt;
use std::time::Duration;

use reqwest::header::{HeaderMap, HeaderValue, RETRY_AFTER};
use reqwest::{Client, StatusCode, Url};
use serde_json::{Map, Value};

pub const DEFAULT_BASE_URL: &str = "https://text.external-api.pangram.com";
pub const STAGE_SUCCESS: &str = "STAGE_SUCCESS";
pub const STAGE_FAILED: &str = "STAGE_FAILED";

/// An API key. `Debug` never prints the value.
#[derive(Clone, PartialEq, Eq)]
pub struct ApiKey(String);

impl ApiKey {
    /// Returns `None` for blank input.
    pub fn new(key: &str) -> Option<Self> {
        let key = key.trim();
        (!key.is_empty()).then(|| Self(key.to_owned()))
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for ApiKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ApiKey(<redacted>)")
    }
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ApiError {
    #[error("Pangram rejected the API key. Check the key in Settings.")]
    Unauthorized,
    #[error("Your Pangram account doesn't have enough credits for this scan.")]
    InsufficientCredits,
    #[error("Access denied: {0}")]
    Forbidden(String),
    #[error("Pangram has no record of this task (it may have expired).")]
    NotFound,
    #[error("Pangram rejected the request: {0}")]
    InvalidRequest(String),
    #[error("Pangram's rate limit was reached.{}", retry_hint(*.retry_after))]
    RateLimited { retry_after: Option<Duration> },
    #[error("The selected model is temporarily unavailable.{}", retry_hint(*.retry_after))]
    ModelUnavailable { retry_after: Option<Duration> },
    #[error("Pangram service error (HTTP {status}).{}", detail_suffix(.detail))]
    Server {
        status: u16,
        detail: String,
        retry_after: Option<Duration>,
    },
    #[error("Network error: {0}")]
    Network(String),
    #[error("Unexpected response from Pangram: {0}")]
    Decode(String),
}

fn retry_hint(retry_after: Option<Duration>) -> String {
    match retry_after {
        Some(d) => format!(" Try again in {} s.", d.as_secs().max(1)),
        None => " Try again shortly.".to_owned(),
    }
}

fn detail_suffix(detail: &str) -> String {
    if detail.is_empty() {
        String::new()
    } else {
        format!(" {detail}")
    }
}

impl ApiError {
    /// Server-suggested delay before retrying.
    pub fn retry_after(&self) -> Option<Duration> {
        match self {
            Self::RateLimited { retry_after }
            | Self::ModelUnavailable { retry_after }
            | Self::Server { retry_after, .. } => *retry_after,
            _ => None,
        }
    }

    /// Whether repeating the same safe (GET) request may succeed.
    pub fn is_transient(&self) -> bool {
        matches!(
            self,
            Self::RateLimited { .. }
                | Self::ModelUnavailable { .. }
                | Self::Server { .. }
                | Self::Network(_)
                | Self::Decode(_)
        )
    }

    /// Whether the error suggests the model catalog is stale.
    pub fn suggests_model_refresh(&self) -> bool {
        matches!(
            self,
            Self::Forbidden(_) | Self::InvalidRequest(_) | Self::ModelUnavailable { .. }
        )
    }
}

/// Outcome of a failed `POST /task`.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum SubmitError {
    /// Pangram definitely did not create a task (or explicitly refused it).
    #[error("{0}")]
    Rejected(ApiError),
    /// The request may have reached Pangram, but no task ID came back. Never resubmit automatically.
    #[error("Pangram may or may not have received this scan ({0}).")]
    OutcomeUnknown(String),
}

/// Parsed `GET /task/{id}` response.
#[derive(Debug, Clone, PartialEq)]
pub enum TaskPoll {
    Pending {
        stage: Option<String>,
    },
    Succeeded {
        raw: String,
        result: Box<DetectionResult>,
    },
    Failed {
        raw: String,
        message: String,
    },
}

/// A completed detection result. Every field is optional except text and windows, because the
/// schema varies across model generations; the raw JSON is kept separately for history.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DetectionResult {
    pub text: String,
    pub version: Option<String>,
    pub headline: Option<String>,
    pub prediction: Option<String>,
    pub prediction_short: Option<String>,
    pub fraction_ai: Option<f64>,
    pub fraction_ai_assisted: Option<f64>,
    pub fraction_human: Option<f64>,
    pub num_ai_segments: Option<i64>,
    pub num_ai_assisted_segments: Option<i64>,
    pub num_human_segments: Option<i64>,
    pub windows: Vec<Window>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Window {
    pub text: Option<String>,
    pub label: Option<String>,
    pub ai_assistance_score: Option<f64>,
    pub confidence: Option<String>,
    pub start_index: Option<i64>,
    pub end_index: Option<i64>,
    pub word_count: Option<i64>,
    pub token_length: Option<i64>,
    pub is_humanized: Option<bool>,
    pub humanizer_score: Option<f64>,
    /// Fields this client does not know about, shown verbatim as model-specific details.
    pub extra: Vec<(String, String)>,
}

const KNOWN_WINDOW_FIELDS: &[&str] = &[
    "text",
    "label",
    "ai_assistance_score",
    "confidence",
    "start_index",
    "end_index",
    "word_count",
    "token_length",
    "is_humanized",
    "humanizer_score",
];

fn get_str(obj: &Map<String, Value>, key: &str) -> Option<String> {
    match obj.get(key)? {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

fn get_f64(obj: &Map<String, Value>, key: &str) -> Option<f64> {
    match obj.get(key)? {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

fn get_i64(obj: &Map<String, Value>, key: &str) -> Option<i64> {
    match obj.get(key)? {
        Value::Number(n) => n.as_i64().or_else(|| {
            n.as_f64()
                .filter(|f| f.fract() == 0.0 && f.is_finite())
                .map(|f| f as i64)
        }),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

fn get_bool(obj: &Map<String, Value>, key: &str) -> Option<bool> {
    match obj.get(key)? {
        Value::Bool(b) => Some(*b),
        Value::String(s) => match s.to_ascii_lowercase().as_str() {
            "true" => Some(true),
            "false" => Some(false),
            _ => None,
        },
        _ => None,
    }
}

fn display_value(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

impl Window {
    fn from_json(obj: &Map<String, Value>) -> Self {
        let mut extra: Vec<(String, String)> = obj
            .iter()
            .filter(|(k, v)| !KNOWN_WINDOW_FIELDS.contains(&k.as_str()) && !v.is_null())
            .map(|(k, v)| (k.clone(), display_value(v)))
            .collect();
        extra.sort();
        Self {
            text: get_str(obj, "text"),
            label: get_str(obj, "label"),
            ai_assistance_score: get_f64(obj, "ai_assistance_score"),
            confidence: get_str(obj, "confidence"),
            start_index: get_i64(obj, "start_index"),
            end_index: get_i64(obj, "end_index"),
            word_count: get_i64(obj, "word_count"),
            token_length: get_i64(obj, "token_length"),
            is_humanized: get_bool(obj, "is_humanized"),
            humanizer_score: get_f64(obj, "humanizer_score"),
            extra,
        }
    }
}

impl DetectionResult {
    pub fn from_json(value: &Value) -> Result<Self, String> {
        let obj = value
            .as_object()
            .ok_or_else(|| "result is not a JSON object".to_owned())?;
        let windows = match obj.get("windows") {
            None | Some(Value::Null) => Vec::new(),
            Some(Value::Array(items)) => items
                .iter()
                .filter_map(Value::as_object)
                .map(Window::from_json)
                .collect(),
            Some(_) => return Err("`windows` is not an array".to_owned()),
        };
        Ok(Self {
            text: get_str(obj, "text").unwrap_or_default(),
            version: get_str(obj, "version").filter(|s| !s.is_empty()),
            headline: get_str(obj, "headline").filter(|s| !s.is_empty()),
            prediction: get_str(obj, "prediction").filter(|s| !s.is_empty()),
            prediction_short: get_str(obj, "prediction_short").filter(|s| !s.is_empty()),
            fraction_ai: get_f64(obj, "fraction_ai"),
            fraction_ai_assisted: get_f64(obj, "fraction_ai_assisted"),
            fraction_human: get_f64(obj, "fraction_human"),
            num_ai_segments: get_i64(obj, "num_ai_segments"),
            num_ai_assisted_segments: get_i64(obj, "num_ai_assisted_segments"),
            num_human_segments: get_i64(obj, "num_human_segments"),
            windows,
        })
    }

    pub fn from_json_str(raw: &str) -> Result<Self, String> {
        let value: Value = serde_json::from_str(raw).map_err(|e| e.to_string())?;
        Self::from_json(&value)
    }
}

/// Parses a `GET /task/{id}` body.
pub fn parse_task_poll(raw: &str) -> Result<TaskPoll, String> {
    let value: Value = serde_json::from_str(raw).map_err(|e| format!("invalid JSON ({e})"))?;
    let obj = value
        .as_object()
        .ok_or_else(|| "response is not a JSON object".to_owned())?;
    let stage = get_str(obj, "stage");
    match stage.as_deref() {
        Some(STAGE_SUCCESS) => Ok(TaskPoll::Succeeded {
            raw: raw.to_owned(),
            result: Box::new(DetectionResult::from_json(&value)?),
        }),
        Some(STAGE_FAILED) => {
            let message = get_str(obj, "headline")
                .or_else(|| get_str(obj, "prediction"))
                .or_else(|| get_str(obj, "error"))
                .filter(|s| !s.trim().is_empty())
                .unwrap_or_else(|| "Pangram could not analyze this text.".to_owned());
            Ok(TaskPoll::Failed {
                raw: raw.to_owned(),
                message,
            })
        }
        _ => Ok(TaskPoll::Pending { stage }),
    }
}

pub fn parse_models(raw: &str) -> Result<Vec<String>, String> {
    let value: Value = serde_json::from_str(raw).map_err(|e| format!("invalid JSON ({e})"))?;
    let models = value
        .get("models")
        .and_then(Value::as_array)
        .ok_or_else(|| "missing `models` array".to_owned())?;
    let mut out: Vec<String> = Vec::with_capacity(models.len());
    for model in models.iter().filter_map(Value::as_str) {
        if !model.is_empty() && !out.iter().any(|m| m == model) {
            out.push(model.to_owned());
        }
    }
    Ok(out)
}

/// Picks the model to use: the saved one if still offered, else `default`, else the first.
pub fn choose_model(catalog: &[String], saved: Option<&str>) -> Option<String> {
    saved
        .and_then(|s| catalog.iter().find(|m| *m == s))
        .or_else(|| catalog.iter().find(|m| *m == "default"))
        .or_else(|| catalog.first())
        .cloned()
}

/// The server's requested delay, unmodified (delta-seconds form).
fn parse_retry_after(headers: &HeaderMap) -> Option<Duration> {
    let value = headers.get(RETRY_AFTER)?.to_str().ok()?.trim();
    value.parse::<u64>().ok().map(Duration::from_secs)
}

/// Extracts a short human-readable message from an error body without echoing large bodies.
fn error_detail(body: &str) -> String {
    let from_json = serde_json::from_str::<Value>(body).ok().and_then(|v| {
        ["detail", "message", "error", "headline"]
            .iter()
            .find_map(|k| match v.get(*k)? {
                Value::String(s) => Some(s.clone()),
                Value::Null => None,
                other => Some(other.to_string()),
            })
    });
    let text = from_json.unwrap_or_else(|| {
        if body.trim_start().starts_with('<') {
            String::new()
        } else {
            body.to_owned()
        }
    });
    let text = text.trim();
    const MAX: usize = 240;
    if text.chars().count() > MAX {
        let cut: String = text.chars().take(MAX).collect();
        format!("{cut}…")
    } else {
        text.to_owned()
    }
}

fn error_for_status(status: StatusCode, headers: &HeaderMap, body: &str) -> ApiError {
    let detail = error_detail(body);
    let retry_after = parse_retry_after(headers);
    let or = |fallback: &str| {
        if detail.is_empty() {
            fallback.to_owned()
        } else {
            detail.clone()
        }
    };
    match status.as_u16() {
        401 => ApiError::Unauthorized,
        402 => ApiError::InsufficientCredits,
        403 => ApiError::Forbidden(or(
            "the model is not enabled for this key, or the key does not own this task",
        )),
        404 => ApiError::NotFound,
        400 | 413 | 422 => ApiError::InvalidRequest(or(match status.as_u16() {
            413 => "the request is too large",
            422 => "the text or model selector is invalid",
            _ => "the request was malformed",
        })),
        429 => ApiError::RateLimited { retry_after },
        503 => ApiError::ModelUnavailable { retry_after },
        code => ApiError::Server {
            status: code,
            detail,
            retry_after,
        },
    }
}

fn network_error(err: &reqwest::Error) -> ApiError {
    let kind = if err.is_timeout() {
        "the request timed out"
    } else if err.is_connect() {
        "could not connect to Pangram"
    } else if err.is_decode() || err.is_body() {
        "the response was interrupted"
    } else {
        "the connection failed"
    };
    ApiError::Network(kind.to_owned())
}

/// A successful `POST /task`.
#[derive(Debug, Clone, PartialEq)]
pub struct Submission {
    pub task_id: String,
    /// Informational `notice.message` from the server, if any.
    pub notice: Option<String>,
    pub raw: String,
}

/// The exact JSON body sent to `POST /task`.
pub fn task_request_body(text: &str, model: &str) -> Value {
    serde_json::json!({
        "text": text,
        "model": model,
        "public_dashboard_link": false,
    })
}

fn parse_submission(raw: &str) -> Result<Submission, String> {
    let value: Value =
        serde_json::from_str(raw).map_err(|_| "the response could not be read".to_owned())?;
    let task_id = match value.get("task_id") {
        Some(Value::String(s)) if !s.is_empty() => s.clone(),
        Some(Value::Number(n)) => n.to_string(),
        _ => return Err("the response contained no task ID".to_owned()),
    };
    let notice = match value.get("notice") {
        Some(Value::String(s)) => Some(s.clone()),
        Some(Value::Object(o)) => o.get("message").and_then(Value::as_str).map(str::to_owned),
        _ => None,
    }
    .filter(|s| !s.trim().is_empty());
    Ok(Submission {
        task_id,
        notice,
        raw: raw.to_owned(),
    })
}

#[derive(Debug, Clone)]
pub struct ClientOptions {
    pub base_url: String,
    pub connect_timeout: Duration,
    /// Timeout for catalog and polling GETs.
    pub read_timeout: Duration,
    /// Timeout for submissions, which upload the whole document.
    pub submit_timeout: Duration,
}

impl Default for ClientOptions {
    fn default() -> Self {
        Self {
            base_url: DEFAULT_BASE_URL.to_owned(),
            connect_timeout: Duration::from_secs(15),
            read_timeout: Duration::from_secs(30),
            submit_timeout: Duration::from_secs(120),
        }
    }
}

#[derive(Debug, Clone)]
pub struct PangramClient {
    http: Client,
    base: Url,
    key: ApiKey,
    options: ClientOptions,
}

impl PangramClient {
    pub fn new(key: ApiKey, options: ClientOptions) -> Result<Self, ApiError> {
        let mut base = Url::parse(&options.base_url)
            .map_err(|e| ApiError::InvalidRequest(format!("invalid API base URL: {e}")))?;
        if base.cannot_be_a_base() {
            return Err(ApiError::InvalidRequest("invalid API base URL".to_owned()));
        }
        // Keep a trailing slash out of the stored path so segments append cleanly.
        if base.path().ends_with('/') && base.path() != "/" {
            let trimmed = base.path().trim_end_matches('/').to_owned();
            base.set_path(&trimmed);
        }
        let http = Client::builder()
            .connect_timeout(options.connect_timeout)
            .user_agent(concat!("pangram-desktop/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| ApiError::Network(format!("could not initialise HTTP client ({e})")))?;
        Ok(Self {
            http,
            base,
            key,
            options,
        })
    }

    fn url(&self, segments: &[&str]) -> Url {
        let mut url = self.base.clone();
        url.path_segments_mut()
            .expect("base URL validated in new()")
            .pop_if_empty()
            .extend(segments);
        url
    }

    fn auth_headers(&self) -> Result<HeaderMap, ApiError> {
        let mut headers = HeaderMap::new();
        let mut value = HeaderValue::from_str(self.key.expose()).map_err(|_| {
            ApiError::InvalidRequest("the API key contains invalid characters".to_owned())
        })?;
        value.set_sensitive(true);
        headers.insert("x-api-key", value);
        Ok(headers)
    }

    async fn get(&self, url: Url) -> Result<String, ApiError> {
        let response = self
            .http
            .get(url)
            .headers(self.auth_headers()?)
            .timeout(self.options.read_timeout)
            .send()
            .await
            .map_err(|e| network_error(&e))?;
        let status = response.status();
        let headers = response.headers().clone();
        let body = response.text().await.map_err(|e| network_error(&e))?;
        if status.is_success() {
            Ok(body)
        } else {
            Err(error_for_status(status, &headers, &body))
        }
    }

    pub async fn list_models(&self) -> Result<Vec<String>, ApiError> {
        let body = self.get(self.url(&["models"])).await?;
        parse_models(&body).map_err(ApiError::Decode)
    }

    /// Submits text for analysis. Returns the task ID, any server notice and the raw body.
    pub async fn submit(&self, text: &str, model: &str) -> Result<Submission, SubmitError> {
        let headers = self.auth_headers().map_err(SubmitError::Rejected)?;
        let response = match self
            .http
            .post(self.url(&["task"]))
            .headers(headers)
            .timeout(self.options.submit_timeout)
            .json(&task_request_body(text, model))
            .send()
            .await
        {
            Ok(r) => r,
            // A failed connection means nothing was sent; anything later is ambiguous.
            Err(e) if e.is_connect() => return Err(SubmitError::Rejected(network_error(&e))),
            Err(e) if e.is_builder() => return Err(SubmitError::Rejected(network_error(&e))),
            Err(e) => return Err(SubmitError::OutcomeUnknown(network_error(&e).to_string())),
        };
        let status = response.status();
        let headers = response.headers().clone();
        let text = match response.text().await {
            Ok(t) => t,
            Err(e) if status.is_success() => {
                return Err(SubmitError::OutcomeUnknown(network_error(&e).to_string()));
            }
            Err(_) => String::new(),
        };
        if status.is_success() {
            return parse_submission(&text).map_err(SubmitError::OutcomeUnknown);
        }
        // Gateway failures do not tell us whether the backend accepted the task.
        if matches!(status.as_u16(), 502 | 504) {
            return Err(SubmitError::OutcomeUnknown(format!(
                "HTTP {} from a gateway",
                status.as_u16()
            )));
        }
        Err(SubmitError::Rejected(error_for_status(
            status, &headers, &text,
        )))
    }

    pub async fn get_task(&self, task_id: &str) -> Result<TaskPoll, ApiError> {
        let body = self.get(self.url(&["task", task_id])).await?;
        parse_task_poll(&body).map_err(ApiError::Decode)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_submission_notice() {
        let raw = r#"{"notice":{"message":"Default is now Pangram 4.0."},"task_id":"abc"}"#;
        let s = parse_submission(raw).unwrap();
        assert_eq!(s.task_id, "abc");
        assert_eq!(s.notice.as_deref(), Some("Default is now Pangram 4.0."));
        assert_eq!(s.raw, raw);
        assert_eq!(
            parse_submission(r#"{"task_id":"x","notice":"plain"}"#)
                .unwrap()
                .notice
                .as_deref(),
            Some("plain")
        );
        assert_eq!(parse_submission(r#"{"task_id":"x"}"#).unwrap().notice, None);
        assert!(parse_submission(r#"{"status":"ok"}"#).is_err());
    }

    #[test]
    fn api_key_is_redacted_and_trimmed() {
        let key = ApiKey::new("  secret-key \n").unwrap();
        assert_eq!(key.expose(), "secret-key");
        assert!(!format!("{key:?}").contains("secret"));
        assert!(ApiKey::new("   ").is_none());
    }

    #[test]
    fn parses_model_catalog_preserving_order() {
        let models = parse_models(r#"{"models": ["pangram-4", "default", "pangram-4"]}"#).unwrap();
        assert_eq!(models, vec!["pangram-4", "default"]);
        assert!(parse_models(r#"{"other": []}"#).is_err());
    }

    #[test]
    fn model_choice_prefers_saved_then_default_then_first() {
        let catalog: Vec<String> = ["a", "default", "b"].map(String::from).to_vec();
        assert_eq!(choose_model(&catalog, Some("b")).as_deref(), Some("b"));
        assert_eq!(
            choose_model(&catalog, Some("gone")).as_deref(),
            Some("default")
        );
        assert_eq!(choose_model(&catalog, None).as_deref(), Some("default"));
        let catalog: Vec<String> = ["x", "y"].map(String::from).to_vec();
        assert_eq!(choose_model(&catalog, None).as_deref(), Some("x"));
        assert_eq!(choose_model(&[], None), None);
    }

    const SUCCESS: &str = r#"{
        "text": "Hello world. This is a test.",
        "version": "4.0",
        "headline": "AI Detected",
        "prediction": "We are confident this document is AI-generated.",
        "prediction_short": "AI",
        "fraction_ai": 0.5,
        "fraction_ai_assisted": 0.25,
        "fraction_human": 0.25,
        "num_ai_segments": 1,
        "num_ai_assisted_segments": 0,
        "num_human_segments": 1,
        "stage": "STAGE_SUCCESS",
        "windows": [
            {"text": "Hello world.", "label": "AI-Generated", "ai_assistance_score": 0.97,
             "confidence": "High", "start_index": 0, "end_index": 12, "word_count": 2,
             "token_length": 3, "is_humanized": false, "humanizer_score": 0.01,
             "future_field": {"x": 1}},
            {"text": "This is a test.", "label": "Human Written", "confidence": "Medium",
             "start_index": 13, "end_index": 28}
        ]
    }"#;

    #[test]
    fn parses_success_with_optional_and_unknown_fields() {
        let TaskPoll::Succeeded { result, raw } = parse_task_poll(SUCCESS).unwrap() else {
            panic!("expected success");
        };
        assert_eq!(raw, SUCCESS);
        assert_eq!(result.version.as_deref(), Some("4.0"));
        assert_eq!(result.prediction_short.as_deref(), Some("AI"));
        assert_eq!(result.fraction_ai_assisted, Some(0.25));
        assert_eq!(result.windows.len(), 2);
        let w0 = &result.windows[0];
        assert_eq!(w0.is_humanized, Some(false));
        assert_eq!(
            w0.extra,
            vec![("future_field".to_owned(), r#"{"x":1}"#.to_owned())]
        );
        let w1 = &result.windows[1];
        assert_eq!(w1.humanizer_score, None);
        assert_eq!(w1.ai_assistance_score, None);
        assert_eq!((w1.start_index, w1.end_index), (Some(13), Some(28)));
    }

    #[test]
    fn tolerates_missing_fields_and_odd_types() {
        let raw = r#"{"stage":"STAGE_SUCCESS","text":"abc","fraction_ai":"0.3","windows":[{"start_index":0.0,"end_index":"3","is_humanized":"true"}]}"#;
        let TaskPoll::Succeeded { result, .. } = parse_task_poll(raw).unwrap() else {
            panic!()
        };
        assert_eq!(result.fraction_ai, Some(0.3));
        assert_eq!(result.version, None);
        assert_eq!(result.windows[0].start_index, Some(0));
        assert_eq!(result.windows[0].end_index, Some(3));
        assert_eq!(result.windows[0].is_humanized, Some(true));
        assert_eq!(result.windows[0].text, None);
    }

    #[test]
    fn parses_pending_and_failed_stages() {
        assert_eq!(
            parse_task_poll(r#"{"task_id":"t","stage":"STAGE_PREPROCESSING"}"#).unwrap(),
            TaskPoll::Pending {
                stage: Some("STAGE_PREPROCESSING".into())
            }
        );
        assert_eq!(
            parse_task_poll(r#"{"task_id":"t"}"#).unwrap(),
            TaskPoll::Pending { stage: None }
        );
        let failed = r#"{"stage":"STAGE_FAILED","text":"","headline":"Preprocessing error: no valid text","windows":[],"fraction_ai":0}"#;
        match parse_task_poll(failed).unwrap() {
            TaskPoll::Failed { message, .. } => {
                assert_eq!(message, "Preprocessing error: no valid text")
            }
            other => panic!("{other:?}"),
        }
        assert!(parse_task_poll("not json").is_err());
    }

    #[test]
    fn maps_statuses_to_errors() {
        let mut headers = HeaderMap::new();
        headers.insert(RETRY_AFTER, HeaderValue::from_static("7"));
        assert_eq!(
            error_for_status(StatusCode::TOO_MANY_REQUESTS, &headers, ""),
            ApiError::RateLimited {
                retry_after: Some(Duration::from_secs(7))
            }
        );
        let none = HeaderMap::new();
        let mut long = HeaderMap::new();
        long.insert(RETRY_AFTER, HeaderValue::from_static("7200"));
        assert_eq!(
            error_for_status(StatusCode::TOO_MANY_REQUESTS, &long, "").retry_after(),
            Some(Duration::from_secs(7200)),
            "long server delays are not shortened"
        );
        assert_eq!(
            error_for_status(StatusCode::UNAUTHORIZED, &none, ""),
            ApiError::Unauthorized
        );
        assert_eq!(
            error_for_status(StatusCode::PAYMENT_REQUIRED, &none, ""),
            ApiError::InsufficientCredits
        );
        assert_eq!(
            error_for_status(
                StatusCode::UNPROCESSABLE_ENTITY,
                &none,
                r#"{"detail":"Unknown model"}"#
            ),
            ApiError::InvalidRequest("Unknown model".into())
        );
        assert!(matches!(
            error_for_status(StatusCode::SERVICE_UNAVAILABLE, &none, ""),
            ApiError::ModelUnavailable { retry_after: None }
        ));
        let long_html = format!("<html>{}</html>", "x".repeat(5000));
        match error_for_status(StatusCode::INTERNAL_SERVER_ERROR, &none, &long_html) {
            ApiError::Server { status, detail, .. } => {
                assert_eq!(status, 500);
                assert!(detail.is_empty());
            }
            other => panic!("{other:?}"),
        }
        assert!(error_detail(&"y".repeat(5000)).chars().count() <= 241);
    }
}
