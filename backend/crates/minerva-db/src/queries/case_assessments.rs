//! Structured credit transfer assessments; see migration
//! `20261104000006_case_assessments.sql`.

use sqlx::PgPool;
use uuid::Uuid;

pub struct NewAssessment<'a> {
    pub message_id: Uuid,
    pub conversation_id: Uuid,
    pub assessment: &'a serde_json::Value,
    pub best_code: Option<&'a str>,
    pub best_verdict: Option<&'a str>,
    pub candidate_count: i32,
}

pub async fn insert(db: &PgPool, a: &NewAssessment<'_>) -> Result<(), sqlx::Error> {
    sqlx::query!(
        r#"INSERT INTO case_assessments
               (message_id, conversation_id, assessment, best_code, best_verdict, candidate_count)
           VALUES ($1, $2, $3, $4, $5, $6)"#,
        a.message_id,
        a.conversation_id,
        a.assessment,
        a.best_code,
        a.best_verdict,
        a.candidate_count,
    )
    .execute(db)
    .await?;
    Ok(())
}

pub struct AssessmentRow {
    pub message_id: Uuid,
    pub assessment: serde_json::Value,
}

/// Every assessment in a conversation, keyed by the message it belongs to.
pub async fn list_for_conversation(
    db: &PgPool,
    conversation_id: Uuid,
) -> Result<Vec<AssessmentRow>, sqlx::Error> {
    sqlx::query_as!(
        AssessmentRow,
        r#"SELECT message_id, assessment FROM case_assessments
           WHERE conversation_id = $1 ORDER BY created_at"#,
        conversation_id,
    )
    .fetch_all(db)
    .await
}
