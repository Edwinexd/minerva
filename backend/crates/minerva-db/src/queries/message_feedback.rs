use sqlx::PgPool;
use uuid::Uuid;

#[derive(Debug)]
pub struct MessageFeedbackRow {
    pub id: Uuid,
    pub message_id: Uuid,
    pub user_id: Uuid,
    pub rating: String,
    pub category: Option<String>,
    pub comment: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug)]
pub struct MessageFeedbackWithUserRow {
    pub id: Uuid,
    pub message_id: Uuid,
    pub user_id: Uuid,
    pub rating: String,
    pub category: Option<String>,
    pub comment: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    pub user_eppn: Option<String>,
    pub user_display_name: Option<String>,
}

/// Insert or update the current user's feedback for a single message. Each
/// (message, user) pair has at most one feedback row.
pub async fn upsert(
    db: &PgPool,
    message_id: Uuid,
    user_id: Uuid,
    rating: &str,
    category: Option<&str>,
    comment: Option<&str>,
) -> Result<MessageFeedbackRow, sqlx::Error> {
    sqlx::query_as!(
        MessageFeedbackRow,
        r#"INSERT INTO message_feedback (id, message_id, user_id, rating, category, comment)
        VALUES ($1, $2, $3, $4, $5, $6)
        ON CONFLICT (message_id, user_id) DO UPDATE
            SET rating = EXCLUDED.rating,
                category = EXCLUDED.category,
                comment = EXCLUDED.comment,
                updated_at = NOW()
        RETURNING id, message_id, user_id, rating, category, comment, created_at, updated_at"#,
        Uuid::new_v4(),
        message_id,
        user_id,
        rating,
        category,
        comment,
    )
    .fetch_one(db)
    .await
}

/// All feedback rows for messages in a conversation, ordered oldest first.
pub async fn list_for_conversation(
    db: &PgPool,
    conversation_id: Uuid,
) -> Result<Vec<MessageFeedbackWithUserRow>, sqlx::Error> {
    sqlx::query_as!(
        MessageFeedbackWithUserRow,
        r#"SELECT f.id, f.message_id, f.user_id, f.rating, f.category, f.comment,
            f.created_at, f.updated_at,
            u.eppn AS "user_eppn?", u.display_name AS user_display_name
        FROM message_feedback f
        JOIN messages m ON m.id = f.message_id
        JOIN users u ON u.id = f.user_id
        WHERE m.conversation_id = $1
        ORDER BY f.created_at ASC"#,
        conversation_id,
    )
    .fetch_all(db)
    .await
}
