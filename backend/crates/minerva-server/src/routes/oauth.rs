//! OAuth 2.1 authorization server for the MCP endpoint (MCP 2026-07-28
//! authorization; see migration `20261104000005_mcp_oauth.sql`).
//!
//! Minerva is both the authorization server and the protected resource:
//!
//! * `/.well-known/oauth-protected-resource[/mcp]` and
//!   `/.well-known/oauth-authorization-server` describe the endpoints.
//! * `GET /api/oauth/authorize` sits behind Shibboleth like the rest of the
//!   site: it validates the request, parks it, and sends the browser to the
//!   SPA consent page `/oauth/requests/{id}`, which approves or denies through
//!   `/api/oauth/requests/{id}/...`.
//! * `/api/oauth/token`, `/register` and `/revoke` are called by the client
//!   itself and are exempt from Shibboleth (see `apache/minerva-app.conf`).
//!
//! Clients are public: no secrets, PKCE S256 only. A client identifies
//! itself either with a Client ID Metadata Document URL (preferred by the
//! spec; fetched and cached here) or, for older clients, an id from
//! dynamic registration (RFC 7591).
//!
//! Only staff can connect: teacher-or-above roles, never `ext:` accounts.

use axum::extract::{Extension, Form, Path, Query, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use base64::Engine;
use minerva_core::models::User;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::error::AppError;
use crate::state::AppState;

/// The one scope Minerva grants: read access to what the user can see.
pub const SCOPE: &str = "minerva:read";
const ACCESS_TTL_SECS: i64 = 60 * 60;
const REFRESH_TTL_DAYS: i64 = 30;
const CODE_TTL_SECS: i64 = 120;
const PENDING_TTL_SECS: i64 = 600;
/// Client ID Metadata Documents are refetched after this long.
const CIMD_REFRESH_HOURS: i64 = 24;
const CIMD_MAX_BYTES: usize = 64 * 1024;

/// The canonical URI of the MCP endpoint: the token audience (RFC 8707).
pub fn mcp_resource(base_url: &str) -> String {
    format!("{base_url}/mcp")
}

pub fn protected_resource_metadata_url(base_url: &str) -> String {
    format!("{base_url}/.well-known/oauth-protected-resource/mcp")
}

/// Who may connect an assistant: staff, never invited external accounts.
pub fn may_connect(user: &User) -> bool {
    user.role.is_teacher_or_above() && !user.eppn.starts_with("ext:") && !user.suspended
}

pub fn hash_token(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}

fn new_token() -> String {
    let bytes: [u8; 32] = rand::random();
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// RFC 7636: `BASE64URL(SHA256(verifier)) == challenge`.
fn pkce_matches(verifier: &str, challenge: &str) -> bool {
    // 43..128 unreserved characters (RFC 7636 section 4.1).
    if !(43..=128).contains(&verifier.len())
        || !verifier
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~'))
    {
        return false;
    }
    let digest = Sha256::digest(verifier.as_bytes());
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest) == challenge
}

/// Redirect URIs must be HTTPS or loopback HTTP (MCP authorization spec).
fn redirect_uri_allowed(uri: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(uri) else {
        return false;
    };
    if url.fragment().is_some() {
        return false;
    }
    match url.scheme() {
        "https" => url.host_str().is_some(),
        "http" => is_loopback(&url),
        _ => false,
    }
}

fn is_loopback(url: &reqwest::Url) -> bool {
    matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"))
}

/// Does a requested redirect URI match one the client registered? Exact,
/// except that a loopback URI matches on any port (RFC 8252 section 7.3):
/// native clients bind an ephemeral port per login and register the URI
/// without one, as Claude Code's metadata document does.
fn redirect_uri_registered(registered: &[String], requested: &str) -> bool {
    if !redirect_uri_allowed(requested) {
        return false;
    }
    if registered.iter().any(|r| r == requested) {
        return true;
    }
    let Ok(mut requested) = reqwest::Url::parse(requested) else {
        return false;
    };
    if requested.scheme() != "http" || !is_loopback(&requested) {
        return false;
    }
    requested.set_port(None).expect("http URL has a host");
    registered.iter().any(|r| {
        reqwest::Url::parse(r).is_ok_and(|mut r| {
            r.scheme() == "http" && is_loopback(&r) && {
                r.set_port(None).expect("http URL has a host");
                r == requested
            }
        })
    })
}

// ── Discovery ───────────────────────────────────────────────────────────

/// Mounted at the site root, outside `/api`.
pub fn well_known_router() -> Router<AppState> {
    Router::new()
        .route(
            "/.well-known/oauth-protected-resource",
            get(protected_resource_metadata),
        )
        .route(
            "/.well-known/oauth-protected-resource/mcp",
            get(protected_resource_metadata),
        )
        .route(
            "/.well-known/oauth-authorization-server",
            get(authorization_server_metadata),
        )
}

async fn protected_resource_metadata(State(state): State<AppState>) -> Json<serde_json::Value> {
    let base = &state.config.base_url;
    Json(serde_json::json!({
        "resource": mcp_resource(base),
        "authorization_servers": [base],
        "scopes_supported": [SCOPE],
        "bearer_methods_supported": ["header"],
        "resource_name": "Minerva",
    }))
}

async fn authorization_server_metadata(State(state): State<AppState>) -> Json<serde_json::Value> {
    let base = &state.config.base_url;
    Json(serde_json::json!({
        "issuer": base,
        "authorization_endpoint": format!("{base}/api/oauth/authorize"),
        "token_endpoint": format!("{base}/api/oauth/token"),
        "registration_endpoint": format!("{base}/api/oauth/register"),
        "revocation_endpoint": format!("{base}/api/oauth/revoke"),
        "response_types_supported": ["code"],
        "grant_types_supported": ["authorization_code", "refresh_token"],
        "code_challenge_methods_supported": ["S256"],
        "token_endpoint_auth_methods_supported": ["none"],
        "revocation_endpoint_auth_methods_supported": ["none"],
        "scopes_supported": [SCOPE],
        "client_id_metadata_document_supported": true,
        "authorization_response_iss_parameter_supported": true,
    }))
}

// ── Client resolution ───────────────────────────────────────────────────

/// A Client ID Metadata Document URL: HTTPS with a path, no fragment or
/// credentials, and a public DNS name. The host checks keep the fetch from
/// reaching cluster-internal services (SSRF); HTTPS certificate validation
/// covers the rest.
fn cimd_url(client_id: &str) -> Option<reqwest::Url> {
    let url = reqwest::Url::parse(client_id).ok()?;
    if url.scheme() != "https"
        || url.path() == "/"
        || url.path().is_empty()
        || url.fragment().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return None;
    }
    // `domain()` is None for IP literals: a metadata document lives at a
    // real domain, never an address.
    let name = url.domain()?.to_ascii_lowercase();
    let internal = !name.contains('.')
        || name == "localhost"
        || [
            ".local",
            ".internal",
            ".svc",
            ".cluster.local",
            ".localhost",
        ]
        .iter()
        .any(|s| name.ends_with(s));
    if internal {
        return None;
    }
    Some(url)
}

