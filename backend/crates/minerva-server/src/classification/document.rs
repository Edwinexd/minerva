//! Cerebras gpt-oss-120b-backed document classifier.

use async_trait::async_trait;
use minerva_ingest::classifier::{ClassifiedKind, Classifier};

use super::prompts::{CLASSIFIER_SYSTEM_PROMPT, CLASSIFIER_USER_TEMPLATE};
use super::types::{DocumentKind, ALL_KINDS};
use crate::strategy::common::cerebras_request_with_retry;

/// Cerebras model name for ingest-time classification work. Open-weights
/// gpt-oss-120b -- strong instruction-following, reasoning_effort
/// supported, structured outputs supported.
const CLASSIFIER_MODEL: &str = "gpt-oss-120b";

/// Confidence below this triggers a retry with reasoning_effort = "high".
const RETRY_CONFIDENCE_THRESHOLD: f32 = 0.6;

/// Soft cap on excerpt length sent to the model. gpt-oss-120b has a 128K
/// context window; we leave plenty of room for the schema + prompt and
/// keep tokens cheap. Head/tail split so the model sees both the
/// introductory framing and any "submit / due / answer" footer.
const MAX_EXCERPT_CHARS: usize = 60_000;
const HEAD_FRACTION: f64 = 0.85;

pub struct CerebrasClassifier {
    http: reqwest::Client,
    api_key: String,
}

impl CerebrasClassifier {
    pub fn new(http: reqwest::Client, api_key: String) -> Self {
        Self { http, api_key }
    }

    async fn call(
        &self,
        filename: &str,
        mime_type: &str,
        excerpt: &str,
        reasoning_effort: &str,
    ) -> Result<ClassifiedKind, String> {
        // Filename hints get serialised as a JSON array literal so the
        // model sees them as an actual array (not a comma-separated
        // string). Empty list still serialises as "[]" -- harmless and
        // keeps the prompt template byte-stable across calls.
        let hints = filename_hints(filename);
        let hints_json = serde_json::to_string(&hints).unwrap_or_else(|_| "[]".to_string());
        let user = CLASSIFIER_USER_TEMPLATE
            .replace("{filename}", filename)
            .replace("{mime_type}", mime_type)
            .replace("{filename_hints}", &hints_json)
            .replace("{excerpt}", excerpt);

        // Cerebras supports OpenAI-style `response_format: json_schema`,
        // and the gpt-oss family additionally accepts `reasoning_effort`.
        // Schema mirrors prompts.rs's contract.
        let body = serde_json::json!({
            "model": CLASSIFIER_MODEL,
            "temperature": 0.0,
            "reasoning_effort": reasoning_effort,
            "messages": [
                { "role": "system", "content": CLASSIFIER_SYSTEM_PROMPT },
                { "role": "user", "content": user },
            ],
            "response_format": {
                "type": "json_schema",
                "json_schema": {
                    "name": "document_kind_classification",
                    "strict": true,
                    "schema": {
                        "type": "object",
                        "additionalProperties": false,
                        "required": ["kind", "confidence", "rationale", "suspicious_flags"],
                        "properties": {
                            "kind": { "type": "string", "enum": ALL_KINDS },
                            "confidence": { "type": "number", "minimum": 0.0, "maximum": 1.0 },
                            "rationale": { "type": "string" },
                            "suspicious_flags": {
                                "type": "array",
                                "items": { "type": "string" },
                            },
                        },
                    }
                }
            }
        });

        let response = cerebras_request_with_retry(&self.http, &self.api_key, &body).await?;
        let payload: serde_json::Value = response
            .json()
            .await
            .map_err(|e| format!("classifier: response not JSON: {e}"))?;

        let raw = payload["choices"][0]["message"]["content"]
            .as_str()
            .ok_or_else(|| {
                format!(
                    "classifier: missing choices[0].message.content; got: {}",
                    payload
                )
            })?;

        parse_classifier_response(raw)
    }
}

