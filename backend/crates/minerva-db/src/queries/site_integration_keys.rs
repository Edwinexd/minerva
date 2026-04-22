use sqlx::PgPool;
use uuid::Uuid;

#[derive(Debug)]
pub struct SiteIntegrationKeyRow {
    pub id: Uuid,
    pub name: String,
    pub key_hash: String,
    pub key_prefix: String,
    pub created_by: Uuid,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub last_used_at: Option<chrono::DateTime<chrono::Utc>>,
    /// NULL or empty = no eppn restriction. Non-empty = acting eppn must
    /// end with `@<d>` for some `d` in this list (case-insensitive).
    /// Domains are stored lowercased.
    pub allowed_eppn_domains: Option<Vec<String>>,
}

pub async fn insert(
    db: &PgPool,
    id: Uuid,
    name: &str,
    key_hash: &str,
    key_prefix: &str,
    created_by: Uuid,
    allowed_eppn_domains: Option<&[String]>,
) -> Result<SiteIntegrationKeyRow, sqlx::Error> {
    sqlx::query_as!(
        SiteIntegrationKeyRow,
        r#"INSERT INTO site_integration_keys (id, name, key_hash, key_prefix, created_by, allowed_eppn_domains)
        VALUES ($1, $2, $3, $4, $5, $6)
        RETURNING id, name, key_hash, key_prefix, created_by, created_at, last_used_at, allowed_eppn_domains"#,
        id,
        name,
        key_hash,
        key_prefix,
        created_by,
        allowed_eppn_domains,
    )
    .fetch_one(db)
    .await
}

pub async fn list_all(db: &PgPool) -> Result<Vec<SiteIntegrationKeyRow>, sqlx::Error> {
    sqlx::query_as!(
        SiteIntegrationKeyRow,
        r#"SELECT id, name, key_hash, key_prefix, created_by, created_at, last_used_at, allowed_eppn_domains
        FROM site_integration_keys ORDER BY created_at DESC"#,
    )
    .fetch_all(db)
    .await
}

pub async fn find_by_hash(
    db: &PgPool,
    key_hash: &str,
) -> Result<Option<SiteIntegrationKeyRow>, sqlx::Error> {
    sqlx::query_as!(
        SiteIntegrationKeyRow,
        r#"SELECT id, name, key_hash, key_prefix, created_by, created_at, last_used_at, allowed_eppn_domains
        FROM site_integration_keys WHERE key_hash = $1"#,
        key_hash,
    )
    .fetch_optional(db)
    .await
}

pub async fn delete(db: &PgPool, id: Uuid) -> Result<bool, sqlx::Error> {
    let result = sqlx::query!("DELETE FROM site_integration_keys WHERE id = $1", id)
        .execute(db)
        .await?;
    Ok(result.rows_affected() > 0)
}

pub async fn touch_last_used(db: &PgPool, id: Uuid) -> Result<(), sqlx::Error> {
    sqlx::query!(
        "UPDATE site_integration_keys SET last_used_at = NOW() WHERE id = $1",
        id,
    )
    .execute(db)
    .await?;
    Ok(())
}