#[derive(Deserialize)]
struct ClientMetadataDocument {
    client_id: String,
    client_name: String,
    redirect_uris: Vec<String>,
    #[serde(default)]
    token_endpoint_auth_method: Option<String>,
}

/// Fetch and validate a Client ID Metadata Document.
async fn fetch_cimd(url: &reqwest::Url) -> Result<ClientMetadataDocument, String> {
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .map_err(|e| e.to_string())?;
    let mut resp = client
        .get(url.clone())
        .header(header::ACCEPT, "application/json")
        .send()
        .await
        .map_err(|e| format!("fetch failed: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!(
            "metadata document answered {}",
            resp.status().as_u16()
        ));
    }
    let mut body = Vec::new();
    while let Some(chunk) = resp.chunk().await.map_err(|e| e.to_string())? {
        if body.len() + chunk.len() > CIMD_MAX_BYTES {
            return Err("metadata document too large".into());
        }
        body.extend_from_slice(&chunk);
    }
    let doc: ClientMetadataDocument =
        serde_json::from_slice(&body).map_err(|e| format!("invalid metadata document: {e}"))?;
    if doc.client_id != url.as_str() {
        return Err("client_id in the document does not match its URL".into());
    }
    if doc.client_name.trim().is_empty() || doc.client_name.chars().count() > 200 {
        return Err("client_name missing or too long".into());
    }
    if doc.redirect_uris.is_empty() || !doc.redirect_uris.iter().all(|u| redirect_uri_allowed(u)) {
        return Err("redirect_uris must be HTTPS or loopback".into());
    }
    if doc
        .token_endpoint_auth_method
        .as_deref()
        .is_some_and(|m| m != "none")
    {
        return Err("only public clients (token_endpoint_auth_method none) are supported".into());
    }
    Ok(doc)
}

