//! LTI 1.3 Tool Provider endpoints.
//!
//! Public endpoints (no Shibboleth / API key auth):
//!   GET/POST /lti/login  -- OIDC third-party initiated login
//!   POST     /lti/launch -- Validate id_token, create session, redirect to embed
//!   GET      /lti/jwks   -- Serve tool public keys
//!
//! Course-level endpoints (behind auth_middleware, course teacher/owner):
//!   GET    /courses/{course_id}/lti          -- List LTI registrations
//!   POST   /courses/{course_id}/lti          -- Register LTI connection
//!   DELETE /courses/{course_id}/lti/{id}     -- Remove registration

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

/// Course-level routes for managing LTI registrations (teacher/owner only).
pub fn course_router() -> Router<AppState> {
    Router::new()
        .route("/lti/setup", get(lti_setup))
        .route("/lti", get(list_registrations).post(create_registration))
        .route("/lti/{registration_id}", delete(delete_registration))
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
    // Look up registration by issuer + client_id. client_id is required.
    let client_id = params
        .client_id
        .as_deref()
        .ok_or_else(|| AppError::BadRequest("client_id is required in login initiation".into()))?;

    let registration =
        minerva_db::queries::lti::find_registration_by_issuer(&state.db, &params.iss, client_id)
            .await?
            .ok_or_else(|| {
                AppError::BadRequest(format!(
                    "no LTI registration for issuer={} client_id={}",
                    params.iss, client_id
                ))
            })?;

    // Generate cryptographic state and nonce.
    let oidc_state = generate_random_string(32);
    let nonce = generate_random_string(32);

    minerva_db::queries::lti::create_launch(
        &state.db,
        Uuid::new_v4(),
        &oidc_state,
        &nonce,
        registration.id,
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
        registration.auth_login_url,
        urlencoding::encode(&registration.client_id),
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

async fn handle_launch(
    State(state): State<AppState>,
    Form(form): Form<LaunchForm>,
) -> Result<Response, AppError> {
    // 1. Consume the OIDC state (validates it exists and hasn't expired).
    let launch = minerva_db::queries::lti::consume_launch(&state.db, &form.state)
        .await?
        .ok_or_else(|| AppError::BadRequest("invalid or expired state".into()))?;

    // 2. Fetch the registration -- this tells us which Minerva course.
    let registration =
        minerva_db::queries::lti::find_registration_by_id(&state.db, launch.registration_id)
            .await?
            .ok_or_else(|| AppError::Internal("registration not found for launch".into()))?;

    let course_id = registration.course_id;

    // 3. Validate the JWT.
    let claims = lti::validate_launch_jwt(
        &registration,
        &form.id_token,
        &launch.nonce,
        &state.http_client,
    )
    .await?;

    // 4. Verify deployment_id if one was registered.
    if let Some(ref expected) = registration.deployment_id {
        match claims.deployment_id.as_deref() {
            Some(actual) if actual == expected.as_str() => {}
            Some(actual) => {
                return Err(AppError::BadRequest(format!(
                    "deployment_id mismatch: expected {}, got {}",
                    expected, actual
                )));
            }
            None => {
                return Err(AppError::BadRequest(
                    "JWT missing deployment_id claim".into(),
                ));
            }
        }
    }

    // 5. Verify the course exists.
    minerva_db::queries::courses::find_by_id(&state.db, course_id)
        .await?
        .ok_or(AppError::NotFound)?;

    // 6. Map user identity. Priority:
    //    a) Custom param "user_eppn" (Moodle can substitute $User.username)
    //    b) email claim
    //    c) Synthetic eppn from LTI sub
    let eppn = claims
        .custom
        .as_ref()
        .and_then(|c| c.get("user_eppn"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .or_else(|| claims.email.clone())
        .unwrap_or_else(|| format!("lti_{}_{}", registration.id, claims.sub))
        .to_lowercase();

    let display_name = claims.name.as_deref();

    // 7. Find or create the user.
    //    If the user already exists (e.g. via Shibboleth), reuse their record but
    //    do NOT modify their role or display name -- LTI should not alter existing accounts.
    //    Only populate display_name on newly created users.
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

    // 8. Add course membership if not already a member.
    //    New LTI arrivals always land as `student` regardless of what the
    //    remote LMS claims -- trusting cross-system role claims lets any
    //    Moodle site admin become a Minerva teacher on any linked course
    //    (Moodle admins have implicit access to every course they didn't
    //    enrol in, so "they couldn't get here without being a teacher
    //    already" doesn't hold). If the claim maps to `teacher`, we file
    //    a pending suggestion instead, which an existing course
    //    teacher/owner approves on the members tab. Declines are sticky
    //    per (user, role) via the unique index on the suggestions table.
    let claimed_role = lti::lti_roles_to_course_role(&claims.roles);
    let existing_role =
        minerva_db::queries::courses::get_member_role(&state.db, course_id, user.id).await?;
    if existing_role.is_none() {
        minerva_db::queries::courses::add_member(&state.db, course_id, user.id, "student").await?;
    }
    // Suggest elevation when the LTI claim is `teacher` and the current
    // membership (after the insert above) isn't already teacher. A prior
    // decline for the same (user, teacher) tuple silently suppresses the
    // suggestion via ON CONFLICT DO NOTHING in upsert_pending.
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

    // 9. Generate an embed token (reuses the existing HMAC mechanism).
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

    // 10. Redirect to the embed UI via JS (avoids token leaking in Referer).
    let embed_path = format!(
        "/embed/{}?token={}&lti_client_id={}",
        course_id,
        token,
        urlencoding::encode(&registration.client_id)
    );

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
            AppError::BadRequest("a registration with this issuer/client_id already exists".into())
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
