//! Integration API for external services (e.g. Moodle plugin).
//!
//! Authenticated via `Authorization: Bearer <api_key>` header where
//! the api_key is a per-course key created by teachers via the UI.
//! The key is hashed (SHA-256) and looked up in the `api_keys` table.
//!
//! Routes that include a course_id verify the key belongs to that course.
//! The `/courses` list endpoint returns only courses the key has access to.

use axum::extract::{Multipart, Path, State};
use axum::http::HeaderMap;
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use hmac::{Hmac, KeyInit, Mac};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::error::AppError;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/courses", get(list_courses))
        .route("/users/ensure", post(ensure_user))
        .route(
            "/courses/{course_id}/members",
            post(add_member).get(list_members),
        )
        .route(
            "/courses/{course_id}/members/by-eppn/{eppn}",
            delete(remove_member_by_eppn),
        )
        .route(
            "/courses/{course_id}/documents",
            post(upload_document)
                .get(list_documents)
                .layer(axum::extract::DefaultBodyLimit::max(
                    super::documents::MAX_UPLOAD_BYTES as usize,
                )),
        )
        .route("/courses/{course_id}/embed-token", post(create_embed_token))
        // Site-level provisioning: authenticated with a site integration key
        // (see site_integration_keys table / admin UI). Lets the LMS plugin
        // present a course picker and mint a regular per-course api_key
        // without the teacher having to visit Minerva first.
        .route("/site/courses-for-user", post(site_courses_for_user))
        .route("/site/provision", post(site_provision_course_key))
}

