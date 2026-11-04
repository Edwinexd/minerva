//! Files attached to one conversation; see migration
//! `20261104000004_conversation_attachments.sql`.

use sqlx::PgPool;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct AttachmentRow {
    pub id: Uuid,
    pub conversation_id: Uuid,
    pub uploaded_by: Uuid,
    pub filename: String,
    pub mime_type: String,
    pub size_bytes: i64,
    pub content_hash: String,
    pub extracted_text: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

pub struct NewAttachment<'a> {
    pub id: Uuid,
    pub conversation_id: Uuid,
    pub uploaded_by: Uuid,
    pub filename: &'a str,
    pub mime_type: &'a str,
    pub size_bytes: i64,
    pub content_hash: &'a str,
    pub extracted_text: &'a str,
}

pub async fn insert(db: &PgPool, a: &NewAttachment<'_>) -> Result<AttachmentRow, sqlx::Error> {
    sqlx::query_as!(
        AttachmentRow,
        r#"INSERT INTO conversation_attachments
               (id, conversation_id, uploaded_by, filename, mime_type, size_bytes,
                content_hash, extracted_text)
           VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
           RETURNING id, conversation_id, uploaded_by, filename, mime_type, size_bytes,
                     content_hash, extracted_text, created_at"#,
        a.id,
        a.conversation_id,
        a.uploaded_by,
        a.filename,
        a.mime_type,
        a.size_bytes,
        a.content_hash,
        a.extracted_text,
    )
    .fetch_one(db)
    .await
}

/// A conversation's attachments, oldest first (the order the model sees).
pub async fn list(db: &PgPool, conversation_id: Uuid) -> Result<Vec<AttachmentRow>, sqlx::Error> {
    sqlx::query_as!(
        AttachmentRow,
        r#"SELECT id, conversation_id, uploaded_by, filename, mime_type, size_bytes,
                  content_hash, extracted_text, created_at
           FROM conversation_attachments
           WHERE conversation_id = $1
           ORDER BY created_at, id"#,
        conversation_id,
    )
    .fetch_all(db)
    .await
}

/// Delete one attachment of a conversation. Returns the row so the caller
/// can remove its file.
pub async fn delete(
    db: &PgPool,
    conversation_id: Uuid,
    id: Uuid,
) -> Result<Option<AttachmentRow>, sqlx::Error> {
    sqlx::query_as!(
        AttachmentRow,
        r#"DELETE FROM conversation_attachments
           WHERE conversation_id = $1 AND id = $2
           RETURNING id, conversation_id, uploaded_by, filename, mime_type, size_bytes,
                     content_hash, extracted_text, created_at"#,
        conversation_id,
        id,
    )
    .fetch_optional(db)
    .await
}

/// A conversation that has outlived its workspace's retention period,
/// with the attachment ids whose files must be removed with it.
pub struct ExpiredConversation {
    pub conversation_id: Uuid,
    pub workspace_id: Uuid,
    pub attachment_ids: Vec<Uuid>,
}

/// Conversations in credit transfer workspaces untouched for longer than
/// the workspace's `case_retention_days`. Bounded so one sweep stays small.
pub async fn find_expired_cases(
    db: &PgPool,
    limit: i64,
) -> Result<Vec<ExpiredConversation>, sqlx::Error> {
    sqlx::query_as!(
        ExpiredConversation,
        r#"SELECT c.id AS "conversation_id!", c.workspace_id AS "workspace_id!",
                  COALESCE(array_agg(a.id) FILTER (WHERE a.id IS NOT NULL), '{}')
                      AS "attachment_ids!"
           FROM conversations c
           JOIN credit_transfer_workspaces ct ON ct.workspace_id = c.workspace_id
           LEFT JOIN conversation_attachments a ON a.conversation_id = c.id
           WHERE c.updated_at < NOW() - make_interval(days => ct.case_retention_days)
           GROUP BY c.id
           ORDER BY c.updated_at
           LIMIT $1"#,
        limit,
    )
    .fetch_all(db)
    .await
}

/// Delete a conversation outright; messages, notes, flags and attachment
/// rows cascade. Callers remove attachment files first.
pub async fn delete_conversation(db: &PgPool, conversation_id: Uuid) -> Result<bool, sqlx::Error> {
    let r = sqlx::query!("DELETE FROM conversations WHERE id = $1", conversation_id)
        .execute(db)
        .await?;
    Ok(r.rows_affected() > 0)
}
