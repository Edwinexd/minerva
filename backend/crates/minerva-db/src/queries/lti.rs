use sqlx::PgPool;
use uuid::Uuid;

//; Registration rows (course-scoped LTI connections) --

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

//; Platform rows (site-level LTI connections, admin-managed) --

#[derive(Debug)]
pub struct PlatformRow {
    pub id: Uuid,
    pub name: String,
    pub issuer: String,
    pub client_id: String,
    pub deployment_id: Option<String>,
    pub auth_login_url: String,
    pub auth_token_url: String,
    pub platform_jwks_url: String,
    /// NULL when the row was installed via LTI Dynamic Registration (the
    /// `/lti/dynamic-register` endpoint is public so there's no logged-in
    /// user to attribute it to). `Some(user.id)` when an integrator
    /// created the platform manually via the admin UI.
    pub created_by: Option<Uuid>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    /// NULL or empty = no eppn restriction. See the migration comment for
    /// matching rules (mirrors `site_integration_keys.allowed_eppn_domains`).
    pub allowed_eppn_domains: Option<Vec<String>>,
    /// NULL = pending (installed via dynreg, not yet approved by an
    /// integrator); non-NULL = active. Launch validators MUST filter on
    /// `activated_at IS NOT NULL`: a pending row trusts an unvalidated
    /// JWKS source and so cannot be used to authenticate launches.
    pub activated_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Last time the platform-health worker probed this platform's
    /// token endpoint (any outcome). NULL until the first probe runs.
    pub last_health_check_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Coarse bucket from the probe response: `ok`, `invalid_client`,
    /// `http_<code>`, `network`, `parse_error`. UI buckets on
    /// `invalid_client` (orphan-LMS warning) vs other (just informational).
    pub last_health_check_status: Option<String>,
    /// Timestamp of the first `invalid_client` response after the most
    /// recent `ok`. Reset to NULL on any `ok`. Transient errors do NOT
    /// touch it. Drives the 30-day auto-delete sweep.
    pub invalid_client_since: Option<chrono::DateTime<chrono::Utc>>,
}

pub struct CreatePlatform<'a> {
    pub name: &'a str,
    pub issuer: &'a str,
    pub client_id: &'a str,
    pub deployment_id: Option<&'a str>,
    pub auth_login_url: &'a str,
    pub auth_token_url: &'a str,
    pub platform_jwks_url: &'a str,
    /// `None` when installed via Dynamic Registration; `Some(user.id)` for
    /// manual creates.
    pub created_by: Option<Uuid>,
    pub allowed_eppn_domains: Option<&'a [String]>,
    /// Manual creates set this to `Some(NOW())` so the platform is active
    /// immediately (existing UX). Dynreg installs leave it `None` so the
    /// row is pending until an integrator approves it.
    pub activated_at: Option<chrono::DateTime<chrono::Utc>>,
}

pub async fn create_platform(
    db: &PgPool,
    id: Uuid,
    input: &CreatePlatform<'_>,
) -> Result<PlatformRow, sqlx::Error> {
    sqlx::query_as!(
        PlatformRow,
        r#"INSERT INTO lti_platforms (id, name, issuer, client_id, deployment_id, auth_login_url, auth_token_url, platform_jwks_url, created_by, allowed_eppn_domains, activated_at)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
        RETURNING id, name, issuer, client_id, deployment_id, auth_login_url, auth_token_url, platform_jwks_url, created_by, created_at, updated_at, allowed_eppn_domains, activated_at, last_health_check_at, last_health_check_status, invalid_client_since"#,
        id,
        input.name,
        input.issuer,
        input.client_id,
        input.deployment_id,
        input.auth_login_url,
        input.auth_token_url,
        input.platform_jwks_url,
        input.created_by,
        input.allowed_eppn_domains,
        input.activated_at,
    )
    .fetch_one(db)
    .await
}

pub async fn find_platform_by_id(
    db: &PgPool,
    id: Uuid,
) -> Result<Option<PlatformRow>, sqlx::Error> {
    sqlx::query_as!(
        PlatformRow,
        "SELECT id, name, issuer, client_id, deployment_id, auth_login_url, auth_token_url, platform_jwks_url, created_by, created_at, updated_at, allowed_eppn_domains, activated_at, last_health_check_at, last_health_check_status, invalid_client_since FROM lti_platforms WHERE id = $1",
        id,
    )
    .fetch_optional(db)
    .await
}