/// Extract and validate the API key from the Authorization header.
/// Returns the API key row (with course_id scope).
async fn authenticate(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<minerva_db::queries::api_keys::ApiKeyRow, AppError> {
    let auth_header = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .ok_or(AppError::Unauthorized)?;

    let token = auth_header
        .strip_prefix("Bearer ")
        .ok_or(AppError::Unauthorized)?;

    // Hash the provided key and look it up.
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    let key_hash = hex::encode(hasher.finalize());

    let api_key = minerva_db::queries::api_keys::find_by_hash(&state.db, &key_hash)
        .await?
        .ok_or(AppError::Unauthorized)?;

    // Update last_used_at (fire-and-forget).
    let db = state.db.clone();
    let key_id = api_key.id;
    tokio::spawn(async move {
        let _ = minerva_db::queries::api_keys::touch_last_used(&db, key_id).await;
    });

    Ok(api_key)
}

/// Authenticate and verify the key is scoped to the given course.
async fn authenticate_for_course(
    state: &AppState,
    headers: &HeaderMap,
    course_id: Uuid,
) -> Result<(), AppError> {
    let api_key = authenticate(state, headers).await?;
    if api_key.course_id != course_id {
        return Err(AppError::Forbidden);
    }
    Ok(())
}

// -- Responses --

#[derive(Serialize)]
struct CourseInfo {
    id: Uuid,
    name: String,
    description: Option<String>,
    active: bool,
}

#[derive(Serialize)]
struct UserInfo {
    id: Uuid,
    eppn: String,
    display_name: Option<String>,
    created: bool,
}

// -- Handlers --

/// List courses the API key has access to (i.e. the key's course).
async fn list_courses(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<CourseInfo>>, AppError> {
    let api_key = authenticate(&state, &headers).await?;

    // Return only the course this key is scoped to.
    let course = minerva_db::queries::courses::find_by_id(&state.db, api_key.course_id)
        .await?
        .ok_or(AppError::NotFound)?;

    Ok(Json(vec![CourseInfo {
        id: course.id,
        name: course.name,
        description: course.description,
        active: course.active,
    }]))
}

#[derive(Deserialize)]
struct EnsureUserRequest {
    eppn: String,
    display_name: Option<String>,
}

/// Find or create a user by eppn. Returns the user ID.
async fn ensure_user(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<EnsureUserRequest>,
) -> Result<Json<UserInfo>, AppError> {
    // Any valid API key can ensure users exist.
    authenticate(&state, &headers).await?;

    let eppn = body.eppn.trim().to_lowercase();
    let (user, created) = minerva_db::queries::users::find_or_create_by_eppn(
        &state.db,
        &eppn,
        body.display_name.as_deref(),
        "student",
        state.config.default_owner_daily_token_limit,
    )
    .await?;
    Ok(Json(UserInfo {
        id: user.id,
        eppn: user.eppn,
        display_name: user.display_name,
        created,
    }))
}

#[derive(Deserialize)]
struct AddMemberRequest {
    eppn: String,
    display_name: Option<String>,
    role: Option<String>,
}

/// Add a user to a course by eppn. Creates the user if they don't exist.
async fn add_member(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(course_id): Path<Uuid>,
    Json(body): Json<AddMemberRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    authenticate_for_course(&state, &headers, course_id).await?;

    minerva_db::queries::courses::find_by_id(&state.db, course_id)
        .await?
        .ok_or(AppError::NotFound)?;

    let eppn = body.eppn.trim().to_lowercase();
    let (user, _) = minerva_db::queries::users::find_or_create_by_eppn(
        &state.db,
        &eppn,
        body.display_name.as_deref(),
        "student",
        state.config.default_owner_daily_token_limit,
    )
    .await?;
    let user_id = user.id;

    let role = body.role.as_deref().unwrap_or("student");
    minerva_db::queries::courses::add_member(&state.db, course_id, user_id, role).await?;

    Ok(Json(
        serde_json::json!({ "added": true, "user_id": user_id }),
    ))
}

#[derive(Serialize)]
struct MemberInfo {
    user_id: Uuid,
    eppn: Option<String>,
    display_name: Option<String>,
    role: String,
}

/// List members of a course (for integration clients that need to diff).
async fn list_members(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(course_id): Path<Uuid>,
) -> Result<Json<Vec<MemberInfo>>, AppError> {
    authenticate_for_course(&state, &headers, course_id).await?;

    let rows = minerva_db::queries::courses::list_members(&state.db, course_id).await?;
    Ok(Json(
        rows.into_iter()
            .map(|r| MemberInfo {
                user_id: r.user_id,
                eppn: r.eppn,
                display_name: r.display_name,
                role: r.role,
            })
            .collect(),
    ))
}

/// Remove a user from a course by eppn.
async fn remove_member_by_eppn(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((course_id, eppn)): Path<(Uuid, String)>,
) -> Result<Json<serde_json::Value>, AppError> {
    authenticate_for_course(&state, &headers, course_id).await?;

    let eppn = eppn.trim().to_lowercase();
    let user = minerva_db::queries::users::find_by_eppn(&state.db, &eppn)
        .await?
        .ok_or(AppError::NotFound)?;

    let removed =
        minerva_db::queries::courses::remove_member(&state.db, course_id, user.id).await?;
    Ok(Json(serde_json::json!({ "removed": removed })))
}

#[derive(Serialize)]
struct DocumentInfo {
    id: Uuid,
    filename: String,
    status: String,
    chunk_count: Option<i32>,
    created_at: chrono::DateTime<chrono::Utc>,
}

/// List documents for a course.
async fn list_documents(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(course_id): Path<Uuid>,
) -> Result<Json<Vec<DocumentInfo>>, AppError> {
    authenticate_for_course(&state, &headers, course_id).await?;

    minerva_db::queries::courses::find_by_id(&state.db, course_id)
        .await?
        .ok_or(AppError::NotFound)?;

    let rows = minerva_db::queries::documents::list_by_course(&state.db, course_id).await?;
    Ok(Json(
        rows.into_iter()
            .map(|r| DocumentInfo {
                id: r.id,
                filename: r.filename,
                status: r.status,
                chunk_count: r.chunk_count,
                created_at: r.created_at,
            })
            .collect(),
    ))
}

/// Upload a document to a course (multipart form with a PDF file).
async fn upload_document(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(course_id): Path<Uuid>,
    mut multipart: Multipart,
) -> Result<Json<DocumentInfo>, AppError> {
    authenticate_for_course(&state, &headers, course_id).await?;

    minerva_db::queries::courses::find_by_id(&state.db, course_id)
        .await?
        .ok_or(AppError::NotFound)?;

    let field = multipart
        .next_field()
        .await
        .map_err(|e| {
            AppError::bad_request_with("doc.multipart_error", [("detail", e.to_string())])
        })?
        .ok_or_else(|| AppError::bad_request("doc.no_file"))?;

    let filename = field.file_name().unwrap_or("document").to_string();
    let content_type = field
        .content_type()
        .unwrap_or("application/octet-stream")
        .to_string();
    let data = field
        .bytes()
        .await
        .map_err(|e| AppError::bad_request_with("doc.read_failed", [("detail", e.to_string())]))?;

    let size_bytes = data.len() as i64;
    if size_bytes > super::documents::MAX_UPLOAD_BYTES {
        return Err(AppError::bad_request_with(
            "doc.file_too_large",
            [
                ("size_bytes", size_bytes.to_string()),
                (
                    "max_mb",
                    (super::documents::MAX_UPLOAD_BYTES / 1_000_000).to_string(),
                ),
            ],
        ));
    }

    let doc_id = Uuid::new_v4();

    // Save file to disk
    let dir = format!("{}/{}", state.config.docs_path, course_id);
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| AppError::Internal(format!("failed to create directory: {}", e)))?;

    let ext = super::documents::extension_from_filename(&filename);
    let file_path = format!("{}/{}.{}", dir, doc_id, ext);
    tokio::fs::write(&file_path, &data)
        .await
        .map_err(|e| AppError::Internal(format!("failed to write file: {}", e)))?;

    // Get course owner as uploader
    let course = minerva_db::queries::courses::find_by_id(&state.db, course_id)
        .await?
        .ok_or(AppError::NotFound)?;

    // Insert as 'pending'. The background worker will pick it up.
    let row = minerva_db::queries::documents::insert(
        &state.db,
        doc_id,
        course_id,
        &filename,
        &content_type,
        size_bytes,
        course.owner_id,
        None,
    )
    .await?;

    Ok(Json(DocumentInfo {
        id: row.id,
        filename: row.filename,
        status: row.status,
        chunk_count: row.chunk_count,
        created_at: row.created_at,
    }))
}

// -- Embed tokens --

type HmacSha256 = Hmac<Sha256>;

/// Embed token lifetime: 8 hours.
const EMBED_TOKEN_TTL_SECS: i64 = 8 * 3600;

#[derive(Deserialize)]
struct CreateEmbedTokenRequest {
    eppn: String,
    display_name: Option<String>,
}

#[derive(Serialize)]
struct EmbedTokenResponse {
    token: String,
    expires_at: chrono::DateTime<chrono::Utc>,
}

/// Create a short-lived HMAC-signed embed token for a student.
///
/// The token encodes `course_id:user_id:expires_ts` and is signed with
/// the server's HMAC secret. The `/api/embed/` routes validate it.
async fn create_embed_token(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(course_id): Path<Uuid>,
    Json(body): Json<CreateEmbedTokenRequest>,
) -> Result<Json<EmbedTokenResponse>, AppError> {
    authenticate_for_course(&state, &headers, course_id).await?;

    // Ensure the user exists.
    let eppn = body.eppn.trim().to_lowercase();
    let (user, _) = minerva_db::queries::users::find_or_create_by_eppn(
        &state.db,
        &eppn,
        body.display_name.as_deref(),
        "student",
        state.config.default_owner_daily_token_limit,
    )
    .await?;
    let user_id = user.id;

    // Ensure the user is a course member.
    if !minerva_db::queries::courses::is_member(&state.db, course_id, user_id).await? {
        minerva_db::queries::courses::add_member(&state.db, course_id, user_id, "student").await?;
    }

    let expires_at = chrono::Utc::now() + chrono::Duration::seconds(EMBED_TOKEN_TTL_SECS);
    let payload = format!("{}:{}:{}", course_id, user_id, expires_at.timestamp());

    let mut mac = HmacSha256::new_from_slice(state.config.hmac_secret.as_bytes())
        .map_err(|_| AppError::Internal("hmac key error".into()))?;
    mac.update(payload.as_bytes());
    let sig = hex::encode(mac.finalize().into_bytes());

    // Token format: base64url(course_id:user_id:expires_ts:signature)
    let token_raw = format!("{}:{}", payload, sig);
    let token = base64_url_encode(&token_raw);

    Ok(Json(EmbedTokenResponse { token, expires_at }))
}

/// Verify an embed token and return (course_id, user_id).
pub fn verify_embed_token(hmac_secret: &str, token: &str) -> Result<(Uuid, Uuid), AppError> {
    let decoded = base64_url_decode(token).map_err(|_| AppError::Unauthorized)?;

    // Format: course_id:user_id:expires_ts:signature
    let parts: Vec<&str> = decoded.splitn(4, ':').collect();
    if parts.len() != 4 {
        return Err(AppError::Unauthorized);
    }

    let course_id: Uuid = parts[0].parse().map_err(|_| AppError::Unauthorized)?;
    let user_id: Uuid = parts[1].parse().map_err(|_| AppError::Unauthorized)?;
    let expires_ts: i64 = parts[2].parse().map_err(|_| AppError::Unauthorized)?;
    let sig = parts[3];

    // Check expiry.
    let now = chrono::Utc::now().timestamp();
    if now > expires_ts {
        return Err(AppError::Unauthorized);
    }

    // Verify HMAC.
    let payload = format!("{}:{}:{}", course_id, user_id, expires_ts);
    let mut mac = HmacSha256::new_from_slice(hmac_secret.as_bytes())
        .map_err(|_| AppError::Internal("hmac key error".into()))?;
    mac.update(payload.as_bytes());
    let expected = hex::encode(mac.finalize().into_bytes());

    if sig != expected {
        return Err(AppError::Unauthorized);
    }

    Ok((course_id, user_id))
}

fn base64_url_encode(input: &str) -> String {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(input.as_bytes())
}

fn base64_url_decode(input: &str) -> Result<String, Box<dyn std::error::Error>> {
    use base64::Engine;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(input)?;
    Ok(String::from_utf8(bytes)?)
}

// ---------------------------------------------------------------------------
// Site-level provisioning
// ---------------------------------------------------------------------------

/// Authenticate a request as holding a valid site integration key.
/// Returns the key row so callers can enforce per-key policy
/// (notably `allowed_eppn_domains`). Touches `last_used_at` in the
/// background, same pattern as course keys.
async fn authenticate_site(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<minerva_db::queries::site_integration_keys::SiteIntegrationKeyRow, AppError> {
    let auth_header = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .ok_or(AppError::Unauthorized)?;

    let token = auth_header
        .strip_prefix("Bearer ")
        .ok_or(AppError::Unauthorized)?;

    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    let key_hash = hex::encode(hasher.finalize());

    let row = minerva_db::queries::site_integration_keys::find_by_hash(&state.db, &key_hash)
        .await?
        .ok_or(AppError::Unauthorized)?;

    let db = state.db.clone();
    let key_id = row.id;
    tokio::spawn(async move {
        let _ = minerva_db::queries::site_integration_keys::touch_last_used(&db, key_id).await;
    });

    Ok(row)
}

/// Reject acting eppns that fall outside a site key's domain allowlist.
/// No allowlist (None or empty) means "any". Otherwise the eppn must end
/// with `@<domain>` for at least one listed domain, comparing lowercase.
/// Eppn is assumed already lowercased by the caller (matches the rest of
/// the codebase, including `auth_middleware::upsert_user`).
fn enforce_eppn_domain(
    key: &minerva_db::queries::site_integration_keys::SiteIntegrationKeyRow,
    eppn: &str,
) -> Result<(), AppError> {
    let Some(domains) = key.allowed_eppn_domains.as_ref() else {
        return Ok(());
    };
    if domains.is_empty() {
        // Treat an empty array as "no restriction" -- see migration comment.
        return Ok(());
    }
    // `@domain` suffix, not `domain` substring, so `@evil-dsv.su.se` doesn't
    // silently match an allowlist of `dsv.su.se`.
    let matches = domains
        .iter()
        .any(|d| eppn.ends_with(&format!("@{}", d.to_lowercase())));
    if !matches {
        return Err(AppError::Forbidden);
    }
    Ok(())
}

#[derive(Deserialize)]
struct SiteCoursesForUserRequest {
    /// Caller-supplied eppn (e.g. the Moodle user's username). Lowercased
    /// before lookup -- matches the rest of the codebase.
    eppn: String,
}

#[derive(Serialize)]
struct SiteCourseInfo {
    id: Uuid,
    name: String,
    description: Option<String>,
}

#[derive(Serialize)]
struct SiteCoursesForUserResponse {
    /// Whether the eppn resolves to an existing Minerva user. When false,
    /// the plugin should tell the teacher to log into Minerva at least once
    /// first (otherwise there's no owner/teacher membership to enumerate).
    user_exists: bool,
    courses: Vec<SiteCourseInfo>,
}

/// List courses the given user can mint an api_key for -- i.e. courses they
/// own or teach. Strict (not ta) so the provisioning surface matches
/// `/courses/{id}/api-keys` (owner/admin only in the UI).
async fn site_courses_for_user(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<SiteCoursesForUserRequest>,
) -> Result<Json<SiteCoursesForUserResponse>, AppError> {
    let key = authenticate_site(&state, &headers).await?;

    let eppn = body.eppn.trim().to_lowercase();
    // Domain scoping: reject before any DB lookup so we don't leak whether
    // out-of-scope users exist. Return 403 rather than pretending they
    // don't exist, so a misconfigured plugin fails loudly.
    enforce_eppn_domain(&key, &eppn)?;

    let user = match minerva_db::queries::users::find_by_eppn(&state.db, &eppn).await? {
        Some(u) => u,
        None => {
            return Ok(Json(SiteCoursesForUserResponse {
                user_exists: false,
                courses: Vec::new(),
            }));
        }
    };

    // Admins see everything; otherwise restrict to courses they own or
    // have the teacher role on.
    let rows = if user.role == "admin" {
        minerva_db::queries::courses::list_all(&state.db).await?
    } else {
        minerva_db::queries::courses::list_for_teacher_strict(&state.db, user.id).await?
    };

    Ok(Json(SiteCoursesForUserResponse {
        user_exists: true,
        courses: rows
            .into_iter()
            .map(|c| SiteCourseInfo {
                id: c.id,
                name: c.name,
                description: c.description,
            })
            .collect(),
    }))
}

#[derive(Deserialize)]
struct SiteProvisionRequest {
    /// Acting user's eppn. The minted key is attributed to this user for
    /// audit purposes, and authorization is checked against their Minerva
    /// membership on the course.
    eppn: String,
    /// Human-readable name for the generated key (shows up in the teacher's
    /// api-keys list). Typically the Moodle course fullname.
    name: String,
    /// Minerva course the key should be scoped to. Caller should have picked
    /// this from `site_courses_for_user` -- we re-verify ownership anyway.
    minerva_course_id: Uuid,
}

#[derive(Serialize)]
struct SiteProvisionResponse {
    /// Full raw key, returned once; caller must persist it. Subsequent calls
    /// cannot retrieve the plaintext (only the hash is stored).
    key: String,
    key_id: Uuid,
    key_prefix: String,
    course: SiteCourseInfo,
}

/// Mint a course-scoped api_key for `eppn` on `minerva_course_id`, provided
/// the user is owner / admin / teacher on that course. The returned key
/// behaves exactly like one created via the course api-keys UI.
async fn site_provision_course_key(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<SiteProvisionRequest>,
) -> Result<Json<SiteProvisionResponse>, AppError> {
    let key = authenticate_site(&state, &headers).await?;

    let eppn = body.eppn.trim().to_lowercase();
    enforce_eppn_domain(&key, &eppn)?;
    let name = body.name.trim();
    if name.is_empty() || name.len() > 100 {
        return Err(AppError::bad_request("api_keys.name_invalid_length"));
    }

    let user = minerva_db::queries::users::find_by_eppn(&state.db, &eppn)
        .await?
        .ok_or_else(|| {
            AppError::bad_request_with("site_integration.user_not_found", [("eppn", eppn.clone())])
        })?;

    let course = minerva_db::queries::courses::find_by_id(&state.db, body.minerva_course_id)
        .await?
        .ok_or(AppError::NotFound)?;

    // Admin, owner, or strict-teacher can provision. Mirrors the
    // /courses/{id}/api-keys POST rules (owner/admin) but also lets a
    // co-teacher provision for a course they teach -- they already have
    // teacher-level trust on that course.
    let is_admin = user.role == "admin";
    let is_owner = course.owner_id == user.id;
    let authorized = is_admin
        || is_owner
        || minerva_db::queries::courses::is_course_teacher_strict(&state.db, course.id, user.id)
            .await?;
    if !authorized {
        return Err(AppError::Forbidden);
    }

    // Mint the key exactly like the course UI does (same prefix, same
    // random bytes, same hash).
    let id = Uuid::new_v4();
    let random_bytes: [u8; 16] = rand::random();
    let raw_key = format!("mnrv_{}", hex::encode(random_bytes));
    let key_prefix = format!("mnrv_{}...", &hex::encode(random_bytes)[..8]);

    let mut hasher = Sha256::new();
    hasher.update(raw_key.as_bytes());
    let key_hash = hex::encode(hasher.finalize());

    let row = minerva_db::queries::api_keys::insert(
        &state.db,
        id,
        course.id,
        user.id,
        name,
        &key_hash,
        &key_prefix,
    )
    .await?;

    Ok(Json(SiteProvisionResponse {
        key: raw_key,
        key_id: row.id,
        key_prefix: row.key_prefix,
        course: SiteCourseInfo {
            id: course.id,
            name: course.name,
            description: course.description,
        },
    }))
}