#[async_trait]
impl Classifier for CerebrasClassifier {
    async fn classify(
        &self,
        filename: &str,
        mime_type: &str,
        text: &str,
    ) -> Result<ClassifiedKind, String> {
        // Zero-text fast-path: if the extractor produced nothing
        // (URL stubs, scanned PDFs without OCR, unsupported types,
        // etc.) there's nothing for the model to read. Return
        // `unknown` with low confidence and the "no_text_extracted"
        // flag rather than asking the model to hallucinate from a
        // filename alone. The chat-time partition keeps unknowns out
        // of context anyway, so this is the safe default.
        if text.trim().is_empty() {
            tracing::info!(
                "classifier: empty text for {} -> short-circuiting to unknown/no_text_extracted",
                filename
            );
            return Ok(ClassifiedKind {
                kind: "unknown".to_string(),
                confidence: 0.2,
                rationale: Some("No text could be extracted from this document.".to_string()),
                suspicious_flags: vec!["no_text_extracted".to_string()],
            });
        }

        let excerpt = truncate_for_classification(text);

        // First pass -- cheap, low effort.
        let initial = self
            .call(filename, mime_type, &excerpt, "low")
            .await
            .map_err(|e| format!("classifier: low-effort call failed: {e}"))?;

        let needs_retry =
            initial.confidence < RETRY_CONFIDENCE_THRESHOLD || !initial.suspicious_flags.is_empty();

        if !needs_retry {
            return Ok(initial);
        }

        tracing::info!(
            "classifier: re-running {} with reasoning_effort=high (initial: kind={} confidence={:.2} flags={:?})",
            filename,
            initial.kind,
            initial.confidence,
            initial.suspicious_flags,
        );

        match self.call(filename, mime_type, &excerpt, "high").await {
            Ok(refined) => Ok(refined),
            Err(e) => {
                tracing::warn!(
                    "classifier: high-effort retry failed for {} ({}); keeping low-effort result",
                    filename,
                    e,
                );
                Ok(initial)
            }
        }
    }
}

/// Deterministic filename-pattern hints, surfaced to the classifier as
/// priors. We extract these in Rust (rather than letting the model
/// guess from the raw filename) so the model sees a structured signal
/// it can override only with explicit content evidence -- not just a
/// vague "the filename also says assignment".
///
/// Kept in sync with the linker's `UNIT_NUMBER_PREFIXES`: anything
/// that's a unit marker is also a kind hint here when it appears with
/// a known role keyword.
///
/// The hints are intentionally informal English sentences. The classifier
/// reads them as user-message context, not as schema enums; phrasing
/// like "looks like a Swedish 'övningsuppgift' (assignment)" makes the
/// signal unambiguous to a multilingual LLM without adding a fragile
/// enum the model might decline to extend.
pub fn filename_hints(filename: &str) -> Vec<String> {
    // Lower-case + drop diacritics so the same matcher handles both
    // "övningsuppgift" and "ovningsuppgift".
    let lc = filename.to_lowercase();
    let folded: String = lc
        .chars()
        .map(|c| match c {
            'å' | 'ä' | 'à' | 'á' | 'â' => 'a',
            'ö' | 'ø' | 'ò' | 'ó' | 'ô' => 'o',
            'é' | 'è' | 'ê' | 'ë' => 'e',
            'ü' | 'ù' | 'ú' | 'û' => 'u',
            'í' | 'ì' | 'î' | 'ï' => 'i',
            'ý' => 'y',
            'ñ' => 'n',
            'ç' => 'c',
            _ => c,
        })
        .collect();
    let mut hints: Vec<String> = Vec::new();

    // Solution markers: very high signal. Several Swedish variants for
    // DSV courses where lecturers love mixing Swedish and English.
    for token in &[
        "solution",
        "solutions",
        "answer",
        "answers",
        "answerkey",
        "facit",
        "losning",         // "lösning"
        "losningar",       // "lösningar"
        "losningsforslag", // "lösningsförslag"
        "model_answer",
        "modelanswer",
        "key.pdf",
    ] {
        if folded.contains(token) {
            hints.push(format!(
                "filename contains \"{}\" -- likely sample_solution",
                token
            ));
            break;
        }
    }

    // Assignment markers.
    for token in &[
        "assignment",
        "assignments",
        "homework",
        "uppgift",        // Swedish "task/assignment"
        "ovningsuppgift", // Swedish "exercise task"
        "inlamning",      // Swedish "submission"
    ] {
        if folded.contains(token) {
            hints.push(format!(
                "filename contains \"{}\" -- likely assignment_brief",
                token
            ));
            break;
        }
    }

    // Lab markers.
    for token in &["lab", "laboration", "practical"] {
        if folded.contains(token) {
            hints.push(format!(
                "filename contains \"{}\" -- likely lab_brief",
                token
            ));
            break;
        }
    }

    // Exam markers.
    for token in &["exam", "tenta", "tentamen", "midterm", "final_exam"] {
        if folded.contains(token) {
            hints.push(format!("filename contains \"{}\" -- likely exam", token));
            break;
        }
    }

    // Lecture markers.
    for token in &["lecture", "lecturenotes", "slides", "forelasning"] {
        if folded.contains(token) {
            hints.push(format!("filename contains \"{}\" -- likely lecture", token));
            break;
        }
    }
    // DSV-specific F01/F02/... naming convention is shorthand for
    // "Föreläsning 01" -- almost always a lecture.
    if has_f_lecture_prefix(&folded) {
        hints.push(
            "filename starts with DSV \"F<number>\" lecture pattern -- likely lecture".to_string(),
        );
    }

    // Reading markers.
    for token in &["chapter", "ch_", "kapitel", "reading", "paper", "article"] {
        if folded.contains(token) {
            hints.push(format!("filename contains \"{}\" -- likely reading", token));
            break;
        }
    }

    // Syllabus markers.
    for token in &["syllabus", "schedule", "courseplan", "kursplan", "kurs_pm"] {
        if folded.contains(token) {
            hints.push(format!(
                "filename contains \"{}\" -- likely syllabus",
                token
            ));
            break;
        }
    }

    hints
}