/// Lookup used by the OIDC login + launch validators. Filters on
/// `activated_at IS NOT NULL` so a pending (dynreg-installed, unapproved)
/// platform CANNOT authenticate a launch, even if a hostile party knows
/// its issuer+client_id. Admin-side listings should use [`list_platforms`]
/// which intentionally returns pending rows so they can be approved.
pub async fn find_platform_by_issuer(
    db: &PgPool,
    issuer: &str,
    client_id: &str,
) -> Result<Option<PlatformRow>, sqlx::Error> {
    sqlx::query_as!(
        PlatformRow,
        "SELECT id, name, issuer, client_id, deployment_id, auth_login_url, auth_token_url, platform_jwks_url, created_by, created_at, updated_at, allowed_eppn_domains, activated_at, last_health_check_at, last_health_check_status, invalid_client_since FROM lti_platforms WHERE issuer = $1 AND client_id = $2 AND activated_at IS NOT NULL",
        issuer,
        client_id,
    )
    .fetch_optional(db)
    .await
}

pub async fn list_platforms(db: &PgPool) -> Result<Vec<PlatformRow>, sqlx::Error> {
    sqlx::query_as!(
        PlatformRow,
        "SELECT id, name, issuer, client_id, deployment_id, auth_login_url, auth_token_url, platform_jwks_url, created_by, created_at, updated_at, allowed_eppn_domains, activated_at, last_health_check_at, last_health_check_status, invalid_client_since FROM lti_platforms ORDER BY activated_at IS NOT NULL, name",
    )
    .fetch_all(db)
    .await
}

/// Activate a pending (dynreg-installed) platform. Optionally overwrites
/// `allowed_eppn_domains` atomically with the activation. Pass `None` for
/// `eppn_domains` to leave the existing value untouched (e.g. if the admin
/// already set it via the dynreg scope form and is approving without
/// changing). Pass `Some(empty_vec)` to deliberately clear the allowlist
/// (= trust any eppn). No-op if already active.
pub async fn activate_platform(
    db: &PgPool,
    id: Uuid,
    eppn_domains: Option<&[String]>,
) -> Result<bool, sqlx::Error> {
    let result = match eppn_domains {
        Some(domains) => {
            sqlx::query!(
                "UPDATE lti_platforms SET activated_at = NOW(), updated_at = NOW(), allowed_eppn_domains = $2 WHERE id = $1 AND activated_at IS NULL",
                id,
                if domains.is_empty() { None } else { Some(domains) },
            )
            .execute(db)
            .await?
        }
        None => {
            sqlx::query!(
                "UPDATE lti_platforms SET activated_at = NOW(), updated_at = NOW() WHERE id = $1 AND activated_at IS NULL",
                id,
            )
            .execute(db)
            .await?
        }
    };
    Ok(result.rows_affected() > 0)
}

/// Set the suggested eppn-domain scope on a pending (still NULL
/// `activated_at`) platform. Called from the public dynreg scope-form
/// endpoint, BEFORE the integrator has approved the row. No-op if the
/// row is already active (the form should never reach an active row, but
/// guard anyway). Pass an empty slice to leave domains NULL (= "any eppn"
/// suggestion).
pub async fn set_pending_platform_scope(
    db: &PgPool,
    id: Uuid,
    eppn_domains: &[String],
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query!(
        "UPDATE lti_platforms SET allowed_eppn_domains = $2, updated_at = NOW() WHERE id = $1 AND activated_at IS NULL",
        id,
        if eppn_domains.is_empty() { None } else { Some(eppn_domains) },
    )
    .execute(db)
    .await?;
    Ok(result.rows_affected() > 0)
}

/// Delete pending platform rows whose `created_at` is older than the
/// supplied interval. Called periodically from the worker to clean up
/// unapproved dynreg installs (which could otherwise pile up if anyone
/// can hit the public dynreg endpoint). Returns the number of rows
/// deleted so the worker can log it.
pub async fn delete_stale_pending_platforms(
    db: &PgPool,
    max_age_hours: i32,
) -> Result<u64, sqlx::Error> {
    let result = sqlx::query!(
        "DELETE FROM lti_platforms WHERE activated_at IS NULL AND created_at < NOW() - make_interval(hours => $1)",
        max_age_hours,
    )
    .execute(db)
    .await?;
    Ok(result.rows_affected())
}

