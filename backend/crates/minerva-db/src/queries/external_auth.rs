use sqlx::PgPool;
use uuid::Uuid;

#[derive(Debug)]
pub struct ExternalAuthInviteRow {
    pub id: Uuid,
    pub jti: Uuid,
    pub eppn: String,
    pub display_name: Option<String>,
    pub created_by: Uuid,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub expires_at: chrono::DateTime<chrono::Utc>,
    pub revoked_at: Option<chrono::DateTime<chrono::Utc>>,
}

pub async fn insert(
    db: &PgPool,
    id: Uuid,
    jti: Uuid,
    eppn: &str,
    display_name: Option<&str>,
    created_by: Uuid,
    expires_at: chrono::DateTime<chrono::Utc>,
) -> Result<ExternalAuthInviteRow, sqlx::Error> {
    sqlx::query_as!(
        ExternalAuthInviteRow,
        r#"INSERT INTO external_auth_invites
            (id, jti, eppn, display_name, created_by, expires_at)
        VALUES ($1, $2, $3, $4, $5, $6)
        RETURNING id, jti, eppn, display_name, created_by, created_at, expires_at, revoked_at"#,
        id,
        jti,
        eppn,
        display_name,
        created_by,
        expires_at,
    )
    .fetch_one(db)
    .await
}

pub async fn find_by_jti(
    db: &PgPool,
    jti: Uuid,
) -> Result<Option<ExternalAuthInviteRow>, sqlx::Error> {
    sqlx::query_as!(
        ExternalAuthInviteRow,
        r#"SELECT id, jti, eppn, display_name, created_by, created_at, expires_at, revoked_at
        FROM external_auth_invites WHERE jti = $1"#,
        jti,
    )
    .fetch_optional(db)
    .await
}

pub async fn list_all(db: &PgPool) -> Result<Vec<ExternalAuthInviteRow>, sqlx::Error> {
    sqlx::query_as!(
        ExternalAuthInviteRow,
        r#"SELECT id, jti, eppn, display_name, created_by, created_at, expires_at, revoked_at
        FROM external_auth_invites ORDER BY created_at DESC"#,
    )
    .fetch_all(db)
    .await
}

pub async fn revoke(db: &PgPool, id: Uuid) -> Result<bool, sqlx::Error> {
    let result = sqlx::query!(
        "UPDATE external_auth_invites SET revoked_at = NOW() WHERE id = $1 AND revoked_at IS NULL",
        id,
    )
    .execute(db)
    .await?;
    Ok(result.rows_affected() > 0)
}