/// True iff the filename starts with a DSV-style "F<digits>" or
/// "f<digits>" lecture marker (e.g. "F01_Arv.pdf", "f12-trees.pdf"),
/// case-insensitive, separator-tolerant. Avoids false positives like
/// "f1-results" being mistaken for a single-digit lecture marker by
/// requiring at least one digit and rejecting words starting with f
/// followed by a non-digit ("first.pdf" / "facit.pdf").
fn has_f_lecture_prefix(folded_lc: &str) -> bool {
    let trimmed = folded_lc.trim_start_matches(['_', '-', ' ']);
    let bytes = trimmed.as_bytes();
    if bytes.first().copied() != Some(b'f') {
        return false;
    }
    if bytes.len() < 2 {
        return false;
    }
    // Reject "facit" / "first" / "final" -- letter immediately after f.
    if bytes[1].is_ascii_alphabetic() {
        return false;
    }
    // Skip optional separator (rare for F-numbering but cheap).
    let mut j = 1;
    while j < bytes.len() && matches!(bytes[j], b'-' | b'_' | b' ' | b'.') {
        j += 1;
    }
    let digits_start = j;
    while j < bytes.len() && bytes[j].is_ascii_digit() {
        j += 1;
    }
    j > digits_start
}

/// Head/tail char-window. UTF-8 safe: we use `char_indices` to split on
/// codepoint boundaries, never byte offsets in the middle of a multi-byte
/// scalar.
pub fn truncate_for_classification(text: &str) -> String {
    let total_chars = text.chars().count();
    if total_chars <= MAX_EXCERPT_CHARS {
        return text.to_string();
    }

    let head_chars = (MAX_EXCERPT_CHARS as f64 * HEAD_FRACTION) as usize;
    let tail_chars = MAX_EXCERPT_CHARS - head_chars;

    // Take the first `head_chars` codepoints
    let mut iter = text.char_indices();
    let head_end = iter
        .nth(head_chars)
        .map(|(i, _)| i)
        .unwrap_or_else(|| text.len());

    // Take the last `tail_chars` codepoints
    let tail_start = text
        .char_indices()
        .rev()
        .nth(tail_chars.saturating_sub(1))
        .map(|(i, _)| i)
        .unwrap_or(0);

    let head = &text[..head_end];
    let tail = &text[tail_start..];

    let omitted = total_chars.saturating_sub(head_chars + tail_chars);
    format!("{head}\n\n[… {omitted} characters omitted …]\n\n{tail}")
}

