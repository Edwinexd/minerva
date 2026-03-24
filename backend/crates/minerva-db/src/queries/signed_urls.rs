use sqlx::PgPool;
use uuid::Uuid;

#[derive(Debug, sqlx::FromRow)]
pub struct SignedUrlRow {
    pub id: Uuid,
    pub course_id: Uuid,
    pub created_by: Uuid,
    pub token: String,
    pub expires_at: chrono::NaiveDateTime,
    pub max_uses: Option<i32>,
    pub use_count: i32,
    pub created_at: chrono::NaiveDateTime,
}

pub async fn create(
    db: &PgPool,
    id: Uuid,
    course_id: Uuid,
    created_by: Uuid,
    token: &str,
    expires_at: chrono::NaiveDateTime,
    max_uses: Option<i32>,
) -> Result<SignedUrlRow, sqlx::Error> {
    sqlx::query_as::<_, SignedUrlRow>(
        r#"INSERT INTO signed_urls (id, course_id, created_by, token, expires_at, max_uses)
        VALUES ($1, $2, $3, $4, $5, $6)
        RETURNING id, course_id, created_by, token, expires_at, max_uses, use_count, created_at"#,
    )
    .bind(id)
    .bind(course_id)
    .bind(created_by)
    .bind(token)
    .bind(expires_at)
    .bind(max_uses)
    .fetch_one(db)
    .await
}

pub async fn find_by_token(db: &PgPool, token: &str) -> Result<Option<SignedUrlRow>, sqlx::Error> {
    sqlx::query_as::<_, SignedUrlRow>(
        "SELECT id, course_id, created_by, token, expires_at, max_uses, use_count, created_at FROM signed_urls WHERE token = $1",
    )
    .bind(token)
    .fetch_optional(db)
    .await
}

pub async fn list_by_course(db: &PgPool, course_id: Uuid) -> Result<Vec<SignedUrlRow>, sqlx::Error> {
    sqlx::query_as::<_, SignedUrlRow>(
        "SELECT id, course_id, created_by, token, expires_at, max_uses, use_count, created_at FROM signed_urls WHERE course_id = $1 ORDER BY created_at DESC",
    )
    .bind(course_id)
    .fetch_all(db)
    .await
}

pub async fn increment_use(db: &PgPool, id: Uuid) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE signed_urls SET use_count = use_count + 1 WHERE id = $1")
        .bind(id)
        .execute(db)
        .await?;
    Ok(())
}

pub async fn delete(db: &PgPool, id: Uuid) -> Result<bool, sqlx::Error> {
    let result = sqlx::query("DELETE FROM signed_urls WHERE id = $1")
        .bind(id)
        .execute(db)
        .await?;
    Ok(result.rows_affected() > 0)
}
