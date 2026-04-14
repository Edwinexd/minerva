use sqlx::PgPool;
use uuid::Uuid;

// -- Registration rows (course-scoped LTI connections) --

#[derive(Debug)]
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

pub struct CreateRegistration<'a> {
    pub course_id: Uuid,
    pub name: &'a str,
    pub issuer: &'a str,
    pub client_id: &'a str,
    pub deployment_id: Option<&'a str>,
    pub auth_login_url: &'a str,
    pub auth_token_url: &'a str,
    pub platform_jwks_url: &'a str,
    pub created_by: Uuid,
}

pub async fn create_registration(
    db: &PgPool,
    id: Uuid,
    input: &CreateRegistration<'_>,
) -> Result<RegistrationRow, sqlx::Error> {
    sqlx::query_as!(
        RegistrationRow,
        r#"INSERT INTO lti_registrations (id, course_id, name, issuer, client_id, deployment_id, auth_login_url, auth_token_url, platform_jwks_url, created_by)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
        RETURNING id, course_id, name, issuer, client_id, deployment_id, auth_login_url, auth_token_url, platform_jwks_url, created_by, created_at, updated_at"#,
        id,
        input.course_id,
        input.name,
        input.issuer,
        input.client_id,
        input.deployment_id,
        input.auth_login_url,
        input.auth_token_url,
        input.platform_jwks_url,
        input.created_by,
    )
    .fetch_one(db)
    .await
}

pub async fn find_registration_by_id(
    db: &PgPool,
    id: Uuid,
) -> Result<Option<RegistrationRow>, sqlx::Error> {
    sqlx::query_as!(
        RegistrationRow,
        "SELECT id, course_id, name, issuer, client_id, deployment_id, auth_login_url, auth_token_url, platform_jwks_url, created_by, created_at, updated_at FROM lti_registrations WHERE id = $1",
        id,
    )
    .fetch_optional(db)
    .await
}

pub async fn find_registration_by_issuer(
    db: &PgPool,
    issuer: &str,
    client_id: &str,
) -> Result<Option<RegistrationRow>, sqlx::Error> {
    sqlx::query_as!(
        RegistrationRow,
        "SELECT id, course_id, name, issuer, client_id, deployment_id, auth_login_url, auth_token_url, platform_jwks_url, created_by, created_at, updated_at FROM lti_registrations WHERE issuer = $1 AND client_id = $2",
        issuer,
        client_id,
    )
    .fetch_optional(db)
    .await
}

pub async fn list_registrations_for_course(
    db: &PgPool,
    course_id: Uuid,
) -> Result<Vec<RegistrationRow>, sqlx::Error> {
    sqlx::query_as!(
        RegistrationRow,
        "SELECT id, course_id, name, issuer, client_id, deployment_id, auth_login_url, auth_token_url, platform_jwks_url, created_by, created_at, updated_at FROM lti_registrations WHERE course_id = $1 ORDER BY name",
        course_id,
    )
    .fetch_all(db)
    .await
}

pub async fn delete_registration(db: &PgPool, id: Uuid) -> Result<bool, sqlx::Error> {
    let result = sqlx::query!("DELETE FROM lti_registrations WHERE id = $1", id)
        .execute(db)
        .await?;
    Ok(result.rows_affected() > 0)
}

// -- Launch state rows --

#[derive(Debug)]
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
    sqlx::query!(
        "INSERT INTO lti_launches (id, state, nonce, registration_id, target_link_uri) VALUES ($1, $2, $3, $4, $5)",
        id,
        state,
        nonce,
        registration_id,
        target_link_uri,
    )
    .execute(db)
    .await?;
    Ok(())
}

/// Find and delete a launch by state (consume it). Returns None if expired or not found.
pub async fn consume_launch(db: &PgPool, state: &str) -> Result<Option<LaunchRow>, sqlx::Error> {
    sqlx::query_as!(
        LaunchRow,
        "DELETE FROM lti_launches WHERE state = $1 AND expires_at > NOW() RETURNING id, state, nonce, registration_id, target_link_uri, created_at, expires_at",
        state,
    )
    .fetch_optional(db)
    .await
}

/// Remove expired launch records.
pub async fn cleanup_expired_launches(db: &PgPool) -> Result<u64, sqlx::Error> {
    let result = sqlx::query!("DELETE FROM lti_launches WHERE expires_at <= NOW()")
        .execute(db)
        .await?;
    Ok(result.rows_affected())
}
