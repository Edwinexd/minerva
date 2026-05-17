//! Cache + lazy regeneration for the chat empty-state starter
//! prompts. Three paths, in order of frequency:
//!
//!   1. Cache row exists AND was checked within `STALENESS`:
//!      return cached, no Postgres writes beyond the SELECT.
//!   2. Row stale (or missing) AND the course's latest-3 ready
//!      docs match cached `source_doc_ids`: bump
//!      `last_checked_at`, return cached. No LLM work.
//!   3. Row stale AND drift detected (or no row): LLM regen,
//!      tokens billed under `suggested_questions` in
//!      `course_token_usage`. Courses with 0 ready docs persist
//!      `questions = []` and skip the LLM entirely.
//!
//! Concurrency: when we regen, the three per-doc Qdrant scrolls
//! fan out in parallel; the LLM call dominates wall-clock either
//! way, but the scrolls were sequential in the first cut.

use axum::extract::{Extension, Path, State};
use axum::routing::get;
use axum::{Json, Router};
use chrono::Duration;
use futures::future::join_all;
use minerva_core::models::User;
use minerva_db::queries::course_token_usage::CATEGORY_SUGGESTED_QUESTIONS;
use minerva_db::queries::documents::ReadyDocSummary;
use serde::Serialize;
use std::fmt::Write;
use uuid::Uuid;

use crate::error::AppError;
use crate::state::AppState;
use crate::strategy::common::{
    cerebras_request_with_retry, record_cerebras_usage, scroll_doc_chunks,
};

/// Same llama3.1-8b the document classifier uses; JSON-schema
/// constrained output is well within its range and prompt cost is
/// dominated by the per-doc excerpts.
const SUGGEST_MODEL: &str = "llama3.1-8b";

const SOURCE_DOC_LIMIT: i64 = 3;
const CHUNKS_PER_DOC: usize = 3;
/// Per-chunk char cap; bigger pulls more prompt tokens with
/// diminishing question quality.
const MAX_CHARS_PER_CHUNK: usize = 600;
/// Qdrant scroll batch per doc. We only consume the first
/// `CHUNKS_PER_DOC` by `chunk_index`; the rest is overhead.
const SCROLL_LIMIT: u32 = 16;
const STALENESS: Duration = Duration::hours(24);
const QUESTIONS_PER_REGEN: usize = 3;

pub fn router() -> Router<AppState> {
    Router::new().route("/suggested-questions", get(handler_shib))
}

#[derive(Serialize)]
pub struct SuggestedQuestionsResponse {
    pub questions: Vec<String>,
}

async fn handler_shib(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(course_id): Path<Uuid>,
) -> Result<Json<SuggestedQuestionsResponse>, AppError> {
    let course = minerva_db::queries::courses::find_by_id(&state.db, course_id)
        .await?
        .ok_or(AppError::NotFound)?;
    if course.owner_id != user.id
        && !minerva_db::queries::courses::is_member(&state.db, course_id, user.id).await?
    {
        return Err(AppError::Forbidden);
    }

    let questions = get_or_refresh(&state, course_id).await?;
    Ok(Json(SuggestedQuestionsResponse { questions }))
}

/// Shared entry point used by both the Shibboleth route and the
/// embed mirror. Caller is responsible for proving the user has
/// access to `course_id`.
pub async fn get_or_refresh(state: &AppState, course_id: Uuid) -> Result<Vec<String>, AppError> {
    let cached = minerva_db::queries::course_suggested_questions::get(&state.db, course_id).await?;
    if let Some(row) = cached.as_ref() {
        if chrono::Utc::now() - row.last_checked_at < STALENESS {
            return Ok(row.questions.0.clone());
        }
    }

    let docs = minerva_db::queries::documents::list_latest_ready_by_course(
        &state.db,
        course_id,
        SOURCE_DOC_LIMIT,
    )
    .await?;
    let current_ids: Vec<Uuid> = docs.iter().map(|d| d.id).collect();

    if let Some(row) = cached.as_ref() {
        if row.source_doc_ids == current_ids {
            minerva_db::queries::course_suggested_questions::touch_checked(&state.db, course_id)
                .await?;
            return Ok(row.questions.0.clone());
        }
    }

    let questions = regenerate(state, course_id, &docs).await?;
    minerva_db::queries::course_suggested_questions::upsert(
        &state.db,
        course_id,
        &questions,
        &current_ids,
        SUGGEST_MODEL,
    )
    .await?;
    Ok(questions)
}

