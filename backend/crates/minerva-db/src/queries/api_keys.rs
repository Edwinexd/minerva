use sqlx::PgPool;
use uuid::Uuid;

#[derive(Debug)]
pub struct ApiKeyRow {
    pub id: Uuid,
    pub course_id: Uuid,
    pub created_by: Uuid,
    pub name: String,
    pub key_hash: String,
    pub key_prefix: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub last_used_at: Option<chrono::DateTime<chrono::Utc>>,
}

pub async fn insert(
    db: &PgPool,
    id: Uuid,
    course_id: Uuid,
    created_by: Uuid,
    name: &str,
    key_hash: &str,
    key_prefix: &str,
) -> Result<ApiKeyRow, sqlx::Error> {
    sqlx::query_as!(
        ApiKeyRow,
        r#"INSERT INTO api_keys (id, course_id, created_by, name, key_hash, key_prefix)
        VALUES ($1, $2, $3, $4, $5, $6)
        RETURNING id, course_id, created_by, name, key_hash, key_prefix, created_at, last_used_at"#,
        id,
        course_id,
        created_by,
        name,
        key_hash,
        key_prefix,
    )
    .fetch_one(db)
    .await
}

pub async fn list_by_course(db: &PgPool, course_id: Uuid) -> Result<Vec<ApiKeyRow>, sqlx::Error> {
    sqlx::query_as!(
        ApiKeyRow,
        r#"SELECT id, course_id, created_by, name, key_hash, key_prefix, created_at, last_used_at
        FROM api_keys WHERE course_id = $1 ORDER BY created_at DESC"#,
        course_id,
    )
    .fetch_all(db)
    .await
}

pub async fn find_by_hash(db: &PgPool, key_hash: &str) -> Result<Option<ApiKeyRow>, sqlx::Error> {
    sqlx::query_as!(
        ApiKeyRow,
        r#"SELECT id, course_id, created_by, name, key_hash, key_prefix, created_at, last_used_at
        FROM api_keys WHERE key_hash = $1"#,
        key_hash,
    )
    .fetch_optional(db)
    .await
}

pub async fn delete(db: &PgPool, id: Uuid, course_id: Uuid) -> Result<bool, sqlx::Error> {
    let result = sqlx::query!(
        "DELETE FROM api_keys WHERE id = $1 AND course_id = $2",
        id,
        course_id,
    )
    .execute(db)
    .await?;
    Ok(result.rows_affected() > 0)
}

pub async fn touch_last_used(db: &PgPool, id: Uuid) -> Result<(), sqlx::Error> {
    sqlx::query!("UPDATE api_keys SET last_used_at = NOW() WHERE id = $1", id)
        .execute(db)
        .await?;
    Ok(())
}
