//! LTI 1.3 Tool Provider endpoints.
//!
//! Public endpoints (no Shibboleth / API key auth), mounted at /lti:
//!   GET/POST /lti/login   -- OIDC third-party initiated login
//!   POST     /lti/launch  -- Validate id_token, create session, redirect to embed
//!   GET      /lti/jwks    -- Serve tool public keys
//!
//! Public API endpoints (also unauthenticated; mounted at /api/lti):
//!   GET      /api/lti/bind -- Read bind-token, return pickable courses (frontend-driven)
//!   POST     /api/lti/bind -- Create a course binding, issue embed token
//!
//! Course-level endpoints (behind auth_middleware, course teacher/owner):
//!   GET    /courses/{course_id}/lti          -- List LTI registrations
//!   POST   /courses/{course_id}/lti          -- Register LTI connection
//!   DELETE /courses/{course_id}/lti/{id}     -- Remove registration
//!
//! Admin endpoints (behind auth_middleware, admin only):
//!   GET    /admin/lti/platforms              -- List site-level platforms
//!   POST   /admin/lti/platforms              -- Create site-level platform
//!   DELETE /admin/lti/platforms/{id}         -- Remove site-level platform
//!   GET    /admin/lti/setup                  -- Moodle/Canvas admin copy-paste config

use axum::extract::{Path, Query, State};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{delete, get, post};
use axum::{Extension, Form, Json, Router};
use hmac::{Hmac, KeyInit, Mac};
use rand::RngExt;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use uuid::Uuid;

use crate::error::AppError;
use crate::lti;
use crate::state::AppState;
use minerva_core::models::User;

type HmacSha256 = Hmac<Sha256>;

const EMBED_TOKEN_TTL_SECS: i64 = 8 * 3600;
const BIND_TOKEN_TTL_SECS: i64 = 15 * 60;

// ---------------------------------------------------------------------------
// Public LTI routes (mounted at /lti, outside auth middleware)
// ---------------------------------------------------------------------------

pub fn public_router() -> Router<AppState> {
    Router::new()
        .route(
            "/login",
            get(login_initiation_get).post(login_initiation_post),
        )
        .route("/launch", post(handle_launch))
        .route("/jwks", get(jwks))
        .route("/icon.svg", get(icon_svg))
        .route("/icon.png", get(icon_png))
}

/// Public-but-not-LTI-protocol routes: the bind picker sits here because it's
/// XHR'd from the SPA (under /api) rather than hit by the LMS directly.
pub fn public_api_router() -> Router<AppState> {
    Router::new().route("/bind", get(bind_info).post(bind_complete))
}

/// Course-level routes for managing LTI registrations (teacher/owner only).
pub fn course_router() -> Router<AppState> {
    Router::new()
        .route("/lti/setup", get(lti_setup))
        .route("/lti", get(list_registrations).post(create_registration))
        .route("/lti/{registration_id}", delete(delete_registration))
}

/// Admin routes for managing site-level LTI platforms.
pub fn admin_router() -> Router<AppState> {
    Router::new()
        .route("/lti/setup", get(admin_lti_setup))
        .route("/lti/platforms", get(list_platforms).post(create_platform))
        .route("/lti/platforms/{platform_id}", delete(delete_platform))
        .route(
            "/lti/platforms/{platform_id}/bindings",
            get(list_platform_bindings),
        )
        .route(
            "/lti/platforms/{platform_id}/bindings/{binding_id}",
            delete(delete_platform_binding),
        )
}

// ---------------------------------------------------------------------------
// OIDC login initiation (Step 1 of LTI 1.3)
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct LoginInitiationParams {
    iss: String,
    login_hint: String,
    target_link_uri: String,
    lti_message_hint: Option<String>,
    client_id: Option<String>,
    #[allow(dead_code)]
    lti_deployment_id: Option<String>,
}

/// GET /lti/login -- Moodle redirects here with query params.
async fn login_initiation_get(
    State(state): State<AppState>,
    Query(params): Query<LoginInitiationParams>,
) -> Result<Response, AppError> {
    do_login_initiation(state, params).await
}

/// POST /lti/login -- Moodle may POST form-encoded params instead.
async fn login_initiation_post(
    State(state): State<AppState>,
    Form(params): Form<LoginInitiationParams>,
) -> Result<Response, AppError> {
    do_login_initiation(state, params).await
}