/// Record the outcome of a platform-health probe. `status` is one of
/// `ok` | `invalid_client` | `http_<code>` | `network` | `parse_error`;
/// the worker stamps `last_health_check_at = NOW()` regardless.
///
/// `invalid_client_since` follows these rules (also see the migration
/// comment):
///   * status = `ok`           -> cleared to NULL
///   * status = `invalid_client` and previous value was NULL -> set to NOW()
///   * status = `invalid_client` and previous value was non-NULL -> unchanged
///     (preserve original detection time so the 30-day countdown is stable)
///   * any other status        -> unchanged (transient errors don't move it)
pub async fn record_platform_health(
    db: &PgPool,
    id: Uuid,
    status: &str,
) -> Result<(), sqlx::Error> {
    let touch_invalid_since = status == "invalid_client";
    let clear_invalid_since = status == "ok";
    sqlx::query!(
        r#"UPDATE lti_platforms
           SET last_health_check_at = NOW(),
               last_health_check_status = $2,
               invalid_client_since = CASE
                   WHEN $4 THEN NULL
                   WHEN $3 AND invalid_client_since IS NULL THEN NOW()
                   ELSE invalid_client_since
               END,
               updated_at = NOW()
           WHERE id = $1"#,
        id,
        status,
        touch_invalid_since,
        clear_invalid_since,
    )
    .execute(db)
    .await?;
    Ok(())
}

/// Cascade-delete platforms that have been continuously rejecting our
/// `client_credentials` for at least the supplied grace period. The
/// platform's `lti_course_bindings` + `lti_nrps_contexts` are wiped by
/// the existing FK cascades, so this is the single source of truth for
/// "the LMS deleted us, clean it all up."
pub async fn delete_long_orphaned_platforms(
    db: &PgPool,
    grace_days: i32,
) -> Result<u64, sqlx::Error> {
    let result = sqlx::query!(
        "DELETE FROM lti_platforms WHERE invalid_client_since IS NOT NULL AND invalid_client_since < NOW() - make_interval(days => $1)",
        grace_days,
    )
    .execute(db)
    .await?;
    Ok(result.rows_affected())
}

/// List every active (activated_at IS NOT NULL) platform the worker
/// should probe. Skips pending rows because their token endpoint will
/// already reject us (the LMS hasn't fully provisioned the tool yet on
/// some platforms during the dynreg flow, and the row is pending on
/// our side so the value of a probe is nil).
pub async fn list_platforms_for_health_check(db: &PgPool) -> Result<Vec<PlatformRow>, sqlx::Error> {
    sqlx::query_as!(
        PlatformRow,
        "SELECT id, name, issuer, client_id, deployment_id, auth_login_url, auth_token_url, platform_jwks_url, created_by, created_at, updated_at, allowed_eppn_domains, activated_at, last_health_check_at, last_health_check_status, invalid_client_since FROM lti_platforms WHERE activated_at IS NOT NULL"
    )
    .fetch_all(db)
    .await
}

pub async fn delete_platform(db: &PgPool, id: Uuid) -> Result<bool, sqlx::Error> {
    let result = sqlx::query!("DELETE FROM lti_platforms WHERE id = $1", id)
        .execute(db)
        .await?;
    Ok(result.rows_affected() > 0)
}

//; Course binding rows (per-context links for site-level platforms) --

#[derive(Debug)]
pub struct BindingRow {
    pub id: Uuid,
    pub platform_id: Uuid,
    pub context_id: String,
    pub context_label: Option<String>,
    pub context_title: Option<String>,
    pub course_id: Uuid,
    pub created_by: Uuid,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

pub struct CreateBinding<'a> {
    pub platform_id: Uuid,
    pub context_id: &'a str,
    pub context_label: Option<&'a str>,
    pub context_title: Option<&'a str>,
    pub course_id: Uuid,
    pub created_by: Uuid,
}

