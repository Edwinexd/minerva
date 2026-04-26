//! Per-course token usage log for the KG / extraction-guard
//! pipeline. Append-only event log; aggregates are built on demand
//! from the indexed table rather than maintained as denormalised
//! counters.
//!
//! Categories the rest of the system writes here (free-form
//! strings -- no enum so we can add categories without touching
//! this module):
//!   * `document_classifier`
//!   * `linker`
//!   * `adversarial_filter`
//!   * `extraction_guard`
//!
//! Embeddings are deliberately not tracked -- pocket change
//! relative to LLM calls (per the operational policy).
//!
//! No spending limits in this iteration. The dashboard shows
//! usage; nothing 429s on threshold.

use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::PgPool;
use uuid::Uuid;

// ── category constants (public so call sites import them) ──────────
//
// Free-form text in the column; these constants exist so the
// strings stay typo-stable across the four call sites and so
// adding a new category is one place. The dashboard groups by the
// raw string.

pub const CATEGORY_DOCUMENT_CLASSIFIER: &str = "document_classifier";
pub const CATEGORY_LINKER: &str = "linker";
pub const CATEGORY_ADVERSARIAL_FILTER: &str = "adversarial_filter";
pub const CATEGORY_EXTRACTION_GUARD: &str = "extraction_guard";

/// Insert a single usage row. Best-effort: callers log a warning
/// on error and continue -- we never block a chat / ingest path
/// because tracking failed.
pub async fn record(
    db: &PgPool,
    course_id: Uuid,
    category: &str,
    model: &str,
    prompt_tokens: i32,
    completion_tokens: i32,
) -> Result<(), sqlx::Error> {
    sqlx::query!(
        r#"
        INSERT INTO course_token_usage
            (course_id, category, model, prompt_tokens, completion_tokens)
        VALUES ($1, $2, $3, $4, $5)
        "#,
        course_id,
        category,
        model,
        prompt_tokens,
        completion_tokens
    )
    .execute(db)
    .await
    .map(|_| ())
}

/// Aggregate row returned by the dashboard query: one bucket per
/// (category, model) with the sum of tokens since `since`. The
/// model split lets the dashboard show "of the linker's spend, X%
/// went to gpt-oss-120b vs Y% to llama3.1-8b" if we ever mix.
#[derive(Debug, Clone, Serialize)]
pub struct CategoryAggregate {
    pub category: String,
    pub model: String,
    pub call_count: i64,
    pub prompt_tokens: i64,
    pub completion_tokens: i64,
}

/// "Since `since`, how much did each (category, model) burn for
/// this course?" The primary dashboard query.
pub async fn aggregate_by_category_for_course(
    db: &PgPool,
    course_id: Uuid,
    since: DateTime<Utc>,
) -> Result<Vec<CategoryAggregate>, sqlx::Error> {
    let rows = sqlx::query!(
        r#"
        SELECT
            category,
            model,
            COUNT(*)                              AS "call_count!",
            COALESCE(SUM(prompt_tokens), 0)::int8 AS "prompt_tokens!",
            COALESCE(SUM(completion_tokens), 0)::int8 AS "completion_tokens!"
        FROM course_token_usage
        WHERE course_id = $1 AND created_at >= $2
        GROUP BY category, model
        ORDER BY category, model
        "#,
        course_id,
        since
    )
    .fetch_all(db)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| CategoryAggregate {
            category: r.category,
            model: r.model,
            call_count: r.call_count,
            prompt_tokens: r.prompt_tokens,
            completion_tokens: r.completion_tokens,
        })
        .collect())
}
