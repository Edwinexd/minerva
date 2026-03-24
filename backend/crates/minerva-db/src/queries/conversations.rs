use sqlx::PgPool;
use uuid::Uuid;

#[derive(Debug, sqlx::FromRow)]
pub struct ConversationRow {
    pub id: Uuid,
    pub course_id: Uuid,
    pub user_id: Uuid,
    pub title: Option<String>,
    pub pinned: bool,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, sqlx::FromRow)]
pub struct ConversationWithUserRow {
    pub id: Uuid,
    pub course_id: Uuid,
    pub user_id: Uuid,
    pub title: Option<String>,
    pub pinned: bool,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    pub user_eppn: Option<String>,
    pub user_display_name: Option<String>,
    pub message_count: Option<i64>,
}

#[derive(Debug, sqlx::FromRow)]
pub struct TeacherNoteRow {
    pub id: Uuid,
    pub conversation_id: Uuid,
    pub message_id: Option<Uuid>,
    pub author_id: Uuid,
    pub content: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    pub author_display_name: Option<String>,
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
        "INSERT INTO conversations (id, course_id, user_id) VALUES ($1, $2, $3) RETURNING id, course_id, user_id, title, pinned, created_at, updated_at",
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
        "SELECT id, course_id, user_id, title, pinned, created_at, updated_at FROM conversations WHERE course_id = $1 AND user_id = $2 ORDER BY updated_at DESC",
    )
    .bind(course_id)
    .bind(user_id)
    .fetch_all(db)
    .await
}

pub async fn find_by_id(db: &PgPool, id: Uuid) -> Result<Option<ConversationRow>, sqlx::Error> {
    sqlx::query_as::<_, ConversationRow>(
        "SELECT id, course_id, user_id, title, pinned, created_at, updated_at FROM conversations WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(db)
    .await
}

pub async fn list_all_by_course(
    db: &PgPool,
    course_id: Uuid,
) -> Result<Vec<ConversationWithUserRow>, sqlx::Error> {
    sqlx::query_as::<_, ConversationWithUserRow>(
        r#"SELECT c.id, c.course_id, c.user_id, c.title, c.pinned, c.created_at, c.updated_at,
            u.eppn AS user_eppn, u.display_name AS user_display_name,
            (SELECT COUNT(*) FROM messages m WHERE m.conversation_id = c.id) AS message_count
        FROM conversations c
        JOIN users u ON u.id = c.user_id
        WHERE c.course_id = $1
        ORDER BY c.updated_at DESC"#,
    )
    .bind(course_id)
    .fetch_all(db)
    .await
}

pub async fn list_pinned_by_course(
    db: &PgPool,
    course_id: Uuid,
) -> Result<Vec<ConversationWithUserRow>, sqlx::Error> {
    sqlx::query_as::<_, ConversationWithUserRow>(
        r#"SELECT c.id, c.course_id, c.user_id, c.title, c.pinned, c.created_at, c.updated_at,
            u.eppn AS user_eppn, u.display_name AS user_display_name,
            (SELECT COUNT(*) FROM messages m WHERE m.conversation_id = c.id) AS message_count
        FROM conversations c
        JOIN users u ON u.id = c.user_id
        WHERE c.course_id = $1 AND c.pinned = true
        ORDER BY c.updated_at DESC"#,
    )
    .bind(course_id)
    .fetch_all(db)
    .await
}

pub async fn set_pinned(
    db: &PgPool,
    id: Uuid,
    pinned: bool,
) -> Result<Option<ConversationRow>, sqlx::Error> {
    sqlx::query_as::<_, ConversationRow>(
        "UPDATE conversations SET pinned = $2, updated_at = NOW() WHERE id = $1 RETURNING id, course_id, user_id, title, pinned, created_at, updated_at",
    )
    .bind(id)
    .bind(pinned)
    .fetch_optional(db)
    .await
}

// Teacher notes

pub async fn create_note(
    db: &PgPool,
    id: Uuid,
    conversation_id: Uuid,
    message_id: Option<Uuid>,
    author_id: Uuid,
    content: &str,
) -> Result<TeacherNoteRow, sqlx::Error> {
    sqlx::query_as::<_, TeacherNoteRow>(
        r#"WITH inserted AS (
            INSERT INTO teacher_notes (id, conversation_id, message_id, author_id, content)
            VALUES ($1, $2, $3, $4, $5)
            RETURNING id, conversation_id, message_id, author_id, content, created_at, updated_at
        )
        SELECT i.*, u.display_name AS author_display_name
        FROM inserted i
        JOIN users u ON u.id = i.author_id"#,
    )
    .bind(id)
    .bind(conversation_id)
    .bind(message_id)
    .bind(author_id)
    .bind(content)
    .fetch_one(db)
    .await
}

pub async fn list_notes(
    db: &PgPool,
    conversation_id: Uuid,
) -> Result<Vec<TeacherNoteRow>, sqlx::Error> {
    sqlx::query_as::<_, TeacherNoteRow>(
        r#"SELECT tn.id, tn.conversation_id, tn.message_id, tn.author_id, tn.content,
            tn.created_at, tn.updated_at, u.display_name AS author_display_name
        FROM teacher_notes tn
        JOIN users u ON u.id = tn.author_id
        WHERE tn.conversation_id = $1
        ORDER BY tn.created_at ASC"#,
    )
    .bind(conversation_id)
    .fetch_all(db)
    .await
}

pub async fn update_note(
    db: &PgPool,
    id: Uuid,
    content: &str,
) -> Result<Option<TeacherNoteRow>, sqlx::Error> {
    sqlx::query_as::<_, TeacherNoteRow>(
        r#"WITH updated AS (
            UPDATE teacher_notes SET content = $2, updated_at = NOW() WHERE id = $1
            RETURNING id, conversation_id, message_id, author_id, content, created_at, updated_at
        )
        SELECT u2.*, users.display_name AS author_display_name
        FROM updated u2
        JOIN users ON users.id = u2.author_id"#,
    )
    .bind(id)
    .bind(content)
    .fetch_optional(db)
    .await
}

pub async fn delete_note(db: &PgPool, id: Uuid) -> Result<bool, sqlx::Error> {
    let result = sqlx::query("DELETE FROM teacher_notes WHERE id = $1")
        .bind(id)
        .execute(db)
        .await?;
    Ok(result.rows_affected() > 0)
}

pub async fn find_note_by_id(db: &PgPool, id: Uuid) -> Result<Option<TeacherNoteRow>, sqlx::Error> {
    sqlx::query_as::<_, TeacherNoteRow>(
        r#"SELECT tn.id, tn.conversation_id, tn.message_id, tn.author_id, tn.content,
            tn.created_at, tn.updated_at, u.display_name AS author_display_name
        FROM teacher_notes tn
        JOIN users u ON u.id = tn.author_id
        WHERE tn.id = $1"#,
    )
    .bind(id)
    .fetch_all(db)
    .await
    .map(|mut v| v.pop())
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