async fn regenerate(
    state: &AppState,
    course_id: Uuid,
    docs: &[ReadyDocSummary],
) -> Result<Vec<String>, AppError> {
    // Zero ready docs: skip the LLM entirely. The empty array is
    // still cached so the next read goes through the fast path.
    if docs.is_empty() {
        return Ok(Vec::new());
    }

    let collection = minerva_ingest::pipeline::collection_name_for_course(&state.db, course_id)
        .await
        .map_err(AppError::Database)?;

    // Fan out per-doc scrolls; sequentially this is ~3x the
    // wall-clock for no good reason.
    let chunk_results = join_all(
        docs.iter()
            .map(|doc| fetch_head_chunks(&state.qdrant, &collection, doc.id)),
    )
    .await;

    let mut grounding = String::new();
    for (doc, chunks) in docs.iter().zip(chunk_results.iter()) {
        let _ = write!(&mut grounding, "## {}", doc.filename);
        if let Some(kind) = doc.kind.as_deref() {
            let _ = write!(&mut grounding, " ({kind})");
        }
        grounding.push('\n');
        if chunks.is_empty() {
            grounding.push_str("(no excerpts available)\n\n");
            continue;
        }
        for chunk in chunks {
            grounding.push_str(chunk);
            grounding.push_str("\n\n");
        }
    }

    let body = serde_json::json!({
        "model": SUGGEST_MODEL,
        "temperature": 0.3,
        "messages": [
            { "role": "system", "content": SYSTEM_PROMPT },
            { "role": "user", "content": format!("{USER_TEMPLATE_PREFIX}\n\n{grounding}") },
        ],
        "response_format": {
            "type": "json_schema",
            "json_schema": {
                "name": "suggested_questions",
                "strict": true,
                "schema": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["questions"],
                    "properties": {
                        "questions": {
                            "type": "array",
                            "items": { "type": "string" },
                            "minItems": QUESTIONS_PER_REGEN,
                            "maxItems": QUESTIONS_PER_REGEN,
                        }
                    }
                }
            }
        }
    });

    let response =
        cerebras_request_with_retry(&state.http_client, &state.config.cerebras_api_key, &body)
            .await
            .map_err(AppError::Internal)?;
    let payload: serde_json::Value = response
        .json()
        .await
        .map_err(|e| AppError::Internal(format!("suggested-questions: response not JSON: {e}")))?;

    record_cerebras_usage(
        &state.db,
        course_id,
        CATEGORY_SUGGESTED_QUESTIONS,
        SUGGEST_MODEL,
        &payload,
    )
    .await;

    let raw = payload["choices"][0]["message"]["content"]
        .as_str()
        .ok_or_else(|| {
            AppError::Internal(format!(
                "suggested-questions: missing choices[0].message.content; got: {payload}"
            ))
        })?;

    #[derive(serde::Deserialize)]
    struct ModelReply {
        questions: Vec<String>,
    }
    let parsed: ModelReply = serde_json::from_str(raw.trim())
        .map_err(|e| AppError::Internal(format!("suggested-questions: invalid JSON: {e}")))?;

    Ok(parsed
        .questions
        .into_iter()
        .map(|q| q.trim().to_string())
        .filter(|s| !s.is_empty())
        .take(QUESTIONS_PER_REGEN)
        .collect())
}

/// Pull the first `CHUNKS_PER_DOC` chunks (by `chunk_index`) and
/// truncate each to `MAX_CHARS_PER_CHUNK`. Soft-fails to an empty
/// vec on Qdrant errors; the LLM still gets the filename + kind.
async fn fetch_head_chunks(
    qdrant: &qdrant_client::Qdrant,
    collection_name: &str,
    doc_id: Uuid,
) -> Vec<String> {
    match scroll_doc_chunks(qdrant, collection_name, doc_id, SCROLL_LIMIT).await {
        Ok(by_index) => by_index
            .into_values()
            .take(CHUNKS_PER_DOC)
            .map(|t| head_truncate(&t, MAX_CHARS_PER_CHUNK))
            .collect(),
        Err(e) => {
            tracing::warn!("suggested-questions: qdrant scroll failed for doc {doc_id}: {e}");
            Vec::new()
        }
    }
}

/// UTF-8-safe head-truncate with an ellipsis suffix when cut.
fn head_truncate(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    let head: String = s.chars().take(max_chars).collect();
    format!("{head}…")
}

const SYSTEM_PROMPT: &str = "\
You generate short starter questions a student might naturally ask \
about a course's most recent materials. Output exactly three \
questions. Each must be:
  * grounded in the supplied excerpts (not generic),
  * phrased as a real first-message a student would send to a TA,
  * <= 12 words,
  * written in the same language as the excerpts.

Avoid yes/no questions, avoid meta questions about the course \
itself (\"what is this course about\"), avoid administrative \
questions (deadlines, grading), avoid repeating wording across \
the three. Return JSON only.";

const USER_TEMPLATE_PREFIX: &str = "\
Here are excerpts from the course's three most recently added \
documents. Generate three starter questions in the response \
language matching the excerpts.";