/// The client behind a `client_id`: a cached metadata document (refetched
/// when stale) or a dynamically registered client.
async fn resolve_client(
    state: &AppState,
    client_id: &str,
) -> Result<minerva_db::queries::oauth::ClientRow, String> {
    use minerva_db::queries::oauth as q;
    let cached = q::find_client(&state.db, client_id)
        .await
        .map_err(|e| e.to_string())?;
    if let Some(url) = cimd_url(client_id) {
        let fresh = cached.as_ref().is_some_and(|c| {
            c.fetched_at.is_some_and(|t| {
                t > chrono::Utc::now() - chrono::Duration::hours(CIMD_REFRESH_HOURS)
            })
        });
        if fresh {
            return Ok(cached.expect("checked above"));
        }
        let doc = fetch_cimd(&url).await?;
        return q::upsert_client(
            &state.db,
            client_id,
            doc.client_name.trim(),
            &doc.redirect_uris,
            "cimd",
        )
        .await
        .map_err(|e| e.to_string());
    }
    match cached {
        Some(c) if c.registration == "dcr" => Ok(c),
        _ => Err("unknown client_id".into()),
    }
}

// ── Authorization endpoint (behind Shibboleth) ──────────────────────────

/// Routes that need a signed-in user; mounted inside the authed api router.
pub fn authed_router() -> Router<AppState> {
    Router::new()
        .route("/oauth/authorize", get(authorize))
        .route("/oauth/requests/{id}", get(get_request))
        .route("/oauth/requests/{id}/approve", post(approve))
        .route("/oauth/requests/{id}/deny", post(deny))
        .route("/oauth/connections", get(list_connections))
        .route("/oauth/connections/{grant_id}", delete(revoke_connection))
}

/// Routes the client calls directly; no user session.
pub fn public_router() -> Router<AppState> {
    Router::new()
        .route("/oauth/token", post(token))
        .route("/oauth/register", post(register))
        .route("/oauth/revoke", post(revoke))
}

#[derive(Deserialize)]
struct AuthorizeQuery {
    response_type: Option<String>,
    client_id: Option<String>,
    redirect_uri: Option<String>,
    state: Option<String>,
    code_challenge: Option<String>,
    code_challenge_method: Option<String>,
    scope: Option<String>,
    resource: Option<String>,
}

/// Errors we cannot send back to the client (no trustworthy redirect URI
/// yet) are shown to the user instead.
fn authorize_error_page(message: &str) -> Response {
    (
        StatusCode::BAD_REQUEST,
        format!("Minerva could not start the connection: {message}"),
    )
        .into_response()
}

/// Send an OAuth error back to a validated redirect URI (RFC 6749 4.1.2.1,
/// with `iss` per RFC 9207).
fn redirect_with(redirect_uri: &str, params: &[(&str, &str)]) -> String {
    let mut url = reqwest::Url::parse(redirect_uri).expect("validated redirect_uri");
    {
        let mut q = url.query_pairs_mut();
        for (k, v) in params {
            q.append_pair(k, v);
        }
    }
    url.to_string()
}

