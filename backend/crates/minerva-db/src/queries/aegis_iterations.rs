//! Aegis live-iteration history. Persists every debounced analyze
//! call the frontend fires while a study participant is editing a
//! draft; see migration `20260505000009_aegis_iterations.sql` for
//! schema and the rationale on which conversations get captured.
//!
//! `insert` is fire-and-forget from the chat path: the analyzer
//! itself is best-effort, so a DB blip here mustn't break analyze.
//! `list_for_conversation` is used by the study export to pull the
//! full per-task iteration trace.

use sqlx::PgPool;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct AegisIterationRow {
    pub id: Uuid,
    pub conversation_id: Uuid,
    pub draft_text: String,
    pub suggestions: serde_json::Value,
    pub mode: String,
    pub model_used: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

pub async fn insert(
    db: &PgPool,
    conversation_id: Uuid,
    draft_text: &str,
    suggestions: &serde_json::Value,
    mode: &str,
    model_used: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query!(
        r#"INSERT INTO aegis_iterations (conversation_id, draft_text, suggestions, mode, model_used)
        VALUES ($1, $2, $3, $4, $5)"#,
        conversation_id,
        draft_text,
        suggestions,
        mode,
        model_used,
    )
    .execute(db)
    .await?;
    Ok(())
}

pub async fn list_for_conversation(
    db: &PgPool,
    conversation_id: Uuid,
) -> Result<Vec<AegisIterationRow>, sqlx::Error> {
    sqlx::query_as!(
        AegisIterationRow,
        r#"SELECT id, conversation_id, draft_text, suggestions, mode, model_used, created_at
        FROM aegis_iterations
        WHERE conversation_id = $1
        ORDER BY created_at ASC"#,
        conversation_id,
    )
    .fetch_all(db)
    .await
}
