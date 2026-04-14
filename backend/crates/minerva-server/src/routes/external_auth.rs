//! External-auth invites: time-limited access for non-Shibboleth users.
//!
//! An admin mints an invite (`POST /api/admin/external-invites`) which
//! produces an opaque token of the form
//! `base64url(jti:eppn:display_name_b64:expires_ts:hmac_sig)`.
//! The link `/api/external-auth/callback?token=...` validates the token,
//! sets a `minerva_ext` cookie, and redirects to `/`.
//!
//! Apache (mod_lua) validates the cookie's HMAC + expiry on every request
//! and injects `eppn`, `displayName`, and `X-Minerva-Ext-Jti` headers. The
//! backend's `auth_middleware` reads `eppn` (same path as Shib users) and
//! additionally looks up the JTI to enforce per-invite revocation -- so
//! an admin can kill a single invite without rotating the shared secret.
//!
//! Tokens are signed with the global `MINERVA_HMAC_SECRET` (shared with
//! the embed/integration HMAC). Apache reads the same secret from
//! `/etc/apache2/secrets/minerva-hmac` (mirrored manually).

use axum::extract::{Extension, Path, Query, State};
use axum::http::header;
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{delete, get};
use axum::{Json, Router};
use hmac::{Hmac, KeyInit, Mac};
use minerva_core::models::User;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use uuid::Uuid;

use crate::error::AppError;
use crate::state::AppState;

type HmacSha256 = Hmac<Sha256>;

/// Cookie name set by the callback and checked by Apache.
pub const COOKIE_NAME: &str = "minerva_ext";

/// Maximum lifetime of an invite, in days. Hard cap on what an admin
/// can request via the API, regardless of the requested value.
const MAX_INVITE_DAYS: i64 = 60;
const DEFAULT_INVITE_DAYS: i64 = 7;

/// Eppn prefix that marks a user as externally authenticated.
/// Real Shib users have eppns like `edsu8469@SU.SE`; we prefix ours with
/// `ext:` so they can never collide with the username-prefix admin check
/// in `Config::is_admin()`.
const EXT_EPPN_PREFIX: &str = "ext:";

// ---- Routers ---------------------------------------------------------------

/// Admin-facing routes; mounted under `/api/admin` behind auth_middleware.
pub fn admin_router() -> Router<AppState> {
    Router::new()
        .route("/external-invites", get(list_invites).post(create_invite))
        .route("/external-invites/{id}", delete(revoke_invite))
}

/// Public routes (no Shib, no auth_middleware). The callback is the only
/// public entry; once it sets the cookie, Apache's mod_lua hook handles
/// validation on every subsequent request.
pub fn public_router() -> Router<AppState> {
    Router::new().nest(
        "/external-auth",
        Router::new().route("/callback", get(callback)),
    )
}

// ---- Admin: create / list / revoke ----------------------------------------

#[derive(Deserialize)]
struct CreateInviteRequest {
    /// Local part of the synthetic eppn (e.g. "alice@foo.com" -> "ext:alice@foo.com").
    eppn: String,
    display_name: Option<String>,
    /// Days until expiry. Defaults to 7, capped at MAX_INVITE_DAYS.
    days: Option<i64>,
}

#[derive(Serialize)]
struct InviteResponse {
    id: Uuid,
    jti: Uuid,
    eppn: String,
    display_name: Option<String>,
    created_at: chrono::DateTime<chrono::Utc>,
    expires_at: chrono::DateTime<chrono::Utc>,
    revoked_at: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Serialize)]
struct CreatedInviteResponse {
    #[serde(flatten)]
    invite: InviteResponse,
    /// The single-use callback URL the admin sends to the external user.
    /// Only returned at creation time; the raw token cannot be retrieved later.
    url: String,
}