async fn authorize(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Query(q): Query<AuthorizeQuery>,
) -> Response {
    let base = state.config.base_url.clone();
    let Some(client_id) = q.client_id.as_deref().filter(|s| !s.is_empty()) else {
        return authorize_error_page("client_id is missing");
    };
    let client = match resolve_client(&state, client_id).await {
        Ok(c) => c,
        Err(e) => return authorize_error_page(&e),
    };
    let Some(redirect_uri) = q
        .redirect_uri
        .as_deref()
        .filter(|u| redirect_uri_registered(&client.redirect_uris, u))
    else {
        return authorize_error_page("redirect_uri is not registered for this client");
    };
    let state_param = q.state.as_deref();
    let fail = |error: &str, description: &str| -> Response {
        let mut params = vec![
            ("error", error),
            ("error_description", description),
            ("iss", base.as_str()),
        ];
        if let Some(s) = state_param {
            params.push(("state", s));
        }
        Redirect::to(&redirect_with(redirect_uri, &params)).into_response()
    };

    if q.response_type.as_deref() != Some("code") {
        return fail("unsupported_response_type", "response_type must be code");
    }
    let Some(challenge) = q.code_challenge.as_deref().filter(|c| !c.is_empty()) else {
        return fail("invalid_request", "PKCE code_challenge is required");
    };
    if q.code_challenge_method.as_deref() != Some("S256") {
        return fail("invalid_request", "code_challenge_method must be S256");
    }
    let resource = mcp_resource(&base);
    if q.resource
        .as_deref()
        .is_some_and(|r| r.trim_end_matches('/') != resource)
    {
        return fail(
            "invalid_target",
            "resource must be this server's MCP endpoint",
        );
    }
    if q.scope
        .as_deref()
        .is_some_and(|s| s.split_whitespace().any(|x| x != SCOPE))
    {
        return fail("invalid_scope", "the only scope is minerva:read");
    }
    if !may_connect(&user) {
        return fail(
            "access_denied",
            "only staff can connect assistants to Minerva",
        );
    }

    let id = Uuid::new_v4();
    let pending = minerva_db::queries::oauth::NewPending {
        id,
        client_id: &client.client_id,
        user_id: user.id,
        redirect_uri,
        state: state_param,
        code_challenge: challenge,
        scope: SCOPE,
        resource: &resource,
        expires_at: chrono::Utc::now() + chrono::Duration::seconds(PENDING_TTL_SECS),
    };
    if let Err(e) = minerva_db::queries::oauth::insert_pending(&state.db, &pending).await {
        tracing::error!("oauth: storing pending authorization failed: {e}");
        return fail("server_error", "could not store the request");
    }
    Redirect::to(&format!("/oauth/requests/{id}")).into_response()
}

#[derive(Serialize)]
struct PendingView {
    id: Uuid,
    client_name: String,
    client_id: String,
    /// Where the browser goes after approval. Shown so a user can tell a
    /// local tool (`localhost`) from a hosted service.
    redirect_host: String,
    redirect_is_loopback: bool,
    scope: String,
}

async fn get_request(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(id): Path<Uuid>,
) -> Result<Json<PendingView>, AppError> {
    let p = minerva_db::queries::oauth::find_pending(&state.db, id, user.id)
        .await?
        .ok_or(AppError::NotFound)?;
    let url = reqwest::Url::parse(&p.redirect_uri)
        .map_err(|_| AppError::Internal("stored redirect_uri unparsable".into()))?;
    Ok(Json(PendingView {
        id: p.id,
        client_name: p.client_name,
        client_id: p.client_id,
        redirect_host: url.host_str().unwrap_or_default().to_string(),
        redirect_is_loopback: is_loopback(&url),
        scope: p.scope,
    }))
}

#[derive(Serialize)]
struct RedirectTo {
    redirect_to: String,
}

async fn approve(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(id): Path<Uuid>,
) -> Result<Json<RedirectTo>, AppError> {
    if !may_connect(&user) {
        return Err(AppError::Forbidden);
    }
    let p = minerva_db::queries::oauth::find_pending(&state.db, id, user.id)
        .await?
        .ok_or(AppError::NotFound)?;
    let code = new_token();
    minerva_db::queries::oauth::approve(
        &state.db,
        &p,
        Uuid::new_v4(),
        &hash_token(&code),
        chrono::Utc::now() + chrono::Duration::seconds(CODE_TTL_SECS),
    )
    .await
    .map_err(|e| match e {
        sqlx::Error::RowNotFound => AppError::NotFound,
        e => e.into(),
    })?;
    let mut params = vec![
        ("code", code.as_str()),
        ("iss", state.config.base_url.as_str()),
    ];
    if let Some(s) = p.state.as_deref() {
        params.push(("state", s));
    }
    Ok(Json(RedirectTo {
        redirect_to: redirect_with(&p.redirect_uri, &params),
    }))
}

async fn deny(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(id): Path<Uuid>,
) -> Result<Json<RedirectTo>, AppError> {
    let p = minerva_db::queries::oauth::find_pending(&state.db, id, user.id)
        .await?
        .ok_or(AppError::NotFound)?;
    minerva_db::queries::oauth::delete_pending(&state.db, id).await?;
    let mut params = vec![
        ("error", "access_denied"),
        ("error_description", "the user declined"),
        ("iss", state.config.base_url.as_str()),
    ];
    if let Some(s) = p.state.as_deref() {
        params.push(("state", s));
    }
    Ok(Json(RedirectTo {
        redirect_to: redirect_with(&p.redirect_uri, &params),
    }))
}

