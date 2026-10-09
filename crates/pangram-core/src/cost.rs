//! Cost estimates. The API reports no cost, so charges are derived from Pangram's published
//! billing rules (pangram.com, checked 2026-10-09):
//!
//! * Pangram 4: one credit per 100 words, rounded up per request; $0.05 per credit.
//! * Pangram 3 (legacy): one credit per 1,000 words.
//!
//! Billed words come from the response's per-window `word_count` (Pangram's own count, which
//! differs from a whitespace split). The price per credit is a user setting.

use serde::Serialize;

use crate::api::DetectionResult;

pub const DEFAULT_USD_PER_CREDIT: f64 = 0.05;

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Charge {
    pub words: i64,
    pub credits: i64,
    pub usd: f64,
    /// True when words were counted locally rather than reported by Pangram.
    pub words_estimated: bool,
}

/// Words per credit for a model version (`"4.0"`) or selector (`"pangram-4"`, `"default"`).
pub fn words_per_credit(version_or_model: &str) -> i64 {
    let major = version_or_model
        .trim_start_matches(|c: char| !c.is_ascii_digit())
        .split(|c: char| !c.is_ascii_digit())
        .next();
    if major == Some("3") { 1000 } else { 100 }
}

pub fn credits(words: i64, words_per_credit: i64) -> i64 {
    if words <= 0 {
        0
    } else {
        (words + words_per_credit - 1) / words_per_credit
    }
}

fn charge(words: i64, model: &str, usd_per_credit: f64, words_estimated: bool) -> Charge {
    let credits = credits(words, words_per_credit(model));
    Charge {
        words,
        credits,
        usd: credits as f64 * usd_per_credit,
        words_estimated,
    }
}

/// Local word count, used before submission and when the response has no counts.
pub fn count_words(text: &str) -> i64 {
    text.split_whitespace().count() as i64
}

/// Charge for a completed scan. `version` falls back to the requested model.
pub fn charge_for_result(
    result: &DetectionResult,
    requested_model: &str,
    usd_per_credit: f64,
) -> Charge {
    let reported: Option<i64> = if result.windows.is_empty() {
        None
    } else {
        result.windows.iter().map(|w| w.word_count).sum()
    };
    let model = result.version.as_deref().unwrap_or(requested_model);
    match reported {
        Some(words) => charge(words, model, usd_per_credit, false),
        None => charge(count_words(&result.text), model, usd_per_credit, true),
    }
}

/// Estimate for a draft before submission.
pub fn estimate_text(text: &str, model: &str, usd_per_credit: f64) -> Charge {
    charge(count_words(text), model, usd_per_credit, true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::Window;

    #[test]
    fn rates_by_model() {
        assert_eq!(words_per_credit("4.0"), 100);
        assert_eq!(words_per_credit("pangram-4"), 100);
        assert_eq!(words_per_credit("default"), 100);
        assert_eq!(words_per_credit("3.3.2"), 1000);
        assert_eq!(words_per_credit("pangram-3.3"), 1000);
        assert_eq!(words_per_credit("30.1"), 100);
    }

    #[test]
    fn rounds_up_per_request() {
        assert_eq!(credits(0, 100), 0);
        assert_eq!(credits(1, 100), 1);
        assert_eq!(credits(99, 100), 1);
        assert_eq!(credits(100, 100), 1);
        assert_eq!(credits(101, 100), 2);
        assert_eq!(credits(174, 1000), 1);
    }

    #[test]
    fn uses_reported_word_counts() {
        let result = DetectionResult {
            text: "one two three".into(),
            version: Some("4.0".into()),
            windows: vec![
                Window {
                    word_count: Some(100),
                    ..Default::default()
                },
                Window {
                    word_count: Some(72),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let c = charge_for_result(&result, "default", 0.05);
        assert_eq!((c.words, c.credits, c.words_estimated), (172, 2, false));
        assert!((c.usd - 0.10).abs() < 1e-9);

        let missing = DetectionResult {
            windows: vec![Window::default()],
            ..result
        };
        let c = charge_for_result(&missing, "default", 0.05);
        assert_eq!((c.words, c.credits, c.words_estimated), (3, 1, true));
    }

    #[test]
    fn estimates_drafts() {
        let c = estimate_text(&"word ".repeat(250), "pangram-4", 0.05);
        assert_eq!((c.words, c.credits), (250, 3));
        assert!((c.usd - 0.15).abs() < 1e-9);
    }
}