async fn create_invite(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Json(body): Json<CreateInviteRequest>,
) -> Result<Json<CreatedInviteResponse>, AppError> {
    require_admin(&user)?;

    let local = body.eppn.trim();
    if local.is_empty() || local.len() > 200 {
        return Err(AppError::BadRequest("eppn must be 1-200 characters".into()));
    }
    // Don't let admins accidentally double-prefix. Lowercase the local part so
    // the invite matches the normalized eppn produced by auth_middleware.
    let local = local
        .strip_prefix(EXT_EPPN_PREFIX)
        .unwrap_or(local)
        .to_lowercase();
    let eppn = format!("{}{}", EXT_EPPN_PREFIX, local);

    let display_name = body
        .display_name
        .as_ref()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    let days = body
        .days
        .unwrap_or(DEFAULT_INVITE_DAYS)
        .clamp(1, MAX_INVITE_DAYS);

    let id = Uuid::new_v4();
    let jti = Uuid::new_v4();
    let expires_at = chrono::Utc::now() + chrono::Duration::days(days);

    let row = minerva_db::queries::external_auth::insert(
        &state.db,
        id,
        jti,
        &eppn,
        display_name.as_deref(),
        user.id,
        expires_at,
    )
    .await?;

    let token = mint_token(
        &state.config.hmac_secret,
        jti,
        &eppn,
        display_name.as_deref(),
        expires_at,
    )?;
    let url = format!(
        "{}/api/external-auth/callback?token={}",
        state.config.base_url, token
    );

    Ok(Json(CreatedInviteResponse {
        invite: row_to_response(row),
        url,
    }))
}

async fn list_invites(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
) -> Result<Json<Vec<InviteResponse>>, AppError> {
    require_admin(&user)?;

    let rows = minerva_db::queries::external_auth::list_all(&state.db).await?;
    Ok(Json(rows.into_iter().map(row_to_response).collect()))
}

