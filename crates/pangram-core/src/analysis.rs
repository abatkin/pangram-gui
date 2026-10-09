//! Result interpretation: label classes, index-unit detection, range validation and the mapping
//! from API offsets to UTF-16 offsets in the text shown by Qt.
//!
//! The API documents end-exclusive `start_index`/`end_index` into the *returned* text but not the
//! unit. We try code points, UTF-16 units and UTF-8 bytes in that order and accept the first unit
//! under which every window's range reproduces that window's text. If none does, highlights are
//! withheld and sections are shown separately.

use serde::Serialize;

use crate::api::{DetectionResult, Window};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum IndexUnit {
    CodePoints,
    Utf16,
    Utf8Bytes,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum LabelClass {
    Ai,
    Assisted,
    Human,
    Unknown,
}

impl LabelClass {
    pub fn from_label(label: &str) -> Self {
        let l = label.to_ascii_lowercase();
        if l.contains("assist") {
            Self::Assisted
        } else if l.contains("human") {
            Self::Human
        } else if l.contains("ai") || l.contains("machine") || l.contains("generated") {
            Self::Ai
        } else {
            Self::Unknown
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Section {
    pub index: usize,
    pub label: String,
    pub class: LabelClass,
    pub confidence: Option<String>,
    pub ai_assistance_score: Option<f64>,
    pub is_humanized: Option<bool>,
    pub humanizer_score: Option<f64>,
    pub word_count: Option<i64>,
    pub token_length: Option<i64>,
    pub text: String,
    pub details: Vec<(String, String)>,
    /// UTF-16 range in `display_text`, present only when highlights are valid.
    pub start: Option<usize>,
    pub end: Option<usize>,
}

/// A run of display text, either inside a section or between sections.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Span {
    pub start: usize,
    pub end: usize,
    pub section: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Analysis {
    /// Returned text with line separators normalised so each UTF-16 unit is one Qt text position.
    pub display_text: String,
    pub display_len: usize,
    pub highlights_valid: bool,
    pub index_unit: Option<IndexUnit>,
    pub highlight_note: Option<String>,
    pub sections: Vec<Section>,
    #[serde(skip)]
    pub spans: Vec<Span>,
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
}

/// Offsets of every char boundary in each unit, plus the display mapping.
struct Offsets {
    utf8: Vec<usize>,
    utf16: Vec<usize>,
    display: Vec<usize>,
    display_text: String,
}

fn offsets(text: &str) -> Offsets {
    let n = text.chars().count();
    let mut utf8 = Vec::with_capacity(n + 1);
    let mut utf16 = Vec::with_capacity(n + 1);
    let mut display = Vec::with_capacity(n + 1);
    let mut display_text = String::with_capacity(text.len());
    let (mut b, mut u, mut d) = (0usize, 0usize, 0usize);
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        utf8.push(b);
        utf16.push(u);
        display.push(d);
        b += c.len_utf8();
        u += c.len_utf16();
        let shown = match c {
            // CRLF becomes one newline: the CR occupies no display position.
            '\r' if chars.peek() == Some(&'\n') => None,
            '\r' | '\u{0B}' | '\u{0C}' | '\u{85}' | '\u{2028}' | '\u{2029}' => Some('\n'),
            '\t' | '\n' => Some(c),
            c if c.is_control() => Some('\u{FFFD}'),
            c => Some(c),
        };
        if let Some(s) = shown {
            display_text.push(s);
            d += s.len_utf16();
        }
    }
    utf8.push(b);
    utf16.push(u);
    display.push(d);
    Offsets {
        utf8,
        utf16,
        display,
        display_text,
    }
}

/// Converts an API offset to a char index under the given unit.
fn to_char_index(o: &Offsets, unit: IndexUnit, offset: i64) -> Option<usize> {
    let offset = usize::try_from(offset).ok()?;
    match unit {
        IndexUnit::CodePoints => (offset < o.utf8.len()).then_some(offset),
        IndexUnit::Utf16 => o.utf16.binary_search(&offset).ok(),
        IndexUnit::Utf8Bytes => o.utf8.binary_search(&offset).ok(),
    }
}

fn texts_match(slice: &str, expected: &str) -> bool {
    slice == expected || slice.trim() == expected.trim()
}

/// Char ranges for every window under `unit`, or `None` if any window fails validation.
fn validate(
    text: &str,
    o: &Offsets,
    windows: &[Window],
    unit: IndexUnit,
) -> Option<Vec<(usize, usize)>> {
    windows
        .iter()
        .map(|w| {
            let start = to_char_index(o, unit, w.start_index?)?;
            let end = to_char_index(o, unit, w.end_index?)?;
            if start > end {
                return None;
            }
            let slice = &text[o.utf8[start]..o.utf8[end]];
            texts_match(slice, w.text.as_deref()?).then_some((start, end))
        })
        .collect()
}

impl Analysis {
    pub fn from_result(result: &DetectionResult) -> Self {
        let text = result.text.as_str();
        let o = offsets(text);

        let mut chosen = None;
        let mut note = None;
        if result.windows.is_empty() {
            chosen = Some((None, Vec::new()));
        } else {
            for unit in [
                IndexUnit::CodePoints,
                IndexUnit::Utf16,
                IndexUnit::Utf8Bytes,
            ] {
                if let Some(ranges) = validate(text, &o, &result.windows, unit) {
                    chosen = Some((Some(unit), ranges));
                    break;
                }
            }
            if chosen.is_none() {
                note = Some(
                    "Section ranges did not match the analyzed text, so highlights are not shown. \
                     Each section's text is listed separately."
                        .to_owned(),
                );
            }
        }

        let highlights_valid = chosen.is_some();
        let index_unit = chosen.as_ref().and_then(|(u, _)| *u);
        let ranges = chosen.map(|(_, r)| r);

        let sections: Vec<Section> = result
            .windows
            .iter()
            .enumerate()
            .map(|(i, w)| {
                let label = w.label.clone().unwrap_or_else(|| "Unlabeled".to_owned());
                let (start, end) = match &ranges {
                    Some(r) => (Some(o.display[r[i].0]), Some(o.display[r[i].1])),
                    None => (None, None),
                };
                Section {
                    index: i,
                    class: LabelClass::from_label(&label),
                    label,
                    confidence: w.confidence.clone(),
                    ai_assistance_score: w.ai_assistance_score,
                    is_humanized: w.is_humanized,
                    humanizer_score: w.humanizer_score,
                    word_count: w.word_count,
                    token_length: w.token_length,
                    text: w.text.clone().unwrap_or_default(),
                    details: w.extra.clone(),
                    start,
                    end,
                }
            })
            .collect();

        let display_len = *o.display.last().unwrap_or(&0);
        let spans = build_spans(&sections, display_len);

        Self {
            display_text: o.display_text,
            display_len,
            highlights_valid,
            index_unit,
            highlight_note: note,
            sections,
            spans,
            version: result.version.clone(),
            headline: result.headline.clone(),
            prediction: result.prediction.clone(),
            prediction_short: result.prediction_short.clone(),
            fraction_ai: result.fraction_ai,
            fraction_ai_assisted: result.fraction_ai_assisted,
            fraction_human: result.fraction_human,
            num_ai_segments: result.num_ai_segments,
            num_ai_assisted_segments: result.num_ai_assisted_segments,
            num_human_segments: result.num_human_segments,
        }
    }

    /// Section containing the UTF-16 display position, if any.
    pub fn section_at(&self, pos: usize) -> Option<usize> {
        let i = self.spans.partition_point(|s| s.end <= pos);
        self.spans
            .get(i)
            .filter(|s| s.start <= pos && pos < s.end)
            .and_then(|s| s.section)
    }

    /// Rich text for Qt's text document. Content is escaped so it always renders literally.
    pub fn to_html(&self, palette: &HighlightPalette) -> String {
        let mut html = String::with_capacity(self.display_text.len() * 2 + 64);
        html.push_str("<div style=\"white-space: pre-wrap;\">");
        let units: Vec<u16> = self.display_text.encode_utf16().collect();
        for span in &self.spans {
            let piece = String::from_utf16_lossy(&units[span.start..span.end]);
            let color = span
                .section
                .and_then(|i| self.sections.get(i))
                .and_then(|s| palette.color(s.class));
            // Line breaks are emitted as <br> outside highlighted spans: Qt's importer gives a
            // new paragraph block the previous span's format, which bleeds the highlight across
            // blank lines. <br> is a single position (U+2028), like the newline it replaces.
            for (i, line) in piece.split('\n').enumerate() {
                if i > 0 {
                    html.push_str("<br>");
                }
                if line.is_empty() {
                    continue;
                }
                match color {
                    Some(c) => {
                        html.push_str("<span style=\"background-color:");
                        html.push_str(&escape_html(c));
                        html.push_str(";\">");
                        html.push_str(&escape_html(line));
                        html.push_str("</span>");
                    }
                    None => html.push_str(&escape_html(line)),
                }
            }
        }
        html.push_str("</div>");
        html
    }

    /// Plain-text summary for the clipboard.
    pub fn summary_text(&self, title: &str, model: &str) -> String {
        let pct = |f: Option<f64>| f.map_or("n/a".to_owned(), |v| format!("{:.0}%", v * 100.0));
        let mut out = String::new();
        if !title.is_empty() {
            out.push_str(&format!("{title}\n"));
        }
        if let Some(h) = &self.headline {
            out.push_str(&format!("Result: {h}"));
            if let Some(p) = &self.prediction_short {
                out.push_str(&format!(" ({p})"));
            }
            out.push('\n');
        }
        if let Some(p) = &self.prediction {
            out.push_str(&format!("{p}\n"));
        }
        out.push_str(&format!(
            "Share of text — AI-generated: {}, AI-assisted: {}, human-written: {}\n",
            pct(self.fraction_ai),
            pct(self.fraction_ai_assisted),
            pct(self.fraction_human)
        ));
        out.push_str(&format!(
            "Model: {model}{}\n",
            self.version
                .as_ref()
                .map(|v| format!(" (version {v})"))
                .unwrap_or_default()
        ));
        if !self.sections.is_empty() {
            out.push_str("\nSections:\n");
            for s in &self.sections {
                let conf = s
                    .confidence
                    .as_ref()
                    .map(|c| format!(", {c} confidence"))
                    .unwrap_or_default();
                out.push_str(&format!("{}. {}{}\n", s.index + 1, s.label, conf));
                out.push_str(&format!("   {}\n", s.text.trim().replace('\n', "\n   ")));
            }
        }
        out
    }
}

/// Plain-language description of how Pangram changed the submitted text, or `None` if it didn't.
pub fn describe_normalization(input: &str, returned: &str) -> Option<String> {
    if input == returned {
        return None;
    }
    let visible = |s: &str| s.chars().filter(|c| !c.is_whitespace()).collect::<String>();
    if visible(input) != visible(returned) {
        return Some(
            "Pangram changed some characters before analyzing (for example quotes or symbols). \
             The highlighted text is Pangram's version, so it can differ slightly from what you \
             submitted."
                .to_owned(),
        );
    }
    let mut changes = Vec::new();
    if input.contains('\r') && !returned.contains('\r') {
        changes.push("converted Windows line endings");
    }
    let blank_lines = |s: &str| s.replace("\r\n", "\n").matches("\n\n").count();
    if blank_lines(input) > blank_lines(returned) {
        changes.push("removed blank lines");
    }
    let indented = |s: &str| s.lines().filter(|l| l.starts_with([' ', '\t'])).count();
    if indented(input) > indented(returned) {
        changes.push("removed indentation");
    }
    if changes.is_empty() {
        changes.push("adjusted spacing");
    }
    let list = match changes.as_slice() {
        [one] => one.to_string(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
        [] => unreachable!(),
    };
    Some(format!(
        "Pangram {list} before analyzing. Only whitespace changed; the words are exactly what you \
         submitted."
    ))
}

fn build_spans(sections: &[Section], len: usize) -> Vec<Span> {
    let mut ordered: Vec<&Section> = sections.iter().filter(|s| s.start.is_some()).collect();
    ordered.sort_by_key(|s| (s.start, s.index));
    let mut spans = Vec::new();
    let mut cursor = 0;
    for s in ordered {
        let (start, end) = (s.start.unwrap().max(cursor), s.end.unwrap().min(len));
        if start >= end {
            continue; // empty, or fully covered by an overlapping earlier section
        }
        if start > cursor {
            spans.push(Span {
                start: cursor,
                end: start,
                section: None,
            });
        }
        spans.push(Span {
            start,
            end,
            section: Some(s.index),
        });
        cursor = end;
    }
    if cursor < len {
        spans.push(Span {
            start: cursor,
            end: len,
            section: None,
        });
    }
    spans
}

#[derive(Debug, Clone)]
pub struct HighlightPalette {
    pub ai: String,
    pub assisted: String,
    pub human: String,
}

impl HighlightPalette {
    fn color(&self, class: LabelClass) -> Option<&str> {
        let c = match class {
            LabelClass::Ai => &self.ai,
            LabelClass::Assisted => &self.assisted,
            LabelClass::Human => &self.human,
            LabelClass::Unknown => return None,
        };
        (!c.is_empty()).then_some(c.as_str())
    }
}

pub fn escape_html(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window(text: &str, label: &str, start: i64, end: i64) -> Window {
        Window {
            text: Some(text.to_owned()),
            label: Some(label.to_owned()),
            start_index: Some(start),
            end_index: Some(end),
            confidence: Some("High".to_owned()),
            ..Default::default()
        }
    }

    fn result(text: &str, windows: Vec<Window>) -> DetectionResult {
        DetectionResult {
            text: text.to_owned(),
            windows,
            ..Default::default()
        }
    }

    fn utf16_slice(s: &str, start: usize, end: usize) -> String {
        let u: Vec<u16> = s.encode_utf16().collect();
        String::from_utf16(&u[start..end]).unwrap()
    }

    #[test]
    fn classifies_labels() {
        assert_eq!(LabelClass::from_label("AI-Generated"), LabelClass::Ai);
        assert_eq!(LabelClass::from_label("AI-Assisted"), LabelClass::Assisted);
        assert_eq!(LabelClass::from_label("Human Written"), LabelClass::Human);
        assert_eq!(LabelClass::from_label("Something new"), LabelClass::Unknown);
    }

    #[test]
    fn ascii_ranges_produce_spans() {
        let text = "Hello world. This is a test.";
        let a = Analysis::from_result(&result(
            text,
            vec![
                window("Hello world.", "AI-Generated", 0, 12),
                window("This is a test.", "Human Written", 13, 28),
            ],
        ));
        assert!(a.highlights_valid);
        assert_eq!(a.index_unit, Some(IndexUnit::CodePoints));
        assert_eq!(
            a.spans,
            vec![
                Span {
                    start: 0,
                    end: 12,
                    section: Some(0)
                },
                Span {
                    start: 12,
                    end: 13,
                    section: None
                },
                Span {
                    start: 13,
                    end: 28,
                    section: Some(1)
                },
            ]
        );
        assert_eq!(a.section_at(0), Some(0));
        assert_eq!(a.section_at(12), None);
        assert_eq!(a.section_at(27), Some(1));
        assert_eq!(a.section_at(28), None);
    }

    // "Café 😀 ok. 日本語です。" — é is 1 cp/1 u16/2 bytes, 😀 is 1 cp/2 u16/4 bytes.
    const UNI: &str = "Café 😀 ok. 日本語です。";

    #[test]
    fn detects_code_point_indices() {
        let first = "Café 😀 ok.";
        let a = Analysis::from_result(&result(
            UNI,
            vec![
                window(first, "AI-Generated", 0, 10),
                window("日本語です。", "Human Written", 11, 17),
            ],
        ));
        assert_eq!(a.index_unit, Some(IndexUnit::CodePoints));
        let s0 = &a.sections[0];
        assert_eq!(
            utf16_slice(&a.display_text, s0.start.unwrap(), s0.end.unwrap()),
            first
        );
        let s1 = &a.sections[1];
        assert_eq!(
            utf16_slice(&a.display_text, s1.start.unwrap(), s1.end.unwrap()),
            "日本語です。"
        );
    }

    #[test]
    fn detects_utf16_indices() {
        let a = Analysis::from_result(&result(
            UNI,
            vec![
                window("Café 😀 ok.", "AI-Generated", 0, 11),
                window("日本語です。", "Human Written", 12, 18),
            ],
        ));
        assert_eq!(a.index_unit, Some(IndexUnit::Utf16));
        assert_eq!(
            (a.sections[1].start, a.sections[1].end),
            (Some(12), Some(18))
        );
    }

    #[test]
    fn detects_utf8_byte_indices() {
        let first_bytes = "Café 😀 ok.".len() as i64;
        let a = Analysis::from_result(&result(
            UNI,
            vec![
                window("Café 😀 ok.", "AI-Generated", 0, first_bytes),
                window(
                    "日本語です。",
                    "Human Written",
                    first_bytes + 1,
                    UNI.len() as i64,
                ),
            ],
        ));
        assert_eq!(a.index_unit, Some(IndexUnit::Utf8Bytes));
        assert_eq!(
            (a.sections[0].start, a.sections[0].end),
            (Some(0), Some(11))
        );
    }

    #[test]
    fn mismatched_ranges_disable_highlights() {
        let a = Analysis::from_result(&result(
            "Hello world.",
            vec![window("Something else", "AI-Generated", 0, 5)],
        ));
        assert!(!a.highlights_valid);
        assert!(a.highlight_note.is_some());
        assert_eq!(a.sections[0].start, None);
        assert_eq!(
            a.spans,
            vec![Span {
                start: 0,
                end: 12,
                section: None
            }]
        );
        assert_eq!(a.section_at(1), None);

        let out_of_range =
            Analysis::from_result(&result("Hi", vec![window("Hi", "AI-Generated", 0, 99)]));
        assert!(!out_of_range.highlights_valid);
        let reversed = Analysis::from_result(&result("Hi", vec![window("", "AI-Generated", 2, 0)]));
        assert!(!reversed.highlights_valid);
        let missing_text = Analysis::from_result(&result(
            "Hi",
            vec![Window {
                start_index: Some(0),
                end_index: Some(2),
                ..Default::default()
            }],
        ));
        assert!(!missing_text.highlights_valid);
    }

    #[test]
    fn splitting_a_surrogate_pair_is_rejected_for_utf16() {
        // Offset 6 lands inside the emoji in UTF-16; code points also fail the text check.
        let a = Analysis::from_result(&result(
            UNI,
            vec![window("Café \u{FFFD}", "AI-Generated", 0, 6)],
        ));
        assert!(!a.highlights_valid);
    }

    #[test]
    fn whitespace_trimmed_window_text_is_accepted() {
        let a = Analysis::from_result(&result(
            "One. Two.",
            vec![
                window("One.", "AI-Generated", 0, 5),
                window("Two.", "Human Written", 5, 9),
            ],
        ));
        assert!(a.highlights_valid);
    }

    #[test]
    fn normalized_returned_text_is_what_gets_highlighted() {
        // Pangram may normalize input (here, curly quotes); offsets address the returned text.
        let returned = "He said \"hi\".";
        let a = Analysis::from_result(&result(
            returned,
            vec![window(returned, "Human Written", 0, 13)],
        ));
        assert!(a.highlights_valid);
        assert_eq!(a.display_text, returned);
    }

    #[test]
    fn crlf_and_separators_map_to_single_positions() {
        let text = "Line one.\r\nLine two.\u{2028}End\u{0}";
        let a = Analysis::from_result(&result(
            text,
            vec![
                window("Line two.", "AI-Generated", 11, 20),
                window("End\u{0}", "Human Written", 21, 25),
            ],
        ));
        assert!(a.highlights_valid);
        assert_eq!(a.display_text, "Line one.\nLine two.\nEnd\u{FFFD}");
        let s0 = &a.sections[0];
        assert_eq!(
            utf16_slice(&a.display_text, s0.start.unwrap(), s0.end.unwrap()),
            "Line two."
        );
        assert_eq!(a.display_len, a.display_text.encode_utf16().count());
    }

    #[test]
    fn overlapping_windows_are_clipped() {
        let a = Analysis::from_result(&result(
            "abcdefghij",
            vec![
                window("abcdef", "AI-Generated", 0, 6),
                window("efghij", "Human Written", 4, 10),
            ],
        ));
        assert!(a.highlights_valid);
        assert_eq!(
            a.spans,
            vec![
                Span {
                    start: 0,
                    end: 6,
                    section: Some(0)
                },
                Span {
                    start: 6,
                    end: 10,
                    section: Some(1)
                },
            ]
        );
    }

    #[test]
    fn no_windows_means_plain_text() {
        let a = Analysis::from_result(&result("Plain.", vec![]));
        assert!(a.highlights_valid);
        assert_eq!(a.index_unit, None);
        assert_eq!(a.spans.len(), 1);
    }

    #[test]
    fn html_escapes_content() {
        let text = "<b>x</b> & \"y\"";
        let a = Analysis::from_result(&result(
            text,
            vec![window("<b>x</b>", "AI-Generated", 0, 8)],
        ));
        let palette = HighlightPalette {
            ai: "#ff0000".into(),
            assisted: "#ffff00".into(),
            human: String::new(),
        };
        let html = a.to_html(&palette);
        assert!(!html.contains("<b>"));
        assert!(html.contains("&lt;b&gt;x&lt;/b&gt;"));
        assert!(html.contains("&amp; &quot;y&quot;"));
        assert!(html.contains("background-color:#ff0000"));
    }

    #[test]
    fn html_line_breaks_stay_outside_highlights() {
        let text = "One.\n\nTwo.";
        let a = Analysis::from_result(&result(
            text,
            vec![
                window("One.\n", "AI-Generated", 0, 5),
                window("Two.", "Human Written", 6, 10),
            ],
        ));
        let palette = HighlightPalette {
            ai: "#a".into(),
            assisted: "#b".into(),
            human: "#c".into(),
        };
        let html = a.to_html(&palette);
        assert_eq!(
            html,
            "<div style=\"white-space: pre-wrap;\"><span style=\"background-color:#a;\">One.</span>\
             <br><br><span style=\"background-color:#c;\">Two.</span></div>"
        );
        assert!(!html.contains('\n'));
    }

    #[test]
    fn summary_mentions_fractions_and_sections() {
        let mut r = result("Hello.", vec![window("Hello.", "AI-Generated", 0, 6)]);
        r.fraction_ai = Some(1.0);
        r.fraction_human = Some(0.0);
        r.headline = Some("AI Detected".into());
        let s = Analysis::from_result(&r).summary_text("Essay", "default");
        assert!(s.contains("AI-generated: 100%"));
        assert!(s.contains("AI-assisted: n/a"));
        assert!(s.contains("1. AI-Generated, High confidence"));
    }

    #[test]
    fn describes_normalization() {
        assert_eq!(describe_normalization("a b", "a b"), None);
        let ws = describe_normalization("One.\r\n\r\n\tTwo.", "One.\nTwo.").unwrap();
        assert!(ws.starts_with("Pangram converted Windows line endings, removed blank lines and removed indentation"), "{ws}");
        assert!(
            describe_normalization("a  b", "a b")
                .unwrap()
                .contains("adjusted spacing")
        );
        assert!(
            describe_normalization("“a”", "\"a\"")
                .unwrap()
                .contains("changed some characters")
        );
    }
}