// ── Token endpoint ──────────────────────────────────────────────────────

#[derive(Deserialize)]
struct TokenRequest {
    grant_type: String,
    code: Option<String>,
    redirect_uri: Option<String>,
    code_verifier: Option<String>,
    refresh_token: Option<String>,
    client_id: Option<String>,
    resource: Option<String>,
}

/// RFC 6749 5.2 error body, never cached.
fn oauth_error(status: StatusCode, error: &str, description: &str) -> Response {
    let mut resp = (
        status,
        Json(serde_json::json!({ "error": error, "error_description": description })),
    )
        .into_response();
    resp.headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    resp
}

async fn issue_tokens(state: &AppState, grant_id: Uuid) -> Result<Response, sqlx::Error> {
    let access = new_token();
    let refresh = new_token();
    let now = chrono::Utc::now();
    minerva_db::queries::oauth::insert_token_pair(
        &state.db,
        grant_id,
        &hash_token(&access),
        now + chrono::Duration::seconds(ACCESS_TTL_SECS),
        &hash_token(&refresh),
        now + chrono::Duration::days(REFRESH_TTL_DAYS),
    )
    .await?;
    let mut resp = Json(serde_json::json!({
        "access_token": access,
        "token_type": "Bearer",
        "expires_in": ACCESS_TTL_SECS,
        "refresh_token": refresh,
        "scope": SCOPE,
    }))
    .into_response();
    resp.headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(resp)
}

async fn token(State(state): State<AppState>, Form(req): Form<TokenRequest>) -> Response {
    match token_inner(&state, req).await {
        Ok(resp) => resp,
        Err(e) => {
            tracing::error!("oauth token endpoint: {e}");
            oauth_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "server_error",
                "internal error",
            )
        }
    }
}

async fn token_inner(state: &AppState, req: TokenRequest) -> Result<Response, sqlx::Error> {
    use minerva_db::queries::oauth as q;
    let bad = |e: &str, d: &str| Ok(oauth_error(StatusCode::BAD_REQUEST, e, d));
    let resource_ok = |r: &Option<String>, granted: &str| {
        r.as_deref()
            .is_none_or(|r| r.trim_end_matches('/') == granted)
    };
    match req.grant_type.as_str() {
        "authorization_code" => {
            let (Some(code), Some(verifier)) = (req.code.as_deref(), req.code_verifier.as_deref())
            else {
                return bad("invalid_request", "code and code_verifier are required");
            };
            let Some(row) = q::consume_code(&state.db, &hash_token(code)).await? else {
                return bad("invalid_grant", "unknown code");
            };
            if row.consumed {
                // A code used twice was intercepted somewhere: kill the grant.
                q::revoke_grant(&state.db, row.grant_id).await?;
                return bad("invalid_grant", "code already used");
            }
            // The code is spent either way: a failed exchange leaves a grant
            // that can never get tokens, so end it rather than list a dead
            // connection.
            let failure = if row.expired || row.grant_revoked {
                Some(("invalid_grant", "code expired"))
            } else if req.client_id.as_deref() != Some(row.client_id.as_str()) {
                Some(("invalid_grant", "code was issued to another client"))
            } else if req.redirect_uri.as_deref() != Some(row.redirect_uri.as_str()) {
                Some(("invalid_grant", "redirect_uri does not match"))
            } else if !pkce_matches(verifier, &row.code_challenge) {
                Some(("invalid_grant", "PKCE verification failed"))
            } else if !resource_ok(&req.resource, &row.resource) {
                Some(("invalid_target", "resource does not match the grant"))
            } else {
                None
            };
            if let Some((error, description)) = failure {
                q::revoke_grant(&state.db, row.grant_id).await?;
                return bad(error, description);
            }
            issue_tokens(state, row.grant_id).await
        }
        "refresh_token" => {
            let Some(refresh) = req.refresh_token.as_deref() else {
                return bad("invalid_request", "refresh_token is required");
            };
            let Some(row) = q::rotate_refresh(&state.db, &hash_token(refresh)).await? else {
                return bad("invalid_grant", "unknown refresh token");
            };
            if row.rotated {
                // Refresh tokens are single-use (rotation for public
                // clients): a second use means it leaked.
                q::revoke_grant(&state.db, row.grant_id).await?;
                return bad("invalid_grant", "refresh token already used");
            }
            if row.expired || row.grant_revoked {
                return bad("invalid_grant", "refresh token expired");
            }
            if req.client_id.as_deref() != Some(row.client_id.as_str()) {
                return bad("invalid_grant", "refresh token belongs to another client");
            }
            if !resource_ok(&req.resource, &row.resource) {
                return bad("invalid_target", "resource does not match the grant");
            }
            issue_tokens(state, row.grant_id).await
        }
        _ => bad(
            "unsupported_grant_type",
            "use authorization_code or refresh_token",
        ),
    }
}