pub async fn create_binding(
    db: &PgPool,
    id: Uuid,
    input: &CreateBinding<'_>,
) -> Result<BindingRow, sqlx::Error> {
    sqlx::query_as!(
        BindingRow,
        r#"INSERT INTO lti_course_bindings (id, platform_id, context_id, context_label, context_title, course_id, created_by)
        VALUES ($1, $2, $3, $4, $5, $6, $7)
        RETURNING id, platform_id, context_id, context_label, context_title, course_id, created_by, created_at"#,
        id,
        input.platform_id,
        input.context_id,
        input.context_label,
        input.context_title,
        input.course_id,
        input.created_by,
    )
    .fetch_one(db)
    .await
}

pub async fn find_binding(
    db: &PgPool,
    platform_id: Uuid,
    context_id: &str,
) -> Result<Option<BindingRow>, sqlx::Error> {
    sqlx::query_as!(
        BindingRow,
        "SELECT id, platform_id, context_id, context_label, context_title, course_id, created_by, created_at FROM lti_course_bindings WHERE platform_id = $1 AND context_id = $2",
        platform_id,
        context_id,
    )
    .fetch_optional(db)
    .await
}

pub async fn list_bindings_for_platform(
    db: &PgPool,
    platform_id: Uuid,
) -> Result<Vec<BindingRow>, sqlx::Error> {
    sqlx::query_as!(
        BindingRow,
        "SELECT id, platform_id, context_id, context_label, context_title, course_id, created_by, created_at FROM lti_course_bindings WHERE platform_id = $1 ORDER BY created_at DESC",
        platform_id,
    )
    .fetch_all(db)
    .await
}

/// Row joining a binding to its parent platform; used by the teacher view to
/// surface "this course is linked via a site-level platform that an admin
/// configured" without needing two round-trips.
#[derive(Debug)]
pub struct BindingWithPlatformRow {
    pub binding_id: Uuid,
    pub platform_id: Uuid,
    pub platform_name: String,
    pub platform_issuer: String,
    pub platform_client_id: String,
    pub context_id: String,
    pub context_label: Option<String>,
    pub context_title: Option<String>,
    pub course_id: Uuid,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

pub async fn list_bindings_for_course(
    db: &PgPool,
    course_id: Uuid,
) -> Result<Vec<BindingWithPlatformRow>, sqlx::Error> {
    sqlx::query_as!(
        BindingWithPlatformRow,
        r#"SELECT
            b.id AS binding_id,
            b.platform_id,
            p.name AS platform_name,
            p.issuer AS platform_issuer,
            p.client_id AS platform_client_id,
            b.context_id,
            b.context_label,
            b.context_title,
            b.course_id,
            b.created_at
        FROM lti_course_bindings b
        JOIN lti_platforms p ON p.id = b.platform_id
        WHERE b.course_id = $1
        ORDER BY b.created_at DESC"#,
        course_id,
    )
    .fetch_all(db)
    .await
}

pub async fn delete_binding(db: &PgPool, id: Uuid) -> Result<bool, sqlx::Error> {
    let result = sqlx::query!("DELETE FROM lti_course_bindings WHERE id = $1", id)
        .execute(db)
        .await?;
    Ok(result.rows_affected() > 0)
}

//; Launch state rows --

#[derive(Debug)]
pub struct LaunchRow {
    pub id: Uuid,
    pub state: String,
    pub nonce: String,
    pub registration_id: Option<Uuid>,
    pub platform_id: Option<Uuid>,
    pub target_link_uri: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub expires_at: chrono::DateTime<chrono::Utc>,
}

/// Source of truth for a launch-in-progress: which platform/registration row
/// holds the keyset + identifiers we'll validate against.
pub enum LaunchSource {
    Registration(Uuid),
    Platform(Uuid),
}

pub async fn create_launch(
    db: &PgPool,
    id: Uuid,
    state: &str,
    nonce: &str,
    source: LaunchSource,
    target_link_uri: Option<&str>,
) -> Result<(), sqlx::Error> {
    let (registration_id, platform_id) = match source {
        LaunchSource::Registration(rid) => (Some(rid), None),
        LaunchSource::Platform(pid) => (None, Some(pid)),
    };
    sqlx::query!(
        "INSERT INTO lti_launches (id, state, nonce, registration_id, platform_id, target_link_uri) VALUES ($1, $2, $3, $4, $5, $6)",
        id,
        state,
        nonce,
        registration_id,
        platform_id,
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
        "DELETE FROM lti_launches WHERE state = $1 AND expires_at > NOW() RETURNING id, state, nonce, registration_id, platform_id, target_link_uri, created_at, expires_at",
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
