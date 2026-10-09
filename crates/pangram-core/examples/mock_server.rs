//! A local stand-in for the Pangram API, for UI development without spending credits.
//!
//! ```sh
//! cargo run -p pangram-core --example mock_server            # prints its URL
//! PANGRAM_API_BASE=http://127.0.0.1:8765 cargo run -p pangram-desktop
//! ```
//!
//! Environment: `MOCK_PORT` (default 8765), `MOCK_DELAY_MS` (time before a task completes,
//! default 3000). Any API key works except `bad` (401). Text containing `FAIL` fails the task;
//! text containing `NOCREDIT` gets HTTP 402. Curly quotes are normalised to straight quotes in
//! the returned text, and windows use code-point offsets (as the real API does).

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use wiremock::matchers::{method, path, path_regex};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

#[derive(Default)]
struct Tasks {
    next: AtomicU64,
    tasks: Mutex<HashMap<String, (Instant, String)>>,
}

fn unauthorized(req: &Request) -> bool {
    req.headers
        .get("x-api-key")
        .and_then(|v| v.to_str().ok())
        .is_none_or(|k| k == "bad")
}

struct Models;
impl Respond for Models {
    fn respond(&self, req: &Request) -> ResponseTemplate {
        if unauthorized(req) {
            return ResponseTemplate::new(401).set_body_json(json!({"detail": "Invalid API key"}));
        }
        ResponseTemplate::new(200).set_body_json(json!({"models": ["default", "pangram-4"]}))
    }
}

struct Submit(Arc<Tasks>);
impl Respond for Submit {
    fn respond(&self, req: &Request) -> ResponseTemplate {
        if unauthorized(req) {
            return ResponseTemplate::new(401).set_body_json(json!({"detail": "Invalid API key"}));
        }
        let body: Value = serde_json::from_slice(&req.body).unwrap_or_default();
        let text = body["text"].as_str().unwrap_or_default().to_owned();
        if text.contains("NOCREDIT") {
            return ResponseTemplate::new(402)
                .set_body_json(json!({"detail": "Insufficient credits"}));
        }
        let id = format!("mock-{}", self.0.next.fetch_add(1, Ordering::SeqCst) + 1);
        self.0
            .tasks
            .lock()
            .unwrap()
            .insert(id.clone(), (Instant::now(), text));
        ResponseTemplate::new(200).set_body_json(json!({
            "notice": {"message": "This is the mock Pangram API; results are synthetic."},
            "task_id": id
        }))
    }
}

struct Poll(Arc<Tasks>, Duration);
impl Respond for Poll {
    fn respond(&self, req: &Request) -> ResponseTemplate {
        if unauthorized(req) {
            return ResponseTemplate::new(401);
        }
        let id = req
            .url
            .path()
            .rsplit('/')
            .next()
            .unwrap_or_default()
            .to_owned();
        let Some((started, text)) = self.0.tasks.lock().unwrap().get(&id).cloned() else {
            return ResponseTemplate::new(404).set_body_json(json!({"detail": "Task not found"}));
        };
        if started.elapsed() < self.1 {
            let stage = if started.elapsed() < self.1 / 2 {
                "STAGE_PREPROCESSING"
            } else {
                "STAGE_POSTPROCESSING"
            };
            return ResponseTemplate::new(200)
                .set_body_json(json!({"task_id": id, "stage": stage}));
        }
        if text.contains("FAIL") {
            return ResponseTemplate::new(200).set_body_json(json!({
                "stage": "STAGE_FAILED", "text": "", "version": "", "headline": "Preprocessing error: mock failure",
                "prediction": "", "prediction_short": "", "fraction_ai": 0.0, "fraction_ai_assisted": 0.0,
                "fraction_human": 0.0, "windows": []
            }));
        }
        ResponseTemplate::new(200).set_body_json(analyze(&text))
    }
}