async fn do_login_initiation(
    state: AppState,
    params: LoginInitiationParams,
) -> Result<Response, AppError> {
    // Look up registration OR platform by issuer + client_id. client_id is required.
    let client_id = params
        .client_id
        .as_deref()
        .ok_or_else(|| AppError::bad_request("lti.client_id_required"))?;

    // Per-course registrations take precedence over site-level platforms: if
    // both exist for the same (iss, client_id) the course-scoped one wins so
    // teachers can override a site-level default. We also reject inserting a
    // conflicting pair at create time.
    let (source, client_id_val, auth_login_url) =
        match minerva_db::queries::lti::find_registration_by_issuer(
            &state.db,
            &params.iss,
            client_id,
        )
        .await?
        {
            Some(r) => (
                minerva_db::queries::lti::LaunchSource::Registration(r.id),
                r.client_id.clone(),
                r.auth_login_url.clone(),
            ),
            None => {
                let platform = minerva_db::queries::lti::find_platform_by_issuer(
                    &state.db,
                    &params.iss,
                    client_id,
                )
                .await?
                .ok_or_else(|| {
                    AppError::bad_request_with(
                        "lti.no_registration",
                        [
                            ("issuer", params.iss.clone()),
                            ("client_id", client_id.to_string()),
                        ],
                    )
                })?;
                (
                    minerva_db::queries::lti::LaunchSource::Platform(platform.id),
                    platform.client_id.clone(),
                    platform.auth_login_url.clone(),
                )
            }
        };

    // Generate cryptographic state and nonce.
    let oidc_state = generate_random_string(32);
    let nonce = generate_random_string(32);

    minerva_db::queries::lti::create_launch(
        &state.db,
        Uuid::new_v4(),
        &oidc_state,
        &nonce,
        source,
        Some(&params.target_link_uri),
    )
    .await
    .map_err(|e| AppError::Internal(format!("failed to store launch state: {}", e)))?;

    // Clean up expired launches in the background.
    let db = state.db.clone();
    tokio::spawn(async move {
        let _ = minerva_db::queries::lti::cleanup_expired_launches(&db).await;
    });

    // redirect_uri MUST be the tool's own launch endpoint (not target_link_uri).
    let launch_url = format!("{}/lti/launch", state.config.base_url);

    let redirect_uri = format!(
        "{}?scope=openid\
         &response_type=id_token\
         &client_id={}\
         &redirect_uri={}\
         &login_hint={}\
         &state={}\
         &nonce={}\
         &response_mode=form_post\
         &prompt=none{}",
        auth_login_url,
        urlencoding::encode(&client_id_val),
        urlencoding::encode(&launch_url),
        urlencoding::encode(&params.login_hint),
        urlencoding::encode(&oidc_state),
        urlencoding::encode(&nonce),
        params
            .lti_message_hint
            .as_ref()
            .map(|h| format!("&lti_message_hint={}", urlencoding::encode(h)))
            .unwrap_or_default(),
    );

    Ok(Redirect::to(&redirect_uri).into_response())
}

// ---------------------------------------------------------------------------
// Launch handler (Step 2 of LTI 1.3)
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct LaunchForm {
    id_token: String,
    state: String,
}

/// Resolved launch source: either a per-course registration (1:1 with a
/// Minerva course) or a site-level platform (course resolved via binding).
enum ResolvedSource {
    Registration(minerva_db::queries::lti::RegistrationRow),
    Platform(minerva_db::queries::lti::PlatformRow),
}

impl ResolvedSource {
    fn deployment_id(&self) -> Option<&str> {
        match self {
            ResolvedSource::Registration(r) => r.deployment_id.as_deref(),
            ResolvedSource::Platform(p) => p.deployment_id.as_deref(),
        }
    }
    fn client_id(&self) -> &str {
        match self {
            ResolvedSource::Registration(r) => &r.client_id,
            ResolvedSource::Platform(p) => &p.client_id,
        }
    }
    fn identifier(&self) -> String {
        match self {
            ResolvedSource::Registration(r) => r.id.to_string(),
            ResolvedSource::Platform(p) => p.id.to_string(),
        }
    }
}

