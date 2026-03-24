use sqlx::PgPool;
use uuid::Uuid;

#[derive(Debug, sqlx::FromRow)]
pub struct UserRow {
    pub id: Uuid,
    pub eppn: String,
    pub display_name: Option<String>,
    pub role: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

pub async fn find_by_eppn(db: &PgPool, eppn: &str) -> Result<Option<UserRow>, sqlx::Error> {
    sqlx::query_as::<_, UserRow>(
        "SELECT id, eppn, display_name, role, created_at, updated_at FROM users WHERE eppn = $1",
    )
    .bind(eppn)
    .fetch_optional(db)
    .await
}

pub async fn insert(
    db: &PgPool,
    id: Uuid,
    eppn: &str,
    display_name: Option<&str>,
    role: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT INTO users (id, eppn, display_name, role) VALUES ($1, $2, $3, $4)")
        .bind(id)
        .bind(eppn)
        .bind(display_name)
        .bind(role)
        .execute(db)
        .await?;
    Ok(())
}

pub async fn update_login(
    db: &PgPool,
    id: Uuid,
    display_name: Option<&str>,
    role: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE users SET display_name = COALESCE($1, display_name), role = $2, updated_at = NOW() WHERE id = $3",
    )
    .bind(display_name)
    .bind(role)
    .bind(id)
    .execute(db)
    .await?;
    Ok(())
}

pub async fn list_all(db: &PgPool) -> Result<Vec<UserRow>, sqlx::Error> {
    sqlx::query_as::<_, UserRow>(
        "SELECT id, eppn, display_name, role, created_at, updated_at FROM users ORDER BY eppn",
    )
    .fetch_all(db)
    .await
}

pub async fn update_role(db: &PgPool, user_id: Uuid, role: &str) -> Result<bool, sqlx::Error> {
    let result = sqlx::query("UPDATE users SET role = $1, updated_at = NOW() WHERE id = $2")
        .bind(role)
        .bind(user_id)
        .execute(db)
        .await?;
    Ok(result.rows_affected() > 0)
}