/// Splits text into sentence-ish windows and labels them deterministically.
fn analyze(input: &str) -> Value {
    let text: String = input
        .chars()
        .map(|c| match c {
            '\u{201C}' | '\u{201D}' => '"',
            '\u{2018}' | '\u{2019}' => '\'',
            c => c,
        })
        .collect();
    let chars: Vec<char> = text.chars().collect();
    let mut bounds = Vec::new();
    let mut start = 0;
    let mut sentences = 0;
    for i in 0..chars.len() {
        let end_of_sentence = matches!(chars[i], '.' | '!' | '?' | '。')
            && chars.get(i + 1).is_none_or(|c| c.is_whitespace());
        let paragraph = chars[i] == '\n' && chars.get(i + 1) == Some(&'\n');
        if end_of_sentence {
            sentences += 1;
        }
        if (end_of_sentence && sentences % 2 == 0) || paragraph || i + 1 == chars.len() {
            bounds.push((start, i + 1));
            start = i + 1;
        }
    }
    let labels = [
        ("AI-Generated", "High", 0.96),
        ("Human Written", "High", 0.04),
        ("AI-Assisted", "Medium", 0.55),
        ("Human Written", "Low", 0.31),
    ];
    let mut windows = Vec::new();
    let (mut ai, mut assisted, mut human) = (0usize, 0usize, 0usize);
    for (n, (s, e)) in bounds.into_iter().enumerate() {
        // Trim surrounding whitespace from each window.
        let (mut s, mut e) = (s, e);
        while s < e && chars[s].is_whitespace() {
            s += 1;
        }
        while e > s && chars[e - 1].is_whitespace() {
            e -= 1;
        }
        if s == e {
            continue;
        }
        let seed = chars[s..e].iter().map(|c| *c as usize).sum::<usize>() + n;
        let (label, confidence, score) = labels[seed % labels.len()];
        let len = e - s;
        match label {
            "AI-Generated" => ai += len,
            "AI-Assisted" => assisted += len,
            _ => human += len,
        }
        let window_text: String = chars[s..e].iter().collect();
        windows.push(json!({
            "text": window_text,
            "label": label,
            "ai_assistance_score": score,
            "confidence": confidence,
            "start_index": s,
            "end_index": e,
            "word_count": window_text.split_whitespace().count(),
            "token_length": window_text.split_whitespace().count() * 4 / 3 + 1,
            "is_humanized": seed % 7 == 0,
            "humanizer_score": (seed % 10) as f64 / 10.0,
        }));
    }
    let total = (ai + assisted + human).max(1) as f64;
    let (fa, fs, fh) = (
        ai as f64 / total,
        assisted as f64 / total,
        human as f64 / total,
    );
    let short = if fa + fs > 0.6 {
        "AI"
    } else if fa + fs < 0.2 {
        "Human"
    } else {
        "Mixed"
    };
    json!({
        "stage": "STAGE_SUCCESS",
        "text": text,
        "version": "4.0-mock",
        "headline": match short { "AI" => "AI Detected", "Human" => "Fully Human Written", _ => "Mixed" },
        "prediction": format!("Mock analysis: {:.0}% AI-generated, {:.0}% AI-assisted.", fa * 100.0, fs * 100.0),
        "prediction_short": short,
        "fraction_ai": fa,
        "fraction_ai_assisted": fs,
        "fraction_human": fh,
        "num_ai_segments": windows.iter().filter(|w| w["label"] == "AI-Generated").count(),
        "num_ai_assisted_segments": windows.iter().filter(|w| w["label"] == "AI-Assisted").count(),
        "num_human_segments": windows.iter().filter(|w| w["label"] == "Human Written").count(),
        "windows": windows,
    })
}

#[tokio::main]
async fn main() {
    let port: u16 = std::env::var("MOCK_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(8765);
    let delay = Duration::from_millis(
        std::env::var("MOCK_DELAY_MS")
            .ok()
            .and_then(|d| d.parse().ok())
            .unwrap_or(3000),
    );
    let listener = std::net::TcpListener::bind(("127.0.0.1", port)).expect("bind mock port");
    let server = MockServer::builder().listener(listener).start().await;
    let tasks = Arc::new(Tasks::default());
    Mock::given(method("GET"))
        .and(path("/models"))
        .respond_with(Models)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/task"))
        .respond_with(Submit(tasks.clone()))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path_regex("^/task/[^/]+$"))
        .respond_with(Poll(tasks, delay))
        .mount(&server)
        .await;
    println!("Mock Pangram API at {}", server.uri());
    println!(
        "Run: PANGRAM_API_BASE={} cargo run -p pangram-desktop",
        server.uri()
    );
    tokio::signal::ctrl_c().await.ok();
}