async fn handle_launch(
    State(state): State<AppState>,
    Form(form): Form<LaunchForm>,
) -> Result<Response, AppError> {
    // 1. Consume the OIDC state (validates it exists and hasn't expired).
    let launch = minerva_db::queries::lti::consume_launch(&state.db, &form.state)
        .await?
        .ok_or_else(|| AppError::bad_request("lti.invalid_or_expired_state"))?;

    // 2. Resolve source from whichever FK the launch row holds. The DB
    //    CHECK constraint guarantees exactly one is set.
    let source = match (launch.registration_id, launch.platform_id) {
        (Some(rid), None) => {
            let reg = minerva_db::queries::lti::find_registration_by_id(&state.db, rid)
                .await?
                .ok_or_else(|| AppError::Internal("registration not found for launch".into()))?;
            ResolvedSource::Registration(reg)
        }
        (None, Some(pid)) => {
            let plat = minerva_db::queries::lti::find_platform_by_id(&state.db, pid)
                .await?
                .ok_or_else(|| AppError::Internal("platform not found for launch".into()))?;
            ResolvedSource::Platform(plat)
        }
        _ => return Err(AppError::Internal("launch row missing source".into())),
    };

    // 3. Validate the JWT using the shared PlatformConfig shape.
    let claims = {
        let cfg = match &source {
            ResolvedSource::Registration(r) => lti::PlatformConfig::from(r),
            ResolvedSource::Platform(p) => lti::PlatformConfig::from(p),
        };
        lti::validate_launch_jwt(cfg, &form.id_token, &launch.nonce, &state.http_client).await?
    };

    // 4. Verify deployment_id if one was registered.
    if let Some(expected) = source.deployment_id() {
        match claims.deployment_id.as_deref() {
            Some(actual) if actual == expected => {}
            Some(actual) => {
                return Err(AppError::bad_request_with(
                    "lti.deployment_id_mismatch",
                    [
                        ("expected", expected.to_string()),
                        ("actual", actual.to_string()),
                    ],
                ));
            }
            None => {
                return Err(AppError::bad_request("lti.deployment_id_missing"));
            }
        }
    }

    // 5. Map user identity. Priority:
    //    a) Custom param "user_eppn" (Moodle can substitute $User.username)
    //    b) email claim
    //    c) Synthetic eppn from LTI sub + source id
    let eppn = claims
        .custom
        .as_ref()
        .and_then(|c| c.get("user_eppn"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .or_else(|| claims.email.clone())
        .unwrap_or_else(|| format!("lti_{}_{}", source.identifier(), claims.sub))
        .to_lowercase();

    let display_name = claims.name.as_deref();

    // 6. Find or create the user.
    //    Reuses an existing Shib user's record if present; does NOT modify
    //    their role or display name -- LTI should not alter existing accounts.
    let user = match minerva_db::queries::users::find_by_eppn(&state.db, &eppn).await? {
        Some(u) => u,
        None => {
            let id = Uuid::new_v4();
            minerva_db::queries::users::insert(&state.db, id, &eppn, display_name, "student")
                .await?;
            minerva_db::queries::users::find_by_id(&state.db, id)
                .await?
                .ok_or_else(|| AppError::Internal("user creation failed".into()))?
        }
    };

    // 7. Resolve target Minerva course. For a per-course registration the
    //    course is baked in. For a site-level platform we look up a binding
    //    on (platform_id, context.id); if none exists, redirect the launcher
    //    into the bind flow instead of resolving a course.
    let course_id = match &source {
        ResolvedSource::Registration(r) => r.course_id,
        ResolvedSource::Platform(p) => {
            let context_id = claims
                .context
                .as_ref()
                .and_then(|c| c.id.clone())
                .ok_or_else(|| AppError::bad_request("lti.context_id_missing"))?;

            match minerva_db::queries::lti::find_binding(&state.db, p.id, &context_id).await? {
                Some(b) => b.course_id,
                None => {
                    // No binding yet → redirect launcher to the bind flow. The
                    // bind token ties this decision to this specific user +
                    // platform + Moodle context so we can trust the choice
                    // without re-auth on the frontend.
                    return bind_redirect_response(&state, &user, p, &claims, &context_id);
                }
            }
        }
    };

    // 8. Verify the resolved course still exists.
    minerva_db::queries::courses::find_by_id(&state.db, course_id)
        .await?
        .ok_or(AppError::NotFound)?;

    // 9. Add course membership + suggest teacher elevation when claimed.
    //    See the original trust-model comment: LTI claims can't promote
    //    directly; they only suggest.
    apply_course_membership(&state, course_id, &user, &claims).await?;

    // 10. Issue the embed token and return the redirect page.
    embed_redirect_response(&state, course_id, &user, source.client_id())
}

/// Apply course membership (always as student) and optionally suggest teacher
/// elevation based on the LTI roles claim. Extracted so the launch handler and
/// the bind-complete handler can share the logic.
async fn apply_course_membership(
    state: &AppState,
    course_id: Uuid,
    user: &minerva_db::queries::users::UserRow,
    claims: &lti::LtiLaunchClaims,
) -> Result<(), AppError> {
    let claimed_role = lti::lti_roles_to_course_role(&claims.roles);
    let existing_role =
        minerva_db::queries::courses::get_member_role(&state.db, course_id, user.id).await?;
    if existing_role.is_none() {
        minerva_db::queries::courses::add_member(&state.db, course_id, user.id, "student").await?;
    }
    if claimed_role == "teacher" && existing_role.as_deref() != Some("teacher") {
        let detail = serde_json::json!({ "lti_roles": claims.roles });
        let _ = minerva_db::queries::role_suggestions::upsert_pending(
            &state.db,
            Uuid::new_v4(),
            course_id,
            user.id,
            "teacher",
            "lti",
            Some(&detail),
        )
        .await?;
    }
    Ok(())
}

/// Build the `/embed/{course_id}?token=...` URL for a launched user. Shared
/// by the launch HTML redirect and the bind-complete JSON response.
fn build_embed_redirect_url(
    state: &AppState,
    course_id: Uuid,
    user: &minerva_db::queries::users::UserRow,
    client_id: &str,
) -> Result<String, AppError> {
    let expires_at = chrono::Utc::now() + chrono::Duration::seconds(EMBED_TOKEN_TTL_SECS);
    let payload = format!("{}:{}:{}", course_id, user.id, expires_at.timestamp());

    let mut mac = HmacSha256::new_from_slice(state.config.hmac_secret.as_bytes())
        .map_err(|_| AppError::Internal("hmac key error".into()))?;
    mac.update(payload.as_bytes());
    let sig = hex::encode(mac.finalize().into_bytes());

    let token_raw = format!("{}:{}", payload, sig);
    let token = {
        use base64::Engine;
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(token_raw.as_bytes())
    };

    Ok(format!(
        "/embed/{}?token={}&lti_client_id={}",
        course_id,
        token,
        urlencoding::encode(client_id)
    ))
}

/// Build the embed-token HTML redirect page (step 10 of launch).
fn embed_redirect_response(
    state: &AppState,
    course_id: Uuid,
    user: &minerva_db::queries::users::UserRow,
    client_id: &str,
) -> Result<Response, AppError> {
    let embed_path = build_embed_redirect_url(state, course_id, user, client_id)?;

    // Both course_id (UUID) and token (base64url) are safe for interpolation,
    // but escape anyway for defense-in-depth.
    let escaped_path = embed_path.replace('\"', "&quot;").replace('<', "&lt;");

    Ok(Html(format!(
        r#"<!DOCTYPE html>
<html><head><meta charset="utf-8"><title>Launching Minerva…</title></head>
<body>
<script>window.location.replace("{escaped_path}");</script>
<noscript><a href="{escaped_path}">Click here to continue</a></noscript>
</body></html>"#,
    ))
    .into_response())
}

/// When a site-level platform launch arrives with no course binding, mint a
/// bind-token and redirect the launcher to the frontend bind picker. The
/// token is HMAC-signed and carries everything the bind-complete handler
/// needs: user + platform + context + claims-derived LTI roles.
fn bind_redirect_response(
    state: &AppState,
    user: &minerva_db::queries::users::UserRow,
    platform: &minerva_db::queries::lti::PlatformRow,
    claims: &lti::LtiLaunchClaims,
    context_id: &str,
) -> Result<Response, AppError> {
    let expires_at = chrono::Utc::now() + chrono::Duration::seconds(BIND_TOKEN_TTL_SECS);

    // Serialize context metadata so the frontend can render "link Moodle
    // course X to which Minerva course?". We keep this compact; labels are
    // best-effort (Moodle may omit them).
    let body = BindTokenPayload {
        user_id: user.id,
        platform_id: platform.id,
        context_id: context_id.to_string(),
        context_label: claims.context.as_ref().and_then(|c| c.label.clone()),
        context_title: claims.context.as_ref().and_then(|c| c.title.clone()),
        roles: claims.roles.clone(),
        client_id: platform.client_id.clone(),
        expires_at: expires_at.timestamp(),
    };

    let token = sign_bind_token(&state.config.hmac_secret, &body)?;
    // /lti-bind is a frontend SPA route (not under the backend's /lti/* LMS
    // namespace). The SPA XHRs the decision through /api/lti/bind.
    let redirect = format!("/lti-bind?token={}", urlencoding::encode(&token));
    let escaped = redirect.replace('\"', "&quot;").replace('<', "&lt;");

    Ok(Html(format!(
        r#"<!DOCTYPE html>
<html><head><meta charset="utf-8"><title>Linking Minerva…</title></head>
<body>
<script>window.location.replace("{escaped}");</script>
<noscript><a href="{escaped}">Click here to continue</a></noscript>
</body></html>"#,
    ))
    .into_response())
}

// ---------------------------------------------------------------------------
// JWKS endpoint
// ---------------------------------------------------------------------------

async fn jwks(State(state): State<AppState>) -> Json<serde_json::Value> {
    Json(state.lti.jwks_json.clone())
}

async fn icon_svg() -> Response {
    // Kept in sync with frontend/public/favicon.svg -- update both when the brand changes.
    const SVG: &str = include_str!("../../assets/favicon.svg");
    ([(axum::http::header::CONTENT_TYPE, "image/svg+xml")], SVG).into_response()
}

// Moodle 4 CSS-masks SVG activity icons with the theme accent color, so a branded
// SVG renders as a flat blob. PNGs bypass that treatment -- advertise this one to Moodle.
async fn icon_png() -> Response {
    const PNG: &[u8] = include_bytes!("../../assets/favicon.png");
    ([(axum::http::header::CONTENT_TYPE, "image/png")], PNG).into_response()
}

// ---------------------------------------------------------------------------
// Course-level: LTI setup + registration management
// ---------------------------------------------------------------------------

/// GET /courses/{course_id}/lti/setup -- returns everything the teacher needs
/// to configure Moodle BEFORE creating a registration in Minerva.
async fn lti_setup(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(course_id): Path<Uuid>,
) -> Result<Json<LtiSetupResponse>, AppError> {
    require_course_teacher(&state, course_id, &user).await?;
    Ok(Json(build_setup_response(&state.config.base_url)))
}

#[derive(Debug, Serialize)]
struct LtiSetupResponse {
    /// Values to enter in Moodle's "Add new LTI External tool" form.
    moodle_tool_config: MoodleToolConfig,
    /// Step-by-step instructions for the teacher.
    steps: Vec<String>,
}

fn build_setup_response(base_url: &str) -> LtiSetupResponse {
    let config = build_moodle_config(base_url);
    LtiSetupResponse {
        steps: vec![
            "In Moodle, go to your course → More → LTI External tools → Add tool.".into(),
            format!("Set Tool URL to: {}", config.tool_url),
            format!("Set LTI version to: {}", config.lti_version),
            format!("Set Public key type to: {}", config.public_key_type),
            format!("Set Public keyset to: {}", config.public_keyset_url),
            format!("Set Initiate login URL to: {}", config.initiate_login_url),
            format!("Set Redirection URI(s) to: {}", config.redirection_uris),
            format!(
                "Under Custom parameters, add: {} -- this links Moodle users to their Minerva identity. Without it, students launched from Moodle will be separate users from those who log in directly.",
                config.custom_parameters,
            ),
            format!(
                "Under 'Show more...', set Icon URL to: {}",
                config.icon_url,
            ),
            "Under Services, leave defaults (no grade passback needed).".into(),
            "Under Privacy, 'Share launcher's name' is optional (populates display names).".into(),
            "Save. Moodle will show the tool's registration details.".into(),
            "Copy the Platform ID (issuer), Client ID, Deployment ID, and the platform endpoints (Authentication request URL, Access token URL, Public keyset URL) from Moodle.".into(),
            "Back in Minerva, create an LTI registration for this course with those values.".into(),
        ],
        moodle_tool_config: config,
    }
}

/// Response includes both the registration data and the tool URLs + Moodle config hints.
#[derive(Debug, Serialize)]
struct RegistrationResponse {
    id: Uuid,
    course_id: Uuid,
    name: String,
    issuer: String,
    client_id: String,
    deployment_id: Option<String>,
    auth_login_url: String,
    auth_token_url: String,
    platform_jwks_url: String,
    created_at: chrono::DateTime<chrono::Utc>,
    /// Pre-filled values for Moodle's "Add new LTI External tool" form.
    moodle_config: MoodleToolConfig,
}

/// Maps directly to Moodle's "Add new LTI External tool" form fields.
#[derive(Debug, Serialize)]
struct MoodleToolConfig {
    /// "Tool URL" in Moodle
    tool_url: String,
    /// "LTI version" in Moodle
    lti_version: &'static str,
    /// "Public key type" in Moodle
    public_key_type: &'static str,
    /// "Public keyset" in Moodle (the tool's JWKS URL)
    public_keyset_url: String,
    /// "Initiate login URL" in Moodle
    initiate_login_url: String,
    /// "Redirection URI(s)" in Moodle
    redirection_uris: String,
    /// Suggested "Custom parameters" for Moodle (maps eppn)
    custom_parameters: &'static str,
    /// "Default launch container"
    default_launch_container: &'static str,
    /// "Icon URL" in Moodle (optional, under "Show more...")
    icon_url: String,
    /// "Share launcher's name with tool"
    share_name: bool,
    /// "Share launcher's email with tool"
    share_email: bool,
    /// "Accept grades from the tool"
    accept_grades: bool,
}

async fn list_registrations(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(course_id): Path<Uuid>,
) -> Result<Json<Vec<RegistrationResponse>>, AppError> {
    require_course_teacher(&state, course_id, &user).await?;

    let rows =
        minerva_db::queries::lti::list_registrations_for_course(&state.db, course_id).await?;

    Ok(Json(
        rows.into_iter()
            .map(|r| to_response(r, &state.config.base_url))
            .collect(),
    ))
}

#[derive(Debug, Deserialize)]
struct CreateRegistrationRequest {
    name: String,
    issuer: String,
    client_id: String,
    deployment_id: Option<String>,
    /// Optional -- defaults to {issuer}/mod/lti/auth.php
    auth_login_url: Option<String>,
    /// Optional -- defaults to {issuer}/mod/lti/token.php
    auth_token_url: Option<String>,
    /// Optional -- defaults to {issuer}/mod/lti/certs.php
    platform_jwks_url: Option<String>,
}

async fn create_registration(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(course_id): Path<Uuid>,
    Json(body): Json<CreateRegistrationRequest>,
) -> Result<Json<RegistrationResponse>, AppError> {
    require_course_teacher(&state, course_id, &user).await?;

    let issuer = body.issuer.trim_end_matches('/');
    let auth_login_url = body
        .auth_login_url
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| format!("{}/mod/lti/auth.php", issuer));
    let auth_token_url = body
        .auth_token_url
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| format!("{}/mod/lti/token.php", issuer));
    let platform_jwks_url = body
        .platform_jwks_url
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| format!("{}/mod/lti/certs.php", issuer));

    // A per-course registration can't share (issuer, client_id) with a
    // site-level platform -- the login handler can only dispatch to one, and
    // silently preferring one path would surprise the other side's admin.
    if minerva_db::queries::lti::find_platform_by_issuer(&state.db, issuer, &body.client_id)
        .await?
        .is_some()
    {
        return Err(AppError::bad_request("lti.platform_already_registered"));
    }

    let id = Uuid::new_v4();
    let row = minerva_db::queries::lti::create_registration(
        &state.db,
        id,
        &minerva_db::queries::lti::CreateRegistration {
            course_id,
            name: &body.name,
            issuer,
            client_id: &body.client_id,
            deployment_id: body.deployment_id.as_deref(),
            auth_login_url: &auth_login_url,
            auth_token_url: &auth_token_url,
            platform_jwks_url: &platform_jwks_url,
            created_by: user.id,
        },
    )
    .await
    .map_err(|e| {
        if e.to_string().contains("duplicate key") {
            AppError::bad_request("lti.registration_duplicate")
        } else {
            AppError::Database(e)
        }
    })?;

    Ok(Json(to_response(row, &state.config.base_url)))
}

