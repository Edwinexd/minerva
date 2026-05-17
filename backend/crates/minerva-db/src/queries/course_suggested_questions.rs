//! Cache table for the LLM-generated starter questions shown on
//! the chat empty state. CRUD only; the cache lifecycle (when to
//! regen, when to bump `last_checked_at`) lives in
//! `minerva-server/src/routes/suggested_questions.rs`.

use chrono::{DateTime, Utc};
use sqlx::types::Json;
use sqlx::PgPool;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct SuggestedQuestionsRow {
    pub course_id: Uuid,
    pub questions: Json<Vec<String>>,
    pub source_doc_ids: Vec<Uuid>,
    pub model: String,
    pub generated_at: DateTime<Utc>,
    pub last_checked_at: DateTime<Utc>,
}

pub async fn get(
    db: &PgPool,
    course_id: Uuid,
) -> Result<Option<SuggestedQuestionsRow>, sqlx::Error> {
    sqlx::query_as!(
        SuggestedQuestionsRow,
        r#"SELECT
              course_id,
              questions       AS "questions: Json<Vec<String>>",
              source_doc_ids,
              model,
              generated_at,
              last_checked_at
           FROM course_suggested_questions
           WHERE course_id = $1"#,
        course_id,
    )
    .fetch_optional(db)
    .await
}

/// Bump `last_checked_at` without rewriting the questions; called
/// when the staleness gate fires but the source set hasn't drifted.
pub async fn touch_checked(db: &PgPool, course_id: Uuid) -> Result<(), sqlx::Error> {
    sqlx::query!(
        r#"UPDATE course_suggested_questions
           SET last_checked_at = NOW()
           WHERE course_id = $1"#,
        course_id,
    )
    .execute(db)
    .await
    .map(|_| ())
}

/// Insert-or-replace; the per-course PK collapses concurrent
/// regenerations to a single row.
pub async fn upsert(
    db: &PgPool,
    course_id: Uuid,
    questions: &[String],
    source_doc_ids: &[Uuid],
    model: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query!(
        r#"INSERT INTO course_suggested_questions
              (course_id, questions, source_doc_ids, model, generated_at, last_checked_at)
           VALUES ($1, $2, $3, $4, NOW(), NOW())
           ON CONFLICT (course_id) DO UPDATE
              SET questions       = EXCLUDED.questions,
                  source_doc_ids  = EXCLUDED.source_doc_ids,
                  model           = EXCLUDED.model,
                  generated_at    = NOW(),
                  last_checked_at = NOW()"#,
        course_id,
        Json(questions) as _,
        source_doc_ids,
        model,
    )
    .execute(db)
    .await
    .map(|_| ())
}