// ── Dynamic client registration (RFC 7591, kept for older clients) ──────

#[derive(Deserialize)]
struct RegistrationRequest {
    #[serde(default)]
    client_name: Option<String>,
    redirect_uris: Vec<String>,
    #[serde(default)]
    token_endpoint_auth_method: Option<String>,
}

async fn register(State(state): State<AppState>, Json(req): Json<RegistrationRequest>) -> Response {
    if req.redirect_uris.is_empty() || !req.redirect_uris.iter().all(|u| redirect_uri_allowed(u)) {
        return oauth_error(
            StatusCode::BAD_REQUEST,
            "invalid_redirect_uri",
            "redirect_uris must be HTTPS or loopback",
        );
    }
    if req
        .token_endpoint_auth_method
        .as_deref()
        .is_some_and(|m| m != "none")
    {
        return oauth_error(
            StatusCode::BAD_REQUEST,
            "invalid_client_metadata",
            "only public clients (token_endpoint_auth_method none) are supported",
        );
    }
    let name: String = req
        .client_name
        .as_deref()
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .unwrap_or("MCP client")
        .chars()
        .take(200)
        .collect();
    let client_id = format!("dcr-{}", hex::encode(rand::random::<[u8; 16]>()));
    match minerva_db::queries::oauth::upsert_client(
        &state.db,
        &client_id,
        &name,
        &req.redirect_uris,
        "dcr",
    )
    .await
    {
        Ok(c) => (
            StatusCode::CREATED,
            Json(serde_json::json!({
                "client_id": c.client_id,
                "client_name": c.client_name,
                "redirect_uris": c.redirect_uris,
                "token_endpoint_auth_method": "none",
                "grant_types": ["authorization_code", "refresh_token"],
                "response_types": ["code"],
                "client_id_issued_at": c.created_at.timestamp(),
            })),
        )
            .into_response(),
        Err(e) => {
            tracing::error!("oauth register: {e}");
            oauth_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "server_error",
                "internal error",
            )
        }
    }
}

// ── Revocation (RFC 7009) ───────────────────────────────────────────────

#[derive(Deserialize)]
struct RevokeRequest {
    token: String,
}

/// Revoking either token of a grant ends the whole connection. Always 200,
/// so callers cannot probe which tokens exist.
async fn revoke(State(state): State<AppState>, Form(req): Form<RevokeRequest>) -> StatusCode {
    if let Ok(Some(grant)) =
        minerva_db::queries::oauth::grant_of_token(&state.db, &hash_token(&req.token)).await
    {
        if let Err(e) = minerva_db::queries::oauth::revoke_grant(&state.db, grant).await {
            tracing::error!("oauth revoke: {e}");
        }
    }
    StatusCode::OK
}

// ── Connected apps (the user's own grants) ──────────────────────────────

#[derive(Serialize)]
struct ConnectionView {
    id: Uuid,
    client_name: String,
    client_id: String,
    connected_at: chrono::DateTime<chrono::Utc>,
    last_used_at: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Serialize)]
struct ConnectionsResponse {
    /// What the user pastes into their assistant as the connector URL.
    mcp_url: String,
    allowed: bool,
    connections: Vec<ConnectionView>,
}

async fn list_connections(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
) -> Result<Json<ConnectionsResponse>, AppError> {
    let rows = minerva_db::queries::oauth::list_connected(&state.db, user.id).await?;
    Ok(Json(ConnectionsResponse {
        mcp_url: mcp_resource(&state.config.base_url),
        allowed: may_connect(&user),
        connections: rows
            .into_iter()
            .map(|r| ConnectionView {
                id: r.grant_id,
                client_name: r.client_name,
                client_id: r.client_id,
                connected_at: r.created_at,
                last_used_at: r.last_used_at,
            })
            .collect(),
    }))
}

