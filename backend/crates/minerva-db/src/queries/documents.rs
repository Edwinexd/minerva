use sqlx::PgPool;
use uuid::Uuid;

#[derive(Debug, sqlx::FromRow)]
pub struct DocumentRow {
    pub id: Uuid,
    pub course_id: Uuid,
    pub filename: String,
    pub mime_type: String,
    pub size_bytes: i64,
    pub status: String,
    pub chunk_count: Option<i32>,
    pub error_msg: Option<String>,
    pub uploaded_by: Uuid,
    pub displayable: bool,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub processed_at: Option<chrono::DateTime<chrono::Utc>>,
}

pub async fn insert(
    db: &PgPool,
    id: Uuid,
    course_id: Uuid,
    filename: &str,
    mime_type: &str,
    size_bytes: i64,
    uploaded_by: Uuid,
) -> Result<DocumentRow, sqlx::Error> {
    sqlx::query_as::<_, DocumentRow>(
        r#"INSERT INTO documents (id, course_id, filename, mime_type, size_bytes, uploaded_by)
        VALUES ($1, $2, $3, $4, $5, $6)
        RETURNING id, course_id, filename, mime_type, size_bytes, status, chunk_count, error_msg, uploaded_by, displayable, created_at, processed_at"#,
    )
    .bind(id)
    .bind(course_id)
    .bind(filename)
    .bind(mime_type)
    .bind(size_bytes)
    .bind(uploaded_by)
    .fetch_one(db)
    .await
}

pub async fn list_by_course(db: &PgPool, course_id: Uuid) -> Result<Vec<DocumentRow>, sqlx::Error> {
    sqlx::query_as::<_, DocumentRow>(
        "SELECT id, course_id, filename, mime_type, size_bytes, status, chunk_count, error_msg, uploaded_by, displayable, created_at, processed_at FROM documents WHERE course_id = $1 ORDER BY created_at DESC",
    )
    .bind(course_id)
    .fetch_all(db)
    .await
}

pub async fn find_by_id(db: &PgPool, id: Uuid) -> Result<Option<DocumentRow>, sqlx::Error> {
    sqlx::query_as::<_, DocumentRow>(
        "SELECT id, course_id, filename, mime_type, size_bytes, status, chunk_count, error_msg, uploaded_by, displayable, created_at, processed_at FROM documents WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(db)
    .await
}

pub async fn update_displayable(
    db: &PgPool,
    id: Uuid,
    displayable: bool,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query("UPDATE documents SET displayable = $1 WHERE id = $2")
        .bind(displayable)
        .bind(id)
        .execute(db)
        .await?;
    Ok(result.rows_affected() > 0)
}

/// Returns the set of document IDs in a course that are NOT displayable.
pub async fn hidden_document_ids(
    db: &PgPool,
    course_id: Uuid,
) -> Result<std::collections::HashSet<String>, sqlx::Error> {
    let rows: Vec<(Uuid,)> =
        sqlx::query_as("SELECT id FROM documents WHERE course_id = $1 AND displayable = FALSE")
            .bind(course_id)
            .fetch_all(db)
            .await?;
    Ok(rows.into_iter().map(|(id,)| id.to_string()).collect())
}

pub async fn delete(db: &PgPool, id: Uuid) -> Result<bool, sqlx::Error> {
    let result = sqlx::query("DELETE FROM documents WHERE id = $1")
        .bind(id)
        .execute(db)
        .await?;
    Ok(result.rows_affected() > 0)
}

/// Atomically claim up to `limit` pending documents for processing.
/// Uses `FOR UPDATE SKIP LOCKED` so multiple workers won't grab the same row.
pub async fn claim_pending(db: &PgPool, limit: i32) -> Result<Vec<DocumentRow>, sqlx::Error> {
    sqlx::query_as::<_, DocumentRow>(
        r#"UPDATE documents
        SET status = 'processing', processing_started_at = NOW()
        WHERE id IN (
            SELECT id FROM documents
            WHERE status = 'pending'
            ORDER BY created_at ASC
            LIMIT $1
            FOR UPDATE SKIP LOCKED
        )
        RETURNING id, course_id, filename, mime_type, size_bytes, status, chunk_count, error_msg, uploaded_by, displayable, created_at, processed_at"#,
    )
    .bind(limit)
    .fetch_all(db)
    .await
}

/// List documents awaiting external transcript processing.
/// These are play.dsv.su.se URL documents that the worker has triaged.
pub async fn list_awaiting_transcripts(db: &PgPool) -> Result<Vec<DocumentRow>, sqlx::Error> {
    sqlx::query_as::<_, DocumentRow>(
        "SELECT id, course_id, filename, mime_type, size_bytes, status, chunk_count, error_msg, uploaded_by, displayable, created_at, processed_at FROM documents WHERE status = 'awaiting_transcript' ORDER BY created_at ASC",
    )
    .fetch_all(db)
    .await
}

/// Update a document's filename, mime_type, size, and reset status to 'pending'.
/// Used when replacing a URL stub with actual transcript content.
pub async fn replace_with_transcript(
    db: &PgPool,
    id: Uuid,
    filename: &str,
    mime_type: &str,
    size_bytes: i64,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query(
        "UPDATE documents SET filename = $1, mime_type = $2, size_bytes = $3, status = 'pending', error_msg = NULL, chunk_count = NULL, processed_at = NULL WHERE id = $4 AND status = 'awaiting_transcript'",
    )
    .bind(filename)
    .bind(mime_type)
    .bind(size_bytes)
    .bind(id)
    .execute(db)
    .await?;
    Ok(result.rows_affected() > 0)
}

/// Reset documents stuck in 'processing' back to 'pending'.
/// Used on startup for crash recovery: any document still marked 'processing'
/// was interrupted by a server restart.
pub async fn reset_stale_processing(db: &PgPool) -> Result<u64, sqlx::Error> {
    let result = sqlx::query(
        "UPDATE documents SET status = 'pending', processing_started_at = NULL WHERE status = 'processing'",
    )
    .execute(db)
    .await?;
    Ok(result.rows_affected())
}

/// Reset documents that have been stuck in 'processing' for longer than
/// `min_age_seconds`. Run periodically so a silently-panicked or wedged
/// processing task doesn't leave a document stuck until the next pod restart.
///
/// Documents being actively processed (started <= threshold ago) are left
/// alone. The threshold should comfortably exceed the worst-case processing
/// time for a single document.
pub async fn reset_stale_processing_older_than(
    db: &PgPool,
    min_age_seconds: i64,
) -> Result<u64, sqlx::Error> {
    let result = sqlx::query(
        r#"UPDATE documents
        SET status = 'pending', processing_started_at = NULL
        WHERE status = 'processing'
          AND (processing_started_at IS NULL
               OR processing_started_at < NOW() - make_interval(secs => $1))"#,
    )
    .bind(min_age_seconds as f64)
    .execute(db)
    .await?;
    Ok(result.rows_affected())
}
