use sqlx::PgPool;
use uuid::Uuid;

#[derive(Debug, sqlx::FromRow)]
pub struct ConversationRow {
    pub id: Uuid,
    pub course_id: Uuid,
    pub user_id: Uuid,
    pub title: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, sqlx::FromRow)]
pub struct MessageRow {
    pub id: Uuid,
    pub conversation_id: Uuid,
    pub role: String,
    pub content: String,
    pub chunks_used: Option<serde_json::Value>,
    pub model_used: Option<String>,
    pub tokens_prompt: Option<i32>,
    pub tokens_completion: Option<i32>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

pub async fn create(
    db: &PgPool,
    id: Uuid,
    course_id: Uuid,
    user_id: Uuid,
) -> Result<ConversationRow, sqlx::Error> {
    sqlx::query_as::<_, ConversationRow>(
        "INSERT INTO conversations (id, course_id, user_id) VALUES ($1, $2, $3) RETURNING id, course_id, user_id, title, created_at, updated_at",
    )
    .bind(id)
    .bind(course_id)
    .bind(user_id)
    .fetch_one(db)
    .await
}

pub async fn list_by_course_user(
    db: &PgPool,
    course_id: Uuid,
    user_id: Uuid,
) -> Result<Vec<ConversationRow>, sqlx::Error> {
    sqlx::query_as::<_, ConversationRow>(
        "SELECT id, course_id, user_id, title, created_at, updated_at FROM conversations WHERE course_id = $1 AND user_id = $2 ORDER BY updated_at DESC",
    )
    .bind(course_id)
    .bind(user_id)
    .fetch_all(db)
    .await
}

pub async fn find_by_id(db: &PgPool, id: Uuid) -> Result<Option<ConversationRow>, sqlx::Error> {
    sqlx::query_as::<_, ConversationRow>(
        "SELECT id, course_id, user_id, title, created_at, updated_at FROM conversations WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(db)
    .await
}

pub async fn list_messages(
    db: &PgPool,
    conversation_id: Uuid,
) -> Result<Vec<MessageRow>, sqlx::Error> {
    sqlx::query_as::<_, MessageRow>(
        "SELECT id, conversation_id, role, content, chunks_used, model_used, tokens_prompt, tokens_completion, created_at FROM messages WHERE conversation_id = $1 ORDER BY created_at ASC",
    )
    .bind(conversation_id)
    .fetch_all(db)
    .await
}

#[allow(clippy::too_many_arguments)]
pub async fn insert_message(
    db: &PgPool,
    id: Uuid,
    conversation_id: Uuid,
    role: &str,
    content: &str,
    chunks_used: Option<&serde_json::Value>,
    model_used: Option<&str>,
    tokens_prompt: Option<i32>,
    tokens_completion: Option<i32>,
) -> Result<MessageRow, sqlx::Error> {
    // Also update conversation timestamp
    let _ = sqlx::query("UPDATE conversations SET updated_at = NOW() WHERE id = $1")
        .bind(conversation_id)
        .execute(db)
        .await;

    sqlx::query_as::<_, MessageRow>(
        r#"INSERT INTO messages (id, conversation_id, role, content, chunks_used, model_used, tokens_prompt, tokens_completion)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
        RETURNING id, conversation_id, role, content, chunks_used, model_used, tokens_prompt, tokens_completion, created_at"#,
    )
    .bind(id)
    .bind(conversation_id)
    .bind(role)
    .bind(content)
    .bind(chunks_used)
    .bind(model_used)
    .bind(tokens_prompt)
    .bind(tokens_completion)
    .fetch_one(db)
    .await
}

pub async fn update_title(db: &PgPool, id: Uuid, title: &str) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE conversations SET title = $1, updated_at = NOW() WHERE id = $2")
        .bind(title)
        .bind(id)
        .execute(db)
        .await?;
    Ok(())
}
