//! Primitives shared between the chat/retrieval path (`strategy`) and
//! the ingest-time classifier (`classification`): the Cerebras HTTP
//! request helper + retry, per-course token-usage recording, the
//! Qdrant payload field extractors, and the `RagChunk` retrieval type.
//!
//! These live here (not in `strategy::common`, which is axum-coupled
//! for SSE streaming) so the worker / scheduler can run classification
//! and retrieval primitives without linking axum or the chat route
//! tree. `strategy::common` re-exports them for the chat strategies.

use std::collections::HashMap;

use qdrant_client::qdrant::value::Kind;
use reqwest::Response;

pub mod cost;
pub mod provider;

pub use cost::cost_usd;
pub use provider::{
    AnthropicProvider, ChatDelta, ChatProvider, ChatRequest, ChatUsage, LlmRegistry,
    OpenAiCompatibleProvider, ProviderKind,
};

/// Production Cerebras chat-completions endpoint.
pub const CEREBRAS_CHAT_COMPLETIONS_URL: &str = "https://api.cerebras.ai/v1/chat/completions";

/// Maximum number of retries for transient Cerebras API errors (5XX, timeouts).
const MAX_RETRIES: u32 = 3;

/// Initial backoff delay between retries.
const INITIAL_BACKOFF: std::time::Duration = std::time::Duration::from_millis(500);

/// A chunk returned by RAG lookup, carrying metadata for display filtering.
///
/// `kind` mirrors the document's classification (lecture, assignment_brief,
/// sample_solution, ...). It is sourced from the Qdrant payload (stamped at
/// embed time by `minerva_pipeline::pipeline`) so we don't need a per-chunk
/// DB roundtrip on hot retrieval paths. Older points without `kind` (i.e.
/// stale data, or vectors uploaded by an out-of-date worker) come through
/// as `None`; the partition logic treats those as "context" with a DB
/// safety check downstream via `unclassified_doc_ids`.
#[derive(Debug, Clone, PartialEq)]
pub struct RagChunk {
    pub document_id: String,
    pub filename: String,
    pub text: String,
    pub kind: Option<String>,
    pub score: f32,
}

impl RagChunk {
    /// Format for inclusion in the LLM system prompt (always full text).
    pub fn formatted(&self) -> String {
        format!("[Source: {}]\n{}", self.filename, self.text)
    }
}

/// Extract a string field from a Qdrant point payload.
pub fn payload_string(
    payload: &HashMap<String, qdrant_client::qdrant::Value>,
    key: &str,
) -> Option<String> {
    match payload.get(key).and_then(|v| v.kind.as_ref()) {
        Some(Kind::StringValue(s)) => Some(s.clone()),
        _ => None,
    }
}

/// Extract an integer field from a Qdrant point payload.
pub fn payload_int(
    payload: &HashMap<String, qdrant_client::qdrant::Value>,
    key: &str,
) -> Option<i64> {
    match payload.get(key).and_then(|v| v.kind.as_ref()) {
        Some(Kind::IntegerValue(i)) => Some(*i),
        _ => None,
    }
}

/// Send a request to the Cerebras API with retry on 5XX / network errors.
/// Returns the successful response or the last error as a formatted string.
pub async fn cerebras_request_with_retry(
    client: &reqwest::Client,
    api_key: &str,
    body: &serde_json::Value,
) -> Result<Response, String> {
    cerebras_request_with_retry_to(client, CEREBRAS_CHAT_COMPLETIONS_URL, api_key, body).await
}

/// Same as `cerebras_request_with_retry` but posts to `url` instead of the
/// production endpoint. Exists so integration tests can point FLARE at a
/// mock server without exposing URL-override plumbing throughout the rest
/// of the codebase.
pub async fn cerebras_request_with_retry_to(
    client: &reqwest::Client,
    url: &str,
    api_key: &str,
    body: &serde_json::Value,
) -> Result<Response, String> {
    let mut last_err = String::new();

    for attempt in 0..=MAX_RETRIES {
        if attempt > 0 {
            let backoff = INITIAL_BACKOFF * 2u32.pow(attempt - 1);
            tracing::warn!(
                "cerebras: retry {}/{} after {:?}",
                attempt,
                MAX_RETRIES,
                backoff
            );
            tokio::time::sleep(backoff).await;
        }

        let result = client
            .post(url)
            .header("Authorization", format!("Bearer {}", api_key))
            .json(body)
            .send()
            .await;

        match result {
            Ok(response) => {
                let status = response.status();
                if status.is_success() {
                    return Ok(response);
                }
                if status.is_server_error() {
                    let body_text = response.text().await.unwrap_or_default();
                    last_err = format!("Cerebras API error {}: {}", status, body_text);
                    tracing::warn!("cerebras: {}", last_err);
                    continue;
                }
                // Client errors (4XX) are not retryable
                let body_text = response.text().await.unwrap_or_default();
                return Err(format!("Cerebras API error {}: {}", status, body_text));
            }
            Err(e) if e.is_timeout() || e.is_connect() => {
                last_err = format!("Request failed: {}", e);
                tracing::warn!("cerebras: {}", last_err);
                continue;
            }
            Err(e) => {
                return Err(format!("Request failed: {}", e));
            }
        }
    }

    Err(last_err)
}

/// Pull `(prompt_tokens, completion_tokens)` out of a parsed
/// Cerebras chat-completions response payload. Returns `None`
/// when the usage block is missing or malformed; callers should
/// just skip token recording in that case (we never block a chat
/// path because tracking failed).
pub fn extract_cerebras_usage(payload: &serde_json::Value) -> Option<(i32, i32)> {
    let usage = payload.get("usage")?;
    let p = usage.get("prompt_tokens")?.as_i64()?;
    let c = usage.get("completion_tokens")?.as_i64()?;
    Some((p as i32, c as i32))
}

/// Convenience wrapper: pull usage out of `payload` and record a
/// `course_token_usage` row. Best-effort; logs a warning on
/// either missing-usage or DB error and returns silently. Used
/// from every classification call site so they don't all repeat
/// the same boilerplate.
pub async fn record_cerebras_usage(
    db: &sqlx::PgPool,
    course_id: uuid::Uuid,
    category: &'static str,
    model: &str,
    payload: &serde_json::Value,
) {
    let Some((prompt_tokens, completion_tokens)) = extract_cerebras_usage(payload) else {
        tracing::warn!(
            "course_token_usage: skipping record for course={} category={}: usage block missing/malformed",
            course_id,
            category
        );
        return;
    };
    // The row records model + tokens; USD cost is derived on read by
    // joining the model's current rate (see usage cost queries), so a
    // later re-price never rewrites historical spend.
    if let Err(e) = minerva_db::queries::course_token_usage::record(
        db,
        course_id,
        category,
        model,
        prompt_tokens,
        completion_tokens,
    )
    .await
    {
        tracing::warn!(
            "course_token_usage: insert failed for course={} category={}: {}",
            course_id,
            category,
            e
        );
    }
}
