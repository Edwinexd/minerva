use sqlx::PgPool;
use uuid::Uuid;

// -- Registration rows (course-scoped LTI connections) --

#[derive(Debug, sqlx::FromRow)]
pub struct RegistrationRow {
    pub id: Uuid,
    pub course_id: Uuid,
    pub name: String,
    pub issuer: String,
    pub client_id: String,
    pub deployment_id: Option<String>,
    pub auth_login_url: String,
    pub auth_token_url: String,
    pub platform_jwks_url: String,
    pub created_by: Uuid,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

pub async fn create_registration(
    db: &PgPool,
    id: Uuid,
    course_id: Uuid,
    name: &str,
    issuer: &str,
    client_id: &str,
    deployment_id: Option<&str>,
    auth_login_url: &str,
    auth_token_url: &str,
    platform_jwks_url: &str,
    created_by: Uuid,
) -> Result<RegistrationRow, sqlx::Error> {
    sqlx::query_as::<_, RegistrationRow>(
        r#"INSERT INTO lti_registrations (id, course_id, name, issuer, client_id, deployment_id, auth_login_url, auth_token_url, platform_jwks_url, created_by)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
        RETURNING id, course_id, name, issuer, client_id, deployment_id, auth_login_url, auth_token_url, platform_jwks_url, created_by, created_at, updated_at"#,
    )
    .bind(id)
    .bind(course_id)
    .bind(name)
    .bind(issuer)
    .bind(client_id)
    .bind(deployment_id)
    .bind(auth_login_url)
    .bind(auth_token_url)
    .bind(platform_jwks_url)
    .bind(created_by)
    .fetch_one(db)
    .await
}

pub async fn find_registration_by_id(
    db: &PgPool,
    id: Uuid,
) -> Result<Option<RegistrationRow>, sqlx::Error> {
    sqlx::query_as::<_, RegistrationRow>(
        "SELECT id, course_id, name, issuer, client_id, deployment_id, auth_login_url, auth_token_url, platform_jwks_url, created_by, created_at, updated_at FROM lti_registrations WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(db)
    .await
}

pub async fn find_registration_by_issuer(
    db: &PgPool,
    issuer: &str,
    client_id: &str,
) -> Result<Option<RegistrationRow>, sqlx::Error> {
    sqlx::query_as::<_, RegistrationRow>(
        "SELECT id, course_id, name, issuer, client_id, deployment_id, auth_login_url, auth_token_url, platform_jwks_url, created_by, created_at, updated_at FROM lti_registrations WHERE issuer = $1 AND client_id = $2",
    )
    .bind(issuer)
    .bind(client_id)
    .fetch_optional(db)
    .await
}

pub async fn list_registrations_for_course(
    db: &PgPool,
    course_id: Uuid,
) -> Result<Vec<RegistrationRow>, sqlx::Error> {
    sqlx::query_as::<_, RegistrationRow>(
        "SELECT id, course_id, name, issuer, client_id, deployment_id, auth_login_url, auth_token_url, platform_jwks_url, created_by, created_at, updated_at FROM lti_registrations WHERE course_id = $1 ORDER BY name",
    )
    .bind(course_id)
    .fetch_all(db)
    .await
}

pub async fn delete_registration(db: &PgPool, id: Uuid) -> Result<bool, sqlx::Error> {
    let result = sqlx::query("DELETE FROM lti_registrations WHERE id = $1")
        .bind(id)
        .execute(db)
        .await?;
    Ok(result.rows_affected() > 0)
}

// -- Launch state rows --

#[derive(Debug, sqlx::FromRow)]
pub struct LaunchRow {
    pub id: Uuid,
    pub state: String,
    pub nonce: String,
    pub registration_id: Uuid,
    pub target_link_uri: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub expires_at: chrono::DateTime<chrono::Utc>,
}

pub async fn create_launch(
    db: &PgPool,
    id: Uuid,
    state: &str,
    nonce: &str,
    registration_id: Uuid,
    target_link_uri: Option<&str>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO lti_launches (id, state, nonce, registration_id, target_link_uri) VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(id)
    .bind(state)
    .bind(nonce)
    .bind(registration_id)
    .bind(target_link_uri)
    .execute(db)
    .await?;
    Ok(())
}

/// Find and delete a launch by state (consume it). Returns None if expired or not found.
pub async fn consume_launch(db: &PgPool, state: &str) -> Result<Option<LaunchRow>, sqlx::Error> {
    sqlx::query_as::<_, LaunchRow>(
        "DELETE FROM lti_launches WHERE state = $1 AND expires_at > NOW() RETURNING id, state, nonce, registration_id, target_link_uri, created_at, expires_at",
    )
    .bind(state)
    .fetch_optional(db)
    .await
}

/// Remove expired launch records.
pub async fn cleanup_expired_launches(db: &PgPool) -> Result<u64, sqlx::Error> {
    let result = sqlx::query("DELETE FROM lti_launches WHERE expires_at <= NOW()")
        .execute(db)
        .await?;
    Ok(result.rows_affected())
}
