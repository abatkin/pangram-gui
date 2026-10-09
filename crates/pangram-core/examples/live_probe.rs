//! Probes the real Pangram API with the key stored by the desktop app (Secret Service).
//! Prints response headers and a summary of the result; never prints the key.
//!
//! ```sh
//! cargo run -p pangram-core --example live_probe -- models          # free: GET /models
//! cargo run -p pangram-core --example live_probe -- scan FILE MODEL # ONE scan (spends credits)
//! ```

use std::time::Duration;

use pangram_core::analysis::Analysis;
use pangram_core::api::{DEFAULT_BASE_URL, DetectionResult};
use pangram_core::credentials::CredentialStore;
use reqwest::header::HeaderMap;

fn print_headers(label: &str, headers: &HeaderMap) {
    println!("--- {label} headers");
    for (k, v) in headers {
        println!("  {k}: {}", v.to_str().unwrap_or("<binary>"));
    }
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let key = CredentialStore::secret_service()
        .load()
        .await
        .expect("keyring")
        .expect("no key stored by the app");
    let http = reqwest::Client::new();
    let base = std::env::var("PANGRAM_API_BASE").unwrap_or_else(|_| DEFAULT_BASE_URL.to_owned());

    match args.first().map(String::as_str) {
        Some("models") => {
            let r = http
                .get(format!("{base}/models"))
                .header("x-api-key", key.expose())
                .send()
                .await
                .unwrap();
            println!("status {}", r.status());
            print_headers("GET /models", r.headers());
            println!("{}", r.text().await.unwrap());
        }
        Some("scan") => {
            let text = std::fs::read_to_string(&args[1]).expect("read text file");
            let model = args.get(2).map(String::as_str).unwrap_or("default");
            let r = http
                .post(format!("{base}/task"))
                .header("x-api-key", key.expose())
                .json(&serde_json::json!({"text": text, "model": model, "public_dashboard_link": false}))
                .send()
                .await
                .unwrap();
            println!("status {}", r.status());
            print_headers("POST /task", r.headers());
            let body: serde_json::Value = r.json().await.unwrap();
            println!("POST body: {body}");
            let task_id = body["task_id"].as_str().expect("task_id").to_owned();
            loop {
                tokio::time::sleep(Duration::from_secs(2)).await;
                let r = http
                    .get(format!("{base}/task/{task_id}"))
                    .header("x-api-key", key.expose())
                    .send()
                    .await
                    .unwrap();
                let status = r.status();
                let headers = r.headers().clone();
                let raw = r.text().await.unwrap();
                let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
                let stage = v["stage"].as_str().unwrap_or("?").to_owned();
                println!("GET status {status} stage {stage}");
                if stage != "STAGE_SUCCESS" && stage != "STAGE_FAILED" {
                    continue;
                }
                print_headers("final GET /task", &headers);
                std::fs::write("live_probe_response.json", &raw).unwrap();
                let mut summary = v.clone();
                summary["text"] = format!(
                    "<{} chars>",
                    v["text"].as_str().unwrap_or("").chars().count()
                )
                .into();
                for w in summary["windows"].as_array_mut().into_iter().flatten() {
                    w["text"] = format!(
                        "<{} chars>",
                        w["text"].as_str().unwrap_or("").chars().count()
                    )
                    .into();
                }
                println!("{}", serde_json::to_string_pretty(&summary).unwrap());
                let result = DetectionResult::from_json(&v).unwrap();
                let a = Analysis::from_result(&result);
                let returned = &result.text;
                println!(
                    "input: {} cp / {} utf16 / {} bytes",
                    text.chars().count(),
                    text.encode_utf16().count(),
                    text.len()
                );
                println!(
                    "returned: {} cp / {} utf16 / {} bytes",
                    returned.chars().count(),
                    returned.encode_utf16().count(),
                    returned.len()
                );
                println!("returned == input: {}", *returned == text);
                println!(
                    "index unit: {:?}, highlights valid: {}",
                    a.index_unit, a.highlights_valid
                );
                println!(
                    "whitespace words in input: {}",
                    text.split_whitespace().count()
                );
                break;
            }
        }
        _ => eprintln!("usage: live_probe models | scan FILE [MODEL]"),
    }
}