/// Parse the model's JSON reply. Tolerates surrounding whitespace/code
/// fences from a misbehaving model -- the structured-output contract
/// should make this redundant, but defense in depth is cheap.
pub fn parse_classifier_response(raw: &str) -> Result<ClassifiedKind, String> {
    let trimmed = raw.trim();
    let json_str = if let Some(stripped) = trimmed.strip_prefix("```json") {
        stripped
            .trim_start()
            .strip_suffix("```")
            .unwrap_or(stripped)
            .trim()
    } else if let Some(stripped) = trimmed.strip_prefix("```") {
        stripped
            .trim_start()
            .strip_suffix("```")
            .unwrap_or(stripped)
            .trim()
    } else {
        trimmed
    };

    let parsed: serde_json::Value =
        serde_json::from_str(json_str).map_err(|e| format!("classifier: invalid JSON: {e}"))?;

    let kind = parsed["kind"]
        .as_str()
        .ok_or_else(|| "classifier: missing 'kind'".to_string())?
        .to_string();

    if DocumentKind::from_str(&kind).is_none() {
        return Err(format!("classifier: unknown kind '{kind}' (not in enum)"));
    }

    let confidence = parsed["confidence"]
        .as_f64()
        .ok_or_else(|| "classifier: missing 'confidence'".to_string())?
        .clamp(0.0, 1.0) as f32;

    let rationale = parsed["rationale"].as_str().map(str::to_string);

    let suspicious_flags = parsed["suspicious_flags"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    Ok(ClassifiedKind {
        kind,
        confidence,
        rationale,
        suspicious_flags,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_well_formed_response() {
        let raw = r#"{"kind":"assignment_brief","confidence":0.92,"rationale":"Numbered tasks with grading criteria.","suspicious_flags":[]}"#;
        let r = parse_classifier_response(raw).unwrap();
        assert_eq!(r.kind, "assignment_brief");
        assert!((r.confidence - 0.92).abs() < 1e-6);
        assert_eq!(
            r.rationale.as_deref(),
            Some("Numbered tasks with grading criteria.")
        );
        assert!(r.suspicious_flags.is_empty());
    }

    #[test]
    fn parses_with_code_fence() {
        let raw = "```json\n{\"kind\":\"sample_solution\",\"confidence\":0.81,\"rationale\":\"Worked answer to lab 2.\",\"suspicious_flags\":[\"might_be_solution\"]}\n```";
        let r = parse_classifier_response(raw).unwrap();
        assert_eq!(r.kind, "sample_solution");
        assert_eq!(r.suspicious_flags, vec!["might_be_solution"]);
    }

    #[test]
    fn rejects_unknown_kind() {
        let raw = r#"{"kind":"essay","confidence":0.9,"rationale":"x","suspicious_flags":[]}"#;
        assert!(parse_classifier_response(raw).is_err());
    }

    #[test]
    fn rejects_missing_field() {
        let raw = r#"{"kind":"lecture","rationale":"x"}"#;
        assert!(parse_classifier_response(raw).is_err());
    }

    #[test]
    fn clamps_confidence() {
        let raw = r#"{"kind":"reading","confidence":1.7,"rationale":"x","suspicious_flags":[]}"#;
        let r = parse_classifier_response(raw).unwrap();
        assert!((r.confidence - 1.0).abs() < 1e-6);
    }

    #[test]
    fn truncate_passes_short_text_through() {
        let s = "hello world";
        assert_eq!(truncate_for_classification(s), s);
    }

    #[test]
    fn truncate_keeps_head_and_tail_on_long_text() {
        let s: String = std::iter::repeat_n('a', MAX_EXCERPT_CHARS / 2)
            .chain(std::iter::repeat_n('z', MAX_EXCERPT_CHARS))
            .collect();
        let out = truncate_for_classification(&s);
        // Truncated string must be shorter than the input.
        assert!(out.chars().count() < s.chars().count());
        assert!(out.starts_with('a'));
        assert!(out.contains("characters omitted"));
        assert!(out.ends_with('z'));
    }

    #[test]
    fn filename_hints_extracts_swedish_assignment() {
        let h = filename_hints("ovningsuppgift3_vt25.pdf");
        assert!(h.iter().any(|s| s.contains("assignment_brief")));
    }

    #[test]
    fn filename_hints_extracts_diacritic_assignment() {
        let h = filename_hints("Övningsuppgift_4.pdf");
        assert!(h.iter().any(|s| s.contains("assignment_brief")));
    }

    #[test]
    fn filename_hints_extracts_swedish_solution() {
        let h = filename_hints("Lösningsforslag_lab2.pdf");
        assert!(h.iter().any(|s| s.contains("sample_solution")));
    }

    #[test]
    fn filename_hints_extracts_dsv_lecture_pattern() {
        let h = filename_hints("F18_OO.pdf");
        assert!(h.iter().any(|s| s.contains("lecture")));
    }

    #[test]
    fn filename_hints_skips_non_lecture_f_words() {
        let h = filename_hints("facit_kapitel3.pdf");
        // "facit" should fire the solution hint, but the F-prefix
        // detector should NOT misread "facit" as a lecture marker.
        assert!(h.iter().any(|s| s.contains("sample_solution")));
        assert!(!h.iter().any(|s| s.contains("F<number>")));
    }

    #[test]
    fn filename_hints_returns_empty_for_generic_name() {
        let h = filename_hints("notes.pdf");
        assert!(h.is_empty());
    }

    #[test]
    fn truncate_is_utf8_safe_with_multibyte() {
        // Each "ä" is 2 bytes in UTF-8. Build a long string of them so
        // we'd hit a byte mid-codepoint if we used naive slicing.
        let s: String = "ä".repeat(MAX_EXCERPT_CHARS * 2);
        let out = truncate_for_classification(&s);
        // Must not panic and must remain valid UTF-8.
        assert!(out.is_char_boundary(0));
        assert!(out.starts_with('ä'));
    }
}