async fn revoke_connection(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(grant_id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, AppError> {
    if minerva_db::queries::oauth::revoke_own_grant(&state.db, grant_id, user.id).await? {
        Ok(Json(serde_json::json!({ "revoked": true })))
    } else {
        Err(AppError::NotFound)
    }
}

/// `WWW-Authenticate` for the MCP endpoint's 401 (RFC 9728 section 5.1).
pub fn www_authenticate(base_url: &str, invalid_token: bool) -> HeaderValue {
    let mut v = format!(
        "Bearer resource_metadata=\"{}\", scope=\"{SCOPE}\"",
        protected_resource_metadata_url(base_url)
    );
    if invalid_token {
        v.push_str(", error=\"invalid_token\"");
    }
    HeaderValue::from_str(&v).expect("ascii header")
}

/// Bearer token from `Authorization`. Never from the query string.
pub fn bearer(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
        .map(str::trim)
        .filter(|t| !t.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkce_s256_round_trip() {
        // RFC 7636 appendix B.
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        assert!(pkce_matches(
            verifier,
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        ));
        assert!(!pkce_matches(
            verifier,
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cX"
        ));
        assert!(!pkce_matches(
            "short",
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        ));
    }

    #[test]
    fn redirect_uris_must_be_https_or_loopback() {
        assert!(redirect_uri_allowed(
            "https://assistant.example/api/mcp/auth_callback"
        ));
        assert!(redirect_uri_allowed("http://localhost:33418/callback"));
        assert!(redirect_uri_allowed("http://127.0.0.1:9000/cb"));
        assert!(!redirect_uri_allowed("http://evil.example/cb"));
        assert!(!redirect_uri_allowed("https://x.example/cb#frag"));
        assert!(!redirect_uri_allowed("javascript:alert(1)"));
    }

    #[test]
    fn loopback_redirect_uris_match_on_any_port() {
        let registered = vec![
            "http://localhost/callback".to_string(),
            "http://127.0.0.1/callback".to_string(),
            "https://assistant.example/cb".to_string(),
        ];
        assert!(redirect_uri_registered(
            &registered,
            "http://localhost:54883/callback"
        ));
        assert!(redirect_uri_registered(
            &registered,
            "http://localhost/callback"
        ));
        assert!(redirect_uri_registered(
            &registered,
            "http://127.0.0.1:9000/callback"
        ));
        assert!(redirect_uri_registered(
            &registered,
            "https://assistant.example/cb"
        ));
        assert!(!redirect_uri_registered(
            &registered,
            "http://localhost:54883/other"
        ));
        assert!(!redirect_uri_registered(
            &registered,
            "http://[::1]:54883/callback"
        ));
        assert!(!redirect_uri_registered(
            &registered,
            "https://assistant.example:8443/cb"
        ));
        assert!(!redirect_uri_registered(
            &registered,
            "http://evil.example:80/callback"
        ));
    }

    #[test]
    fn metadata_document_urls_must_be_public_https_with_a_path() {
        assert!(cimd_url("https://app.example.com/oauth/client.json").is_some());
        assert!(cimd_url("https://app.example.com/").is_none());
        assert!(cimd_url("http://app.example.com/client.json").is_none());
        assert!(cimd_url("https://10.0.0.1/client.json").is_none());
        assert!(cimd_url("https://postgres/client.json").is_none());
        assert!(cimd_url("https://qdrant.minerva.svc.cluster.local/x").is_none());
        assert!(cimd_url("https://user:pw@app.example.com/c.json").is_none());
        assert!(cimd_url("dcr-abc").is_none());
    }

    #[test]
    fn www_authenticate_points_at_the_resource_metadata() {
        let v = www_authenticate("https://minerva.dsv.su.se", true);
        let s = v.to_str().unwrap();
        assert!(s.starts_with("Bearer resource_metadata=\"https://minerva.dsv.su.se/.well-known/oauth-protected-resource/mcp\""));
        assert!(s.contains("scope=\"minerva:read\""));
        assert!(s.contains("error=\"invalid_token\""));
    }
}
