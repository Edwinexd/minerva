//! Cerebras gpt-oss-120b-backed document classifier.

use async_trait::async_trait;
use minerva_ingest::classifier::{ClassifiedKind, Classifier};
use sqlx::PgPool;
use uuid::Uuid;

use super::prompts::{CLASSIFIER_SYSTEM_PROMPT, CLASSIFIER_USER_TEMPLATE};
use super::types::{DocumentKind, ALL_KINDS};
use crate::strategy::common::{cerebras_request_with_retry, record_cerebras_usage};
use minerva_db::queries::course_token_usage::CATEGORY_DOCUMENT_CLASSIFIER;

/// Cerebras model name for ingest-time classification work. Open-weights
/// gpt-oss-120b; strong instruction-following, reasoning_effort
/// supported, structured outputs supported.
const CLASSIFIER_MODEL: &str = "gpt-oss-120b";

/// Confidence below this triggers a retry with reasoning_effort = "high".
const RETRY_CONFIDENCE_THRESHOLD: f32 = 0.6;

/// Soft cap on excerpt length sent to the model. Head/tail split so
/// the model sees both the introductory framing and any "submit /
/// due / answer" footer.
///
/// Tightened from 60K to 10K (chars, ~2.5K tokens) after the
/// dashboard surfaced 5-6K avg prompt tokens per call across hundreds
/// of classifications. 9-class doc classification rarely needs more
/// than the first 2-3 pages plus the footer to make a confident call:
/// the discriminating signal (numbered tasks, "submit by", "frivillig",
/// "worked solution", "syllabus / schedule") shows up early. The few
/// docs that genuinely need more context will fall below the
/// confidence threshold and pick up `reasoning_effort=high` on the
/// retry, which is fine; that path stays cheap because it happens
/// rarely now that flags don't trigger retries.
const MAX_EXCERPT_CHARS: usize = 10_000;
const HEAD_FRACTION: f64 = 0.85;

pub struct CerebrasClassifier {
    http: reqwest::Client,
    api_key: String,
    /// DB handle so each Cerebras call can record its token spend
    /// to `course_token_usage`. Cloned cheaply per ingest call.
    db: PgPool,
}

impl CerebrasClassifier {
    pub fn new(http: reqwest::Client, api_key: String, db: PgPool) -> Self {
        Self { http, api_key, db }
    }

    async fn call(
        &self,
        course_id: Uuid,
        mime_type: &str,
        excerpt: &str,
        reasoning_effort: &str,
    ) -> Result<ClassifiedKind, String> {
        // Filename is intentionally NOT in the prompt; see the
        // CLASSIFIER_USER_TEMPLATE doc comment. Classifier must decide
        // from content alone.
        let user = CLASSIFIER_USER_TEMPLATE
            .replace("{mime_type}", mime_type)
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

        // Best-effort token-spend bookkeeping. Both the low-effort
        // and high-effort retry calls land here; the dashboard
        // sums them as one bucket per (category, model).
        record_cerebras_usage(
            &self.db,
            course_id,
            CATEGORY_DOCUMENT_CLASSIFIER,
            CLASSIFIER_MODEL,
            &payload,
        )
        .await;

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
        course_id: Uuid,
        filename: &str,
        mime_type: &str,
        text: &str,
    ) -> Result<ClassifiedKind, String> {
        // `filename` arrives here as part of the Classifier trait but is
        // ONLY used for log lines; we deliberately do not feed it to
        // the model. Filenames in real DSV courses are too unreliable
        // (stale templates, copy/pasted names, F-numbers that don't
        // match content). Pure content classification.

        // Zero-text fast-path: if the extractor produced nothing
        // (URL stubs, scanned PDFs without OCR, unsupported types,
        // etc.) there's nothing for the model to read. Return
        // `unknown` with low confidence and the "no_text_extracted"
        // flag. The chat-time partition keeps unknowns out of context
        // anyway, so this is the safe default.
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

        // First pass; cheap, low effort.
        let initial = self
            .call(course_id, mime_type, &excerpt, "low")
            .await
            .map_err(|e| format!("classifier: low-effort call failed: {e}"))?;

        // Retry on UNCERTAINTY only; i.e. low confidence. Earlier
        // versions also retried whenever `suspicious_flags` was
        // non-empty, but the system prompt actively encourages flags
        // as a UI hint ("might_be_solution",
        // "ambiguous_between_assignment_and_lab",
        // "language_mixed_swedish_english"); so flags fired on
        // most nuanced docs and the second high-effort call doubled
        // the classifier's token spend without correspondingly
        // changing the kind decision. Flags now flow through to the
        // teacher unchanged; only confidence drives retries.
        if initial.confidence >= RETRY_CONFIDENCE_THRESHOLD {
            return Ok(initial);
        }

        tracing::info!(
            "classifier: re-running {} with reasoning_effort=high (initial: kind={} confidence={:.2} flags={:?})",
            filename,
            initial.kind,
            initial.confidence,
            initial.suspicious_flags,
        );

        match self.call(course_id, mime_type, &excerpt, "high").await {
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
/// fences from a misbehaving model; the structured-output contract
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
