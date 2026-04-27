//! Aegis prompt-coaching analyses. One row per user message that
//! the analyzer produced suggestions for. See migration
//! `20260427000004_aegis_suggestions.sql` for the schema.
//!
//! The row carries an opaque JSONB array of suggestions so adding,
//! reordering, or renaming a suggestion field never forces a
//! migration. The Rust side validates shape at insert by going
//! through `serde` -- the column is `NOT NULL` with default `[]`
//! so reads are total and "no suggestions to make" reads back
//! identically to "the analyzer found nothing worth saying".
//!
//! Two layers like every other queries module:
//!
//!   1. `insert` -- the route writes one row when the user sends a
//!      message AND had a non-empty live verdict cached client-
//!      side. Idempotent under `(message_id)` UNIQUE: a retry after
//!      a transient failure replaces the previous row.
//!   2. `list_for_conversation` -- the chat detail route pulls
//!      every analysis for a conversation in one query so the
//!      Feedback panel's history list comes from the same payload
//!      the messages do.

use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct PromptAnalysisRow {
    pub id: Uuid,
    pub message_id: Uuid,
    /// Opaque JSONB; the application layer deserialises into its
    /// `Vec<AegisSuggestion>` shape. We keep it as `Value` here so
    /// schema evolution (add a `severity`, etc.) doesn't ripple
    /// into this crate.
    pub suggestions: serde_json::Value,
    /// `"beginner"` or `"expert"` -- enforced by CHECK constraint
    /// at the table level. Persisted so a History row can carry
    /// the calibration label it was generated under.
    pub mode: String,
    pub model_used: String,
    pub created_at: DateTime<Utc>,
}

/// All fields the caller wants to persist for one user message.
/// `suggestions` is borrowed `serde_json::Value` so the route's
/// strongly-typed `Vec<AegisSuggestion>` can be serialised once
/// (in `to_value`) and handed in by reference.
#[derive(Debug, Clone)]
pub struct PromptAnalysisInsert<'a> {
    pub message_id: Uuid,
    pub suggestions: &'a serde_json::Value,
    pub mode: &'a str,
    pub model_used: &'a str,
}

/// Insert (or replace, on retry) one analysis row. ON CONFLICT
/// (message_id) DO UPDATE so a transient analyzer retry doesn't
/// trip the UNIQUE constraint -- the latest verdict for a turn is
/// what the panel shows.
pub async fn insert(
    db: &PgPool,
    a: PromptAnalysisInsert<'_>,
) -> Result<PromptAnalysisRow, sqlx::Error> {
    sqlx::query_as!(
        PromptAnalysisRow,
        r#"
        INSERT INTO prompt_analyses (message_id, suggestions, mode, model_used)
        VALUES ($1, $2, $3, $4)
        ON CONFLICT (message_id) DO UPDATE SET
            suggestions = EXCLUDED.suggestions,
            mode = EXCLUDED.mode,
            model_used = EXCLUDED.model_used
        RETURNING id, message_id, suggestions, mode, model_used, created_at
        "#,
        a.message_id,
        a.suggestions,
        a.mode,
        a.model_used,
    )
    .fetch_one(db)
    .await
}

/// All analyses for messages in `conversation_id`, oldest first.
/// Joined through `messages` so we can scope by conversation
/// without storing a redundant conversation_id column on the
/// analyses table.
pub async fn list_for_conversation(
    db: &PgPool,
    conversation_id: Uuid,
) -> Result<Vec<PromptAnalysisRow>, sqlx::Error> {
    sqlx::query_as!(
        PromptAnalysisRow,
        r#"
        SELECT
            a.id, a.message_id, a.suggestions, a.mode, a.model_used, a.created_at
        FROM prompt_analyses a
        JOIN messages m ON m.id = a.message_id
        WHERE m.conversation_id = $1
        ORDER BY m.created_at ASC, a.created_at ASC
        "#,
        conversation_id,
    )
    .fetch_all(db)
    .await
}
