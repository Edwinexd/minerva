use sqlx::PgPool;
use uuid::Uuid;

// -- Canvas connection rows --

#[derive(Debug)]
pub struct ConnectionRow {
    pub id: Uuid,
    pub course_id: Uuid,
    pub name: String,
    pub canvas_base_url: String,
    pub canvas_api_token: String,
    pub canvas_course_id: String,
    pub auto_sync: bool,
    pub created_by: Uuid,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    pub last_synced_at: Option<chrono::DateTime<chrono::Utc>>,
}

pub struct CreateConnection<'a> {
    pub course_id: Uuid,
    pub name: &'a str,
    pub canvas_base_url: &'a str,
    pub canvas_api_token: &'a str,
    pub canvas_course_id: &'a str,
    pub auto_sync: bool,
    pub created_by: Uuid,
}

pub async fn create_connection(
    db: &PgPool,
    id: Uuid,
    input: &CreateConnection<'_>,
) -> Result<ConnectionRow, sqlx::Error> {
    sqlx::query_as!(
        ConnectionRow,
        r#"INSERT INTO canvas_connections (id, course_id, name, canvas_base_url, canvas_api_token, canvas_course_id, auto_sync, created_by)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
        RETURNING id, course_id, name, canvas_base_url, canvas_api_token, canvas_course_id, auto_sync, created_by, created_at, updated_at, last_synced_at"#,
        id,
        input.course_id,
        input.name,
        input.canvas_base_url,
        input.canvas_api_token,
        input.canvas_course_id,
        input.auto_sync,
        input.created_by,
    )
    .fetch_one(db)
    .await
}

pub async fn list_connections(
    db: &PgPool,
    course_id: Uuid,
) -> Result<Vec<ConnectionRow>, sqlx::Error> {
    sqlx::query_as!(
        ConnectionRow,
        "SELECT id, course_id, name, canvas_base_url, canvas_api_token, canvas_course_id, auto_sync, created_by, created_at, updated_at, last_synced_at FROM canvas_connections WHERE course_id = $1 ORDER BY name",
        course_id,
    )
    .fetch_all(db)
    .await
}

pub async fn find_connection(db: &PgPool, id: Uuid) -> Result<Option<ConnectionRow>, sqlx::Error> {
    sqlx::query_as!(
        ConnectionRow,
        "SELECT id, course_id, name, canvas_base_url, canvas_api_token, canvas_course_id, auto_sync, created_by, created_at, updated_at, last_synced_at FROM canvas_connections WHERE id = $1",
        id,
    )
    .fetch_optional(db)
    .await
}

pub async fn delete_connection(db: &PgPool, id: Uuid) -> Result<bool, sqlx::Error> {
    let result = sqlx::query!("DELETE FROM canvas_connections WHERE id = $1", id)
        .execute(db)
        .await?;
    Ok(result.rows_affected() > 0)
}

pub async fn update_last_synced(db: &PgPool, id: Uuid) -> Result<(), sqlx::Error> {
    sqlx::query!(
        "UPDATE canvas_connections SET last_synced_at = NOW(), updated_at = NOW() WHERE id = $1",
        id,
    )
    .execute(db)
    .await?;
    Ok(())
}

// -- Sync log rows --

#[derive(Debug)]
pub struct SyncLogRow {
    pub id: Uuid,
    pub connection_id: Uuid,
    pub canvas_file_id: String,
    pub filename: String,
    pub content_type: Option<String>,
    pub minerva_document_id: Option<Uuid>,
    pub synced_at: chrono::DateTime<chrono::Utc>,
}

pub async fn list_synced_files(
    db: &PgPool,
    connection_id: Uuid,
) -> Result<Vec<SyncLogRow>, sqlx::Error> {
    sqlx::query_as!(
        SyncLogRow,
        "SELECT id, connection_id, canvas_file_id, filename, content_type, minerva_document_id, synced_at FROM canvas_sync_log WHERE connection_id = $1 ORDER BY synced_at DESC",
        connection_id,
    )
    .fetch_all(db)
    .await
}

/// Returns the set of Canvas file IDs already synced for this connection.
pub async fn synced_file_ids(
    db: &PgPool,
    connection_id: Uuid,
) -> Result<std::collections::HashSet<String>, sqlx::Error> {
    let rows = sqlx::query!(
        "SELECT canvas_file_id FROM canvas_sync_log WHERE connection_id = $1",
        connection_id,
    )
    .fetch_all(db)
    .await?;
    Ok(rows.into_iter().map(|r| r.canvas_file_id).collect())
}

pub async fn insert_sync_log(
    db: &PgPool,
    id: Uuid,
    connection_id: Uuid,
    canvas_file_id: &str,
    filename: &str,
    content_type: Option<&str>,
    minerva_document_id: Option<Uuid>,
) -> Result<SyncLogRow, sqlx::Error> {
    sqlx::query_as!(
        SyncLogRow,
        r#"INSERT INTO canvas_sync_log (id, connection_id, canvas_file_id, filename, content_type, minerva_document_id)
        VALUES ($1, $2, $3, $4, $5, $6)
        ON CONFLICT (connection_id, canvas_file_id) DO UPDATE SET filename = EXCLUDED.filename, minerva_document_id = COALESCE(EXCLUDED.minerva_document_id, canvas_sync_log.minerva_document_id)
        RETURNING id, connection_id, canvas_file_id, filename, content_type, minerva_document_id, synced_at"#,
        id,
        connection_id,
        canvas_file_id,
        filename,
        content_type,
        minerva_document_id,
    )
    .fetch_one(db)
    .await
}