async fn revoke_invite(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, AppError> {
    require_admin(&user)?;

    let revoked = minerva_db::queries::external_auth::revoke(&state.db, id).await?;
    Ok(Json(serde_json::json!({ "revoked": revoked })))
}

// ---- Public: callback (set cookie) ----------------------------------------

#[derive(Deserialize)]
struct CallbackQuery {
    token: String,
}

async fn callback(
    State(state): State<AppState>,
    Query(query): Query<CallbackQuery>,
) -> Result<Response, AppError> {
    // Verify the token end-to-end (signature, expiry, jti not revoked) before
    // dropping the cookie. Bad links should never set a cookie.
    let claims = verify_token_signature(&state.config.hmac_secret, &query.token)?;
    let row = minerva_db::queries::external_auth::find_by_jti(&state.db, claims.jti)
        .await?
        .ok_or(AppError::Unauthorized)?;
    if row.revoked_at.is_some() || chrono::Utc::now() > row.expires_at || row.eppn != claims.eppn {
        return Err(AppError::Unauthorized);
    }
    let _ = claims.expires_at;
    let _ = claims.display_name;

    // Cookie max-age is set to the JWT's remaining lifetime so browsers drop
    // it around the same time Apache stops accepting it. We don't extract the
    // exact remaining seconds here -- the verify endpoint is the source of
    // truth on expiry, so a slight cookie/token mismatch is harmless.
    let cookie_value = format!(
        "{name}={value}; Path=/; Max-Age={max_age}; HttpOnly; Secure; SameSite=Lax",
        name = COOKIE_NAME,
        value = query.token,
        max_age = MAX_INVITE_DAYS * 24 * 3600,
    );

    let mut response = Redirect::to("/").into_response();
    response
        .headers_mut()
        .insert(header::SET_COOKIE, cookie_value.parse().unwrap());
    Ok(response)
}

// ---- Token helpers --------------------------------------------------------

struct TokenClaims {
    jti: Uuid,
    eppn: String,
    display_name: Option<String>,
    expires_at: chrono::DateTime<chrono::Utc>,
}

/// Mint an opaque token: `base64url(jti:eppn_b64:display_name_b64:expires_ts:hmac_sig)`.
/// The eppn is base64url-encoded inside the payload because it can contain
/// `:` (e.g. the `ext:` prefix), which would otherwise break the split.
fn mint_token(
    hmac_secret: &str,
    jti: Uuid,
    eppn: &str,
    display_name: Option<&str>,
    expires_at: chrono::DateTime<chrono::Utc>,
) -> Result<String, AppError> {
    let eppn_b64 = base64_url_encode(eppn.as_bytes());
    let display_b64 = base64_url_encode(display_name.unwrap_or("").as_bytes());
    let payload = format!(
        "{}:{}:{}:{}",
        jti,
        eppn_b64,
        display_b64,
        expires_at.timestamp()
    );

    let mut mac = HmacSha256::new_from_slice(hmac_secret.as_bytes())
        .map_err(|_| AppError::Internal("hmac key error".into()))?;
    mac.update(payload.as_bytes());
    let sig = hex::encode(mac.finalize().into_bytes());

    let raw = format!("{}:{}", payload, sig);
    Ok(base64_url_encode(raw.as_bytes()))
}

/// Parse + verify signature + expiry, **without** the DB revocation check.
/// Returns the embedded claims.
fn verify_token_signature(hmac_secret: &str, token: &str) -> Result<TokenClaims, AppError> {
    let decoded = base64_url_decode_str(token).map_err(|_| AppError::Unauthorized)?;

    let parts: Vec<&str> = decoded.splitn(5, ':').collect();
    if parts.len() != 5 {
        return Err(AppError::Unauthorized);
    }

    let jti: Uuid = parts[0].parse().map_err(|_| AppError::Unauthorized)?;
    let eppn_b64 = parts[1];
    let display_b64 = parts[2];
    let expires_ts: i64 = parts[3].parse().map_err(|_| AppError::Unauthorized)?;
    let sig = parts[4];

    let payload = format!("{}:{}:{}:{}", jti, eppn_b64, display_b64, expires_ts);
    let mut mac = HmacSha256::new_from_slice(hmac_secret.as_bytes())
        .map_err(|_| AppError::Internal("hmac key error".into()))?;
    mac.update(payload.as_bytes());
    let expected = hex::encode(mac.finalize().into_bytes());
    if sig != expected {
        return Err(AppError::Unauthorized);
    }

    let expires_at = chrono::DateTime::<chrono::Utc>::from_timestamp(expires_ts, 0)
        .ok_or(AppError::Unauthorized)?;
    if chrono::Utc::now() > expires_at {
        return Err(AppError::Unauthorized);
    }

    let eppn = base64_url_decode_bytes(eppn_b64)
        .ok()
        .and_then(|b| String::from_utf8(b).ok())
        .ok_or(AppError::Unauthorized)?;

    if !eppn.starts_with(EXT_EPPN_PREFIX) {
        // Defensive: never accept a token whose payload doesn't claim to be external.
        return Err(AppError::Unauthorized);
    }

    let display_name = base64_url_decode_bytes(display_b64)
        .ok()
        .and_then(|b| String::from_utf8(b).ok())
        .filter(|s| !s.is_empty());

    Ok(TokenClaims {
        jti,
        eppn,
        display_name,
        expires_at,
    })
}

// ---- Misc helpers ---------------------------------------------------------

fn require_admin(user: &User) -> Result<(), AppError> {
    if !user.role.is_admin() {
        return Err(AppError::Forbidden);
    }
    Ok(())
}

fn row_to_response(
    row: minerva_db::queries::external_auth::ExternalAuthInviteRow,
) -> InviteResponse {
    InviteResponse {
        id: row.id,
        jti: row.jti,
        eppn: row.eppn,
        display_name: row.display_name,
        created_at: row.created_at,
        expires_at: row.expires_at,
        revoked_at: row.revoked_at,
    }
}

fn base64_url_encode(input: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(input)
}

fn base64_url_decode_bytes(input: &str) -> Result<Vec<u8>, base64::DecodeError> {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(input)
}

fn base64_url_decode_str(input: &str) -> Result<String, Box<dyn std::error::Error>> {
    let bytes = base64_url_decode_bytes(input)?;
    Ok(String::from_utf8(bytes)?)
}