async fn delete_registration(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path((course_id, registration_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<serde_json::Value>, AppError> {
    require_course_teacher(&state, course_id, &user).await?;

    let reg = minerva_db::queries::lti::find_registration_by_id(&state.db, registration_id)
        .await?
        .ok_or(AppError::NotFound)?;
    if reg.course_id != course_id {
        return Err(AppError::NotFound);
    }

    minerva_db::queries::lti::delete_registration(&state.db, registration_id).await?;
    Ok(Json(serde_json::json!({ "deleted": true })))
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

async fn require_course_teacher(
    state: &AppState,
    course_id: Uuid,
    user: &User,
) -> Result<(), AppError> {
    if user.role.is_admin() {
        return Ok(());
    }

    let course = minerva_db::queries::courses::find_by_id(&state.db, course_id)
        .await?
        .ok_or(AppError::NotFound)?;

    if course.owner_id == user.id {
        return Ok(());
    }

    // LTI registrations are a teacher-only operation -- TAs are excluded.
    let is_teacher =
        minerva_db::queries::courses::is_course_teacher_strict(&state.db, course_id, user.id)
            .await?;
    if is_teacher {
        return Ok(());
    }

    Err(AppError::Forbidden)
}

fn build_moodle_config(base_url: &str) -> MoodleToolConfig {
    MoodleToolConfig {
        tool_url: format!("{}/lti/launch", base_url),
        lti_version: "LTI 1.3",
        public_key_type: "Keyset URL",
        public_keyset_url: format!("{}/lti/jwks", base_url),
        initiate_login_url: format!("{}/lti/login", base_url),
        redirection_uris: format!("{}/lti/launch", base_url),
        custom_parameters: "user_eppn=$User.username",
        default_launch_container: "Embed",
        icon_url: format!("{}/lti/icon.png", base_url),
        share_name: true,
        share_email: false,
        accept_grades: false,
    }
}

fn to_response(
    r: minerva_db::queries::lti::RegistrationRow,
    base_url: &str,
) -> RegistrationResponse {
    RegistrationResponse {
        id: r.id,
        course_id: r.course_id,
        name: r.name,
        issuer: r.issuer,
        client_id: r.client_id,
        deployment_id: r.deployment_id,
        auth_login_url: r.auth_login_url,
        auth_token_url: r.auth_token_url,
        platform_jwks_url: r.platform_jwks_url,
        created_at: r.created_at,
        moodle_config: build_moodle_config(base_url),
    }
}

fn generate_random_string(len: usize) -> String {
    let mut rng = rand::rng();
    (0..len)
        .map(|_| {
            let idx = rng.random_range(0..36u8);
            if idx < 10 {
                (b'0' + idx) as char
            } else {
                (b'a' + idx - 10) as char
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Bind flow (site-level platforms, no existing binding)
// ---------------------------------------------------------------------------

/// Payload carried inside the HMAC-signed bind token. Small on purpose --
/// everything the bind-complete handler needs, without hitting the DB again.
#[derive(Serialize, Deserialize)]
struct BindTokenPayload {
    user_id: Uuid,
    platform_id: Uuid,
    context_id: String,
    context_label: Option<String>,
    context_title: Option<String>,
    roles: Vec<String>,
    client_id: String,
    /// Unix timestamp.
    expires_at: i64,
}

/// Serialize + HMAC-SHA256 sign a bind token. Format is
/// base64url(json):base64url(mac), ready for use in a URL.
fn sign_bind_token(secret: &str, payload: &BindTokenPayload) -> Result<String, AppError> {
    use base64::Engine;
    let json = serde_json::to_vec(payload)
        .map_err(|e| AppError::Internal(format!("bind token serialize: {}", e)))?;
    let b64_payload = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&json);

    let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
        .map_err(|_| AppError::Internal("hmac key error".into()))?;
    mac.update(b64_payload.as_bytes());
    let sig = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes());

    Ok(format!("{}.{}", b64_payload, sig))
}

fn verify_bind_token(secret: &str, token: &str) -> Result<BindTokenPayload, AppError> {
    use base64::Engine;
    let (b64_payload, sig) = token
        .split_once('.')
        .ok_or_else(|| AppError::bad_request("lti.bind_token_malformed"))?;

    let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
        .map_err(|_| AppError::Internal("hmac key error".into()))?;
    mac.update(b64_payload.as_bytes());
    let expected =
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes());

    // Constant-time compare (subtle-sensitive but tokens are short-lived).
    if expected.len() != sig.len() {
        return Err(AppError::bad_request("lti.bind_token_bad_sig"));
    }
    let eq = expected
        .as_bytes()
        .iter()
        .zip(sig.as_bytes().iter())
        .fold(0u8, |acc, (a, b)| acc | (a ^ b));
    if eq != 0 {
        return Err(AppError::bad_request("lti.bind_token_bad_sig"));
    }

    let json = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(b64_payload)
        .map_err(|_| AppError::bad_request("lti.bind_token_malformed"))?;
    let payload: BindTokenPayload = serde_json::from_slice(&json)
        .map_err(|_| AppError::bad_request("lti.bind_token_malformed"))?;

    if payload.expires_at < chrono::Utc::now().timestamp() {
        return Err(AppError::bad_request("lti.bind_token_expired"));
    }

    Ok(payload)
}

#[derive(Debug, Deserialize)]
struct BindTokenQuery {
    token: String,
}

#[derive(Debug, Serialize)]
struct BindInfoResponse {
    /// Platform name (human-readable, from admin config).
    platform_name: String,
    /// LMS-provided context labels so the teacher can identify the course.
    context_id: String,
    context_label: Option<String>,
    context_title: Option<String>,
    /// Whether the LMS-claimed roles look teacher-ish (used by UI for
    /// messaging; the actual authorization check is on submit).
    is_teacher_role: bool,
    /// Minerva courses the launching user can bind to (owner + teacher/ta).
    /// Non-teachers see this empty and must ask a course teacher to launch.
    courses: Vec<BindInfoCourse>,
}

#[derive(Debug, Serialize)]
struct BindInfoCourse {
    id: Uuid,
    name: String,
}

/// GET /lti/bind?token=... -- returns enough for the frontend to show the
/// picker. Not authenticated by Shibboleth; the token itself is the auth.
async fn bind_info(
    State(state): State<AppState>,
    Query(q): Query<BindTokenQuery>,
) -> Result<Json<BindInfoResponse>, AppError> {
    let payload = verify_bind_token(&state.config.hmac_secret, &q.token)?;

    let platform = minerva_db::queries::lti::find_platform_by_id(&state.db, payload.platform_id)
        .await?
        .ok_or_else(|| AppError::bad_request("lti.platform_not_found"))?;

    // Admins can bind anything. Teachers see their owned + co-taught
    // courses. Students/unknown users get an empty list -- the UI then
    // shows a "ask your teacher to launch this once" message.
    let user = minerva_db::queries::users::find_by_id(&state.db, payload.user_id)
        .await?
        .ok_or_else(|| AppError::bad_request("lti.bind_user_not_found"))?;

    let courses = if user.role == "admin" {
        minerva_db::queries::courses::list_all(&state.db).await?
    } else if user.role == "teacher" {
        minerva_db::queries::courses::list_for_teacher(&state.db, user.id).await?
    } else {
        Vec::new()
    };

    let is_teacher_role = lti::lti_roles_to_course_role(&payload.roles) == "teacher";

    Ok(Json(BindInfoResponse {
        platform_name: platform.name,
        context_id: payload.context_id,
        context_label: payload.context_label,
        context_title: payload.context_title,
        is_teacher_role,
        courses: courses
            .into_iter()
            .map(|c| BindInfoCourse {
                id: c.id,
                name: c.name,
            })
            .collect(),
    }))
}

#[derive(Debug, Deserialize)]
struct BindCompleteRequest {
    token: String,
    course_id: Uuid,
}

#[derive(Debug, Serialize)]
struct BindCompleteResponse {
    /// Embed URL to navigate to. Includes the signed embed token.
    redirect_url: String,
}

/// POST /lti/bind -- creates the (platform, context) → course binding and
/// returns an embed URL the frontend should redirect to. Returns JSON (not
/// HTML) because the call is XHR from the bind picker page; the frontend
/// performs the redirect itself.
async fn bind_complete(
    State(state): State<AppState>,
    Json(body): Json<BindCompleteRequest>,
) -> Result<Json<BindCompleteResponse>, AppError> {
    let payload = verify_bind_token(&state.config.hmac_secret, &body.token)?;

    let platform = minerva_db::queries::lti::find_platform_by_id(&state.db, payload.platform_id)
        .await?
        .ok_or_else(|| AppError::bad_request("lti.platform_not_found"))?;

    let user = minerva_db::queries::users::find_by_id(&state.db, payload.user_id)
        .await?
        .ok_or_else(|| AppError::bad_request("lti.bind_user_not_found"))?;

    // Re-check authorization: must be admin, course owner, or teacher
    // on the target course. Mirrors require_course_teacher but operating
    // on a UserRow rather than an auth-middleware User.
    let course = minerva_db::queries::courses::find_by_id(&state.db, body.course_id)
        .await?
        .ok_or(AppError::NotFound)?;
    let is_admin = user.role == "admin";
    let is_owner = course.owner_id == user.id;
    let is_teacher = is_admin
        || is_owner
        || minerva_db::queries::courses::is_course_teacher_strict(
            &state.db,
            body.course_id,
            user.id,
        )
        .await?;
    if !is_teacher {
        return Err(AppError::Forbidden);
    }

    // Idempotent-ish: if a binding already exists for this (platform, context),
    // reuse it instead of failing. The UNIQUE index guarantees at most one.
    let binding = if let Some(existing) =
        minerva_db::queries::lti::find_binding(&state.db, platform.id, &payload.context_id).await?
    {
        existing
    } else {
        minerva_db::queries::lti::create_binding(
            &state.db,
            Uuid::new_v4(),
            &minerva_db::queries::lti::CreateBinding {
                platform_id: platform.id,
                context_id: &payload.context_id,
                context_label: payload.context_label.as_deref(),
                context_title: payload.context_title.as_deref(),
                course_id: body.course_id,
                created_by: user.id,
            },
        )
        .await?
    };

    // Apply course membership + role suggestion as on a normal launch.
    let synthetic_claims = lti::LtiLaunchClaims {
        iss: String::new(),
        sub: String::new(),
        aud: lti::AudClaim::Single(String::new()),
        exp: 0,
        iat: 0,
        nonce: String::new(),
        name: None,
        email: None,
        given_name: None,
        family_name: None,
        message_type: None,
        version: None,
        deployment_id: None,
        roles: payload.roles.clone(),
        context: None,
        resource_link: None,
        custom: None,
        launch_presentation: None,
    };
    apply_course_membership(&state, binding.course_id, &user, &synthetic_claims).await?;

    let redirect_url =
        build_embed_redirect_url(&state, binding.course_id, &user, &payload.client_id)?;
    Ok(Json(BindCompleteResponse { redirect_url }))
}

// ---------------------------------------------------------------------------
// Admin (site-level) platforms
// ---------------------------------------------------------------------------

fn require_admin(user: &User) -> Result<(), AppError> {
    if !user.role.is_admin() {
        return Err(AppError::Forbidden);
    }
    Ok(())
}

/// GET /admin/lti/setup -- the same Moodle/Canvas tool config hints the
/// per-course flow offers, but for the site admin. Exposes tool URLs and the
/// recommended custom parameter (user_eppn) to copy into "Manage tools".
async fn admin_lti_setup(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
) -> Result<Json<LtiSetupResponse>, AppError> {
    require_admin(&user)?;
    Ok(Json(build_admin_setup_response(&state.config.base_url)))
}

fn build_admin_setup_response(base_url: &str) -> LtiSetupResponse {
    let config = build_moodle_config(base_url);
    LtiSetupResponse {
        steps: vec![
            "In Moodle, go to Site administration → Plugins → Activity modules → External tool → Manage tools, then 'configure a tool manually'.".into(),
            format!("Set Tool URL to: {}", config.tool_url),
            format!("Set LTI version to: {}", config.lti_version),
            format!("Set Public key type to: {}", config.public_key_type),
            format!("Set Public keyset to: {}", config.public_keyset_url),
            format!("Set Initiate login URL to: {}", config.initiate_login_url),
            format!("Set Redirection URI(s) to: {}", config.redirection_uris),
            format!(
                "Under Custom parameters, add: {}",
                config.custom_parameters,
            ),
            format!(
                "Under 'Show more...', set Icon URL to: {}",
                config.icon_url,
            ),
            "Save. Moodle will show the tool's registration details -- copy the Platform ID (issuer), Client ID, Deployment ID, and the platform endpoints.".into(),
            "Back in Minerva, create an LTI platform with those values. After that, teachers can add Minerva to any Moodle course and will be asked (on first launch) which Minerva course to bind to.".into(),
        ],
        moodle_tool_config: config,
    }
}

#[derive(Debug, Serialize)]
struct PlatformResponse {
    id: Uuid,
    name: String,
    issuer: String,
    client_id: String,
    deployment_id: Option<String>,
    auth_login_url: String,
    auth_token_url: String,
    platform_jwks_url: String,
    created_at: chrono::DateTime<chrono::Utc>,
    moodle_config: MoodleToolConfig,
}

fn platform_to_response(
    p: minerva_db::queries::lti::PlatformRow,
    base_url: &str,
) -> PlatformResponse {
    PlatformResponse {
        id: p.id,
        name: p.name,
        issuer: p.issuer,
        client_id: p.client_id,
        deployment_id: p.deployment_id,
        auth_login_url: p.auth_login_url,
        auth_token_url: p.auth_token_url,
        platform_jwks_url: p.platform_jwks_url,
        created_at: p.created_at,
        moodle_config: build_moodle_config(base_url),
    }
}

async fn list_platforms(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
) -> Result<Json<Vec<PlatformResponse>>, AppError> {
    require_admin(&user)?;
    let rows = minerva_db::queries::lti::list_platforms(&state.db).await?;
    Ok(Json(
        rows.into_iter()
            .map(|r| platform_to_response(r, &state.config.base_url))
            .collect(),
    ))
}

#[derive(Debug, Deserialize)]
struct CreatePlatformRequest {
    name: String,
    issuer: String,
    client_id: String,
    deployment_id: Option<String>,
    auth_login_url: Option<String>,
    auth_token_url: Option<String>,
    platform_jwks_url: Option<String>,
}

async fn create_platform(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Json(body): Json<CreatePlatformRequest>,
) -> Result<Json<PlatformResponse>, AppError> {
    require_admin(&user)?;

    let issuer = body.issuer.trim_end_matches('/');
    let auth_login_url = body
        .auth_login_url
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| format!("{}/mod/lti/auth.php", issuer));
    let auth_token_url = body
        .auth_token_url
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| format!("{}/mod/lti/token.php", issuer));
    let platform_jwks_url = body
        .platform_jwks_url
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| format!("{}/mod/lti/certs.php", issuer));

    // Guard against collisions with per-course registrations. See the
    // matching check in create_registration.
    if minerva_db::queries::lti::find_registration_by_issuer(&state.db, issuer, &body.client_id)
        .await?
        .is_some()
    {
        return Err(AppError::bad_request("lti.registration_already_exists"));
    }

    let id = Uuid::new_v4();
    let row = minerva_db::queries::lti::create_platform(
        &state.db,
        id,
        &minerva_db::queries::lti::CreatePlatform {
            name: &body.name,
            issuer,
            client_id: &body.client_id,
            deployment_id: body.deployment_id.as_deref(),
            auth_login_url: &auth_login_url,
            auth_token_url: &auth_token_url,
            platform_jwks_url: &platform_jwks_url,
            created_by: user.id,
        },
    )
    .await
    .map_err(|e| {
        if e.to_string().contains("duplicate key") {
            AppError::bad_request("lti.platform_duplicate")
        } else {
            AppError::Database(e)
        }
    })?;

    Ok(Json(platform_to_response(row, &state.config.base_url)))
}

async fn delete_platform(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(platform_id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, AppError> {
    require_admin(&user)?;
    minerva_db::queries::lti::delete_platform(&state.db, platform_id).await?;
    Ok(Json(serde_json::json!({ "deleted": true })))
}

#[derive(Debug, Serialize)]
struct BindingResponse {
    id: Uuid,
    platform_id: Uuid,
    context_id: String,
    context_label: Option<String>,
    context_title: Option<String>,
    course_id: Uuid,
    course_name: Option<String>,
    created_at: chrono::DateTime<chrono::Utc>,
}

async fn list_platform_bindings(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(platform_id): Path<Uuid>,
) -> Result<Json<Vec<BindingResponse>>, AppError> {
    require_admin(&user)?;

    let rows = minerva_db::queries::lti::list_bindings_for_platform(&state.db, platform_id).await?;
    let mut out = Vec::with_capacity(rows.len());
    for b in rows {
        let course_name = minerva_db::queries::courses::find_by_id(&state.db, b.course_id)
            .await?
            .map(|c| c.name);
        out.push(BindingResponse {
            id: b.id,
            platform_id: b.platform_id,
            context_id: b.context_id,
            context_label: b.context_label,
            context_title: b.context_title,
            course_id: b.course_id,
            course_name,
            created_at: b.created_at,
        });
    }
    Ok(Json(out))
}

async fn delete_platform_binding(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path((_platform_id, binding_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<serde_json::Value>, AppError> {
    require_admin(&user)?;
    minerva_db::queries::lti::delete_binding(&state.db, binding_id).await?;
    Ok(Json(serde_json::json!({ "deleted": true })))
}
