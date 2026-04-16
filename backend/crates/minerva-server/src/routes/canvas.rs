//! Canvas LMS integration for syncing course resources into Minerva.
//!
//! Two discovery sources are queried in parallel because Canvas courses
//! commonly hide the Files tab from students while keeping Modules public:
//!   - `/api/v1/courses/{id}/files`     -- flat file listing (may 403)
//!   - `/api/v1/courses/{id}/modules`   -- module tree; items are
//!     File / Page / ExternalUrl / (SubHeader etc. ignored)
//!
//! Each failing source surfaces as a warning rather than killing the sync,
//! so a teacher whose course only exposes Modules still gets a working sync.
//!
//! Identity & dedup. `canvas_sync_log.canvas_file_id` is the per-connection
//! idempotency key. We prefix it to keep namespaces disjoint:
//! `file:{canvas_file_id}`, `page:{canvas_page_id}`, `url:{absolute_url}`.
//! Re-sync triggers when Canvas's `updated_at` advances past the stored
//! `canvas_updated_at` (Files and Pages). ExternalUrls carry no timestamp
//! and stay skip-once unless the underlying `source_url` changes.
//!
//! Course-level endpoints (behind auth_middleware, course teacher/owner):
//!   GET    /courses/{course_id}/canvas                      -- List connections
//!   POST   /courses/{course_id}/canvas                      -- Create connection
//!   DELETE /courses/{course_id}/canvas/{connection_id}       -- Remove connection
//!   POST   /courses/{course_id}/canvas/{connection_id}/sync  -- Trigger sync
//!   GET    /courses/{course_id}/canvas/{connection_id}/files -- Preview items

use axum::extract::{Path, State};
use axum::routing::{delete, get, patch, post};
use axum::{Extension, Json, Router};
use chrono::{DateTime, Utc};
use qdrant_client::qdrant::DeletePointsBuilder;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap};
use uuid::Uuid;

use crate::error::{AppError, LocalizedMessage};
use crate::state::AppState;
use minerva_core::models::User;

pub fn course_router() -> Router<AppState> {
    Router::new()
        .route("/canvas", get(list_connections).post(create_connection))
        .route("/canvas/lookup-courses", post(lookup_courses))
        .route("/canvas/{connection_id}", delete(delete_connection))
        .route("/canvas/{connection_id}/auto-sync", patch(update_auto_sync))
        .route("/canvas/{connection_id}/sync", post(trigger_sync))
        .route("/canvas/{connection_id}/files", get(list_canvas_items))
}

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

    let is_teacher =
        minerva_db::queries::courses::is_course_teacher(&state.db, course_id, user.id).await?;
    if is_teacher {
        return Ok(());
    }

    Err(AppError::Forbidden)
}

// ---------------------------------------------------------------------------
// Canvas API: raw wire types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, Clone)]
struct CanvasFile {
    id: i64,
    display_name: String,
    filename: String,
    #[serde(rename = "content-type")]
    content_type: Option<String>,
    size: i64,
    url: Option<String>,
    #[serde(default)]
    hidden: bool,
    #[serde(default)]
    locked: bool,
    updated_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Deserialize)]
struct CanvasModule {
    #[serde(default)]
    items: Vec<CanvasModuleItem>,
}

#[derive(Debug, Deserialize)]
struct CanvasModuleItem {
    #[serde(rename = "type")]
    item_type: String,
    title: Option<String>,
    /// File ID for File items, Page id (numeric) for Page items.
    content_id: Option<i64>,
    /// Page slug for Page items.
    page_url: Option<String>,
    /// Absolute URL for ExternalUrl items.
    external_url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CanvasPage {
    title: String,
    body: Option<String>,
    updated_at: Option<DateTime<Utc>>,
    #[serde(default)]
    published: bool,
}

// ---------------------------------------------------------------------------
// HTTP helpers
// ---------------------------------------------------------------------------

async fn canvas_get_json<T: serde::de::DeserializeOwned>(
    http: &reqwest::Client,
    url: &str,
    api_token: &str,
) -> Result<(T, Option<String>), AppError> {
    let resp = http
        .get(url)
        .header("Authorization", format!("Bearer {}", api_token))
        .send()
        .await
        .map_err(|e| {
            AppError::bad_request_with("canvas.api_request_failed", [("detail", e.to_string())])
        })?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(AppError::bad_request_with(
            "canvas.api_error",
            [("status", status.to_string()), ("body", body)],
        ));
    }

    let link_header = resp
        .headers()
        .get("link")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    let parsed: T = resp.json().await.map_err(|e| {
        AppError::bad_request_with("canvas.api_parse_failed", [("detail", e.to_string())])
    })?;

    Ok((parsed, next_page_url(&link_header)))
}

fn next_page_url(link_header: &Option<String>) -> Option<String> {
    let header = link_header.as_ref()?;
    for part in header.split(',') {
        let part = part.trim();
        if part.contains("rel=\"next\"") {
            let start = part.find('<')? + 1;
            let end = part.find('>')?;
            return Some(part[start..end].to_string());
        }
    }
    None
}

async fn paginate_json<T: serde::de::DeserializeOwned>(
    http: &reqwest::Client,
    first_url: String,
    api_token: &str,
) -> Result<Vec<T>, AppError> {
    let mut out = Vec::new();
    let mut url = first_url;
    loop {
        let (page, next): (Vec<T>, _) = canvas_get_json(http, &url, api_token).await?;
        out.extend(page);
        match next {
            Some(n) => url = n,
            None => break,
        }
    }
    Ok(out)
}

async fn fetch_canvas_files(
    http: &reqwest::Client,
    base_url: &str,
    api_token: &str,
    canvas_course_id: &str,
) -> Result<Vec<CanvasFile>, AppError> {
    let base = base_url.trim_end_matches('/');
    let url = format!(
        "{}/api/v1/courses/{}/files?per_page=100",
        base, canvas_course_id
    );
    paginate_json(http, url, api_token).await
}

async fn fetch_canvas_modules(
    http: &reqwest::Client,
    base_url: &str,
    api_token: &str,
    canvas_course_id: &str,
) -> Result<Vec<CanvasModule>, AppError> {
    let base = base_url.trim_end_matches('/');
    let url = format!(
        "{}/api/v1/courses/{}/modules?include[]=items&per_page=100",
        base, canvas_course_id
    );
    paginate_json(http, url, api_token).await
}

async fn fetch_file_by_id(
    http: &reqwest::Client,
    base_url: &str,
    api_token: &str,
    file_id: i64,
) -> Result<CanvasFile, AppError> {
    let base = base_url.trim_end_matches('/');
    let url = format!("{}/api/v1/files/{}", base, file_id);
    let (file, _) = canvas_get_json::<CanvasFile>(http, &url, api_token).await?;
    Ok(file)
}

async fn fetch_canvas_page(
    http: &reqwest::Client,
    base_url: &str,
    api_token: &str,
    canvas_course_id: &str,
    page_url_slug: &str,
) -> Result<CanvasPage, AppError> {
    let base = base_url.trim_end_matches('/');
    let url = format!(
        "{}/api/v1/courses/{}/pages/{}",
        base, canvas_course_id, page_url_slug
    );
    let (page, _) = canvas_get_json::<CanvasPage>(http, &url, api_token).await?;
    Ok(page)
}

async fn download_canvas_file(
    http: &reqwest::Client,
    api_token: &str,
    download_url: &str,
) -> Result<Vec<u8>, AppError> {
    let resp = http
        .get(download_url)
        .header("Authorization", format!("Bearer {}", api_token))
        .send()
        .await
        .map_err(|e| AppError::Internal(format!("Canvas file download failed: {}", e)))?;

    if !resp.status().is_success() {
        return Err(AppError::Internal(format!(
            "Canvas file download error: {}",
            resp.status()
        )));
    }

    let bytes = resp
        .bytes()
        .await
        .map_err(|e| AppError::Internal(format!("Canvas file read error: {}", e)))?;
    Ok(bytes.to_vec())
}

// ---------------------------------------------------------------------------
// Discovery: merge Files API + Modules into a single item list
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
enum ItemKind {
    File,
    Page,
    Url,
}

#[derive(Debug, Clone)]
struct DiscoveredItem {
    /// canvas_sync_log.canvas_file_id -- prefixed identity key.
    key: String,
    kind: ItemKind,
    display_name: String,
    content_type: Option<String>,
    size_hint: i64,
    updated_at: Option<DateTime<Utc>>,
    /// Seen in {"files_api","modules"} -- drives the preview "origin" chips.
    sources: BTreeSet<&'static str>,
    // File-only
    file: Option<CanvasFile>,
    // Page-only
    page_slug: Option<String>,
    // Url-only
    external_url: Option<String>,
}

struct Discovery {
    items: Vec<DiscoveredItem>,
    warnings: Vec<LocalizedMessage>,
}

/// Query both sources concurrently; per-source failures become warnings.
/// If BOTH sources fail, `items` is empty but `warnings` describes why so
/// the teacher sees something useful instead of an opaque error.
async fn discover_items(
    http: &reqwest::Client,
    base_url: &str,
    api_token: &str,
    canvas_course_id: &str,
) -> Discovery {
    let files_fut = fetch_canvas_files(http, base_url, api_token, canvas_course_id);
    let modules_fut = fetch_canvas_modules(http, base_url, api_token, canvas_course_id);
    let (files_res, modules_res) = tokio::join!(files_fut, modules_fut);

    let mut warnings = Vec::new();
    let mut by_key: HashMap<String, DiscoveredItem> = HashMap::new();

    match files_res {
        Ok(files) => {
            for f in files {
                if f.hidden || f.locked {
                    continue;
                }
                let key = format!("file:{}", f.id);
                let entry = by_key.entry(key.clone()).or_insert_with(|| DiscoveredItem {
                    key: key.clone(),
                    kind: ItemKind::File,
                    display_name: f.display_name.clone(),
                    content_type: f.content_type.clone(),
                    size_hint: f.size,
                    updated_at: f.updated_at,
                    sources: BTreeSet::new(),
                    file: Some(f.clone()),
                    page_slug: None,
                    external_url: None,
                });
                entry.sources.insert("files_api");
                if entry.file.is_none() {
                    entry.file = Some(f);
                }
            }
        }
        Err(e) => {
            warnings.push(LocalizedMessage::with(
                "canvas.warning.files_unavailable",
                [("detail", e.to_string())],
            ));
        }
    }

    let modules_ok = modules_res.is_ok();
    match modules_res {
        Ok(modules) => {
            for module in modules {
                for item in module.items {
                    match item.item_type.as_str() {
                        "File" => {
                            let Some(fid) = item.content_id else {
                                continue;
                            };
                            let key = format!("file:{}", fid);
                            let entry =
                                by_key.entry(key.clone()).or_insert_with(|| DiscoveredItem {
                                    key: key.clone(),
                                    kind: ItemKind::File,
                                    display_name: item
                                        .title
                                        .clone()
                                        .unwrap_or_else(|| format!("Canvas file {}", fid)),
                                    content_type: None,
                                    size_hint: 0,
                                    updated_at: None,
                                    sources: BTreeSet::new(),
                                    file: None,
                                    page_slug: None,
                                    external_url: None,
                                });
                            entry.sources.insert("modules");
                        }
                        "Page" => {
                            let Some(slug) = item.page_url else {
                                continue;
                            };
                            let key = format!("page:{}", slug);
                            let entry =
                                by_key.entry(key.clone()).or_insert_with(|| DiscoveredItem {
                                    key: key.clone(),
                                    kind: ItemKind::Page,
                                    display_name: item
                                        .title
                                        .clone()
                                        .unwrap_or_else(|| slug.clone()),
                                    content_type: Some("text/html".into()),
                                    size_hint: 0,
                                    updated_at: None,
                                    sources: BTreeSet::new(),
                                    file: None,
                                    page_slug: Some(slug.clone()),
                                    external_url: None,
                                });
                            entry.sources.insert("modules");
                        }
                        "ExternalUrl" => {
                            let Some(url) = item.external_url else {
                                continue;
                            };
                            let key = format!("url:{}", url);
                            let entry =
                                by_key.entry(key.clone()).or_insert_with(|| DiscoveredItem {
                                    key: key.clone(),
                                    kind: ItemKind::Url,
                                    display_name: item.title.clone().unwrap_or_else(|| url.clone()),
                                    content_type: Some("text/x-url".into()),
                                    size_hint: url.len() as i64,
                                    updated_at: None,
                                    sources: BTreeSet::new(),
                                    file: None,
                                    page_slug: None,
                                    external_url: Some(url.clone()),
                                });
                            entry.sources.insert("modules");
                        }
                        _ => {} // SubHeader / Assignment / Quiz / Discussion / ExternalTool -- ignored
                    }
                }
            }
        }
        Err(e) => {
            warnings.push(LocalizedMessage::with(
                "canvas.warning.modules_unavailable",
                [("detail", e.to_string())],
            ));
        }
    }

    if !modules_ok && by_key.is_empty() {
        // Both sources failed. warnings already carries the reasons.
    }

    let mut items: Vec<DiscoveredItem> = by_key.into_values().collect();
    items.sort_by(|a, b| {
        a.display_name
            .to_lowercase()
            .cmp(&b.display_name.to_lowercase())
    });

    Discovery { items, warnings }
}

// ---------------------------------------------------------------------------
// Route: list/create/delete connections
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct ConnectionResponse {
    id: Uuid,
    course_id: Uuid,
    name: String,
    canvas_base_url: String,
    canvas_course_id: String,
    auto_sync: bool,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    last_synced_at: Option<DateTime<Utc>>,
}

impl From<minerva_db::queries::canvas::ConnectionRow> for ConnectionResponse {
    fn from(row: minerva_db::queries::canvas::ConnectionRow) -> Self {
        Self {
            id: row.id,
            course_id: row.course_id,
            name: row.name,
            canvas_base_url: row.canvas_base_url,
            canvas_course_id: row.canvas_course_id,
            auto_sync: row.auto_sync,
            created_at: row.created_at,
            updated_at: row.updated_at,
            last_synced_at: row.last_synced_at,
        }
    }
}

async fn list_connections(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(course_id): Path<Uuid>,
) -> Result<Json<Vec<ConnectionResponse>>, AppError> {
    require_course_teacher(&state, course_id, &user).await?;

    let rows = minerva_db::queries::canvas::list_connections(&state.db, course_id).await?;
    Ok(Json(
        rows.into_iter().map(ConnectionResponse::from).collect(),
    ))
}

#[derive(Deserialize)]
struct CanvasCourseItem {
    id: i64,
    name: String,
    course_code: Option<String>,
}

#[derive(Serialize)]
struct CanvasCourseInfo {
    id: String,
    name: String,
    course_code: Option<String>,
}

#[derive(Deserialize)]
struct LookupCoursesRequest {
    canvas_base_url: String,
    canvas_api_token: String,
}

async fn lookup_courses(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(course_id): Path<Uuid>,
    Json(body): Json<LookupCoursesRequest>,
) -> Result<Json<Vec<CanvasCourseInfo>>, AppError> {
    require_course_teacher(&state, course_id, &user).await?;

    let base = body.canvas_base_url.trim().trim_end_matches('/');
    let token = body.canvas_api_token.trim();

    if base.is_empty() || token.is_empty() {
        return Err(AppError::bad_request("canvas.base_url_and_token_required"));
    }

    let url = format!("{}/api/v1/courses?per_page=100", base);
    let courses: Vec<CanvasCourseItem> = paginate_json(&state.http_client, url, token).await?;

    Ok(Json(
        courses
            .into_iter()
            .map(|c| CanvasCourseInfo {
                id: c.id.to_string(),
                name: c.name,
                course_code: c.course_code,
            })
            .collect(),
    ))
}

#[derive(Deserialize)]
struct CreateConnectionRequest {
    name: String,
    canvas_base_url: String,
    canvas_api_token: String,
    canvas_course_id: String,
    #[serde(default)]
    auto_sync: bool,
}

async fn create_connection(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(course_id): Path<Uuid>,
    Json(body): Json<CreateConnectionRequest>,
) -> Result<Json<ConnectionResponse>, AppError> {
    require_course_teacher(&state, course_id, &user).await?;

    if body.name.trim().is_empty() {
        return Err(AppError::bad_request("canvas.name_required"));
    }
    if body.canvas_base_url.trim().is_empty() {
        return Err(AppError::bad_request("canvas.base_url_required"));
    }
    if body.canvas_api_token.trim().is_empty() {
        return Err(AppError::bad_request("canvas.api_token_required"));
    }
    if body.canvas_course_id.trim().is_empty() {
        return Err(AppError::bad_request("canvas.course_id_required"));
    }

    let base_url = body.canvas_base_url.trim().trim_end_matches('/');

    // Validate credentials. A teacher-role course with Files hidden for
    // students will 403 on /files but /modules still works, so require at
    // least one source to succeed rather than demanding both.
    let disc = discover_items(
        &state.http_client,
        base_url,
        body.canvas_api_token.trim(),
        body.canvas_course_id.trim(),
    )
    .await;

    if disc.warnings.len() == 2 {
        return Err(AppError::bad_request("canvas.unreachable"));
    }

    let id = Uuid::new_v4();
    let input = minerva_db::queries::canvas::CreateConnection {
        course_id,
        name: body.name.trim(),
        canvas_base_url: base_url,
        canvas_api_token: body.canvas_api_token.trim(),
        canvas_course_id: body.canvas_course_id.trim(),
        auto_sync: body.auto_sync,
        created_by: user.id,
    };
    let row = minerva_db::queries::canvas::create_connection(&state.db, id, &input).await?;

    Ok(Json(ConnectionResponse::from(row)))
}

async fn delete_connection(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path((course_id, connection_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<serde_json::Value>, AppError> {
    require_course_teacher(&state, course_id, &user).await?;

    let conn = minerva_db::queries::canvas::find_connection(&state.db, connection_id)
        .await?
        .ok_or(AppError::NotFound)?;

    if conn.course_id != course_id {
        return Err(AppError::NotFound);
    }

    minerva_db::queries::canvas::delete_connection(&state.db, connection_id).await?;
    Ok(Json(serde_json::json!({ "deleted": true })))
}

// ---------------------------------------------------------------------------
// Route: preview
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct CanvasItemInfo {
    id: String,
    filename: String,
    kind: ItemKind,
    content_type: Option<String>,
    size: i64,
    sources: Vec<&'static str>,
    already_synced: bool,
    needs_resync: bool,
}

#[derive(Serialize)]
struct CanvasItemsResponse {
    items: Vec<CanvasItemInfo>,
    warnings: Vec<LocalizedMessage>,
}

async fn list_canvas_items(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path((course_id, connection_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<CanvasItemsResponse>, AppError> {
    require_course_teacher(&state, course_id, &user).await?;

    let conn = minerva_db::queries::canvas::find_connection(&state.db, connection_id)
        .await?
        .ok_or(AppError::NotFound)?;

    if conn.course_id != course_id {
        return Err(AppError::NotFound);
    }

    let disc = discover_items(
        &state.http_client,
        &conn.canvas_base_url,
        &conn.canvas_api_token,
        &conn.canvas_course_id,
    )
    .await;

    let existing =
        minerva_db::queries::canvas::synced_log_by_canvas_id(&state.db, connection_id).await?;

    let items = disc
        .items
        .into_iter()
        .map(|it| {
            let already_synced = existing.contains_key(&it.key);
            let needs_resync = match (existing.get(&it.key), it.updated_at) {
                (Some(row), Some(latest)) => match row.canvas_updated_at {
                    Some(prev) => latest > prev,
                    None => true, // never recorded: safest to resync once
                },
                _ => false,
            };
            CanvasItemInfo {
                id: it.key,
                filename: it.display_name,
                kind: it.kind,
                content_type: it.content_type,
                size: it.size_hint,
                sources: it.sources.iter().copied().collect(),
                already_synced,
                needs_resync,
            }
        })
        .collect();

    Ok(Json(CanvasItemsResponse {
        items,
        warnings: disc.warnings,
    }))
}

// ---------------------------------------------------------------------------
// Route: sync
// ---------------------------------------------------------------------------

#[derive(Serialize)]
pub struct SyncResult {
    pub synced: usize,
    pub resynced: usize,
    pub skipped: usize,
    pub errors: Vec<LocalizedMessage>,
    pub warnings: Vec<LocalizedMessage>,
}

async fn trigger_sync(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path((course_id, connection_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<SyncResult>, AppError> {
    require_course_teacher(&state, course_id, &user).await?;

    let conn = minerva_db::queries::canvas::find_connection(&state.db, connection_id)
        .await?
        .ok_or(AppError::NotFound)?;

    if conn.course_id != course_id {
        return Err(AppError::NotFound);
    }

    let result = run_sync(&state, &conn).await?;
    Ok(Json(result))
}

/// Core sync routine, callable from both the manual HTTP trigger and the
/// background auto-sync loop. Performs discovery, iterates items, updates
/// `last_synced_at`. Errors are per-item (returned in `SyncResult.errors`);
/// only infra failures (e.g. course row missing) bubble up.
pub async fn run_sync(
    state: &AppState,
    conn: &minerva_db::queries::canvas::ConnectionRow,
) -> Result<SyncResult, AppError> {
    let course = minerva_db::queries::courses::find_by_id(&state.db, conn.course_id)
        .await?
        .ok_or(AppError::NotFound)?;

    let disc = discover_items(
        &state.http_client,
        &conn.canvas_base_url,
        &conn.canvas_api_token,
        &conn.canvas_course_id,
    )
    .await;

    let existing = minerva_db::queries::canvas::synced_log_by_canvas_id(&state.db, conn.id).await?;

    let mut result = SyncResult {
        synced: 0,
        resynced: 0,
        skipped: 0,
        errors: Vec::new(),
        warnings: disc.warnings,
    };

    for item in disc.items {
        let prev = existing.get(&item.key);
        let outcome = match item.kind {
            ItemKind::File => sync_file(state, conn, course.owner_id, &item, prev).await,
            ItemKind::Page => sync_page(state, conn, course.owner_id, &item, prev).await,
            ItemKind::Url => {
                sync_url(state, conn, course.owner_id, conn.course_id, &item, prev).await
            }
        };

        match outcome {
            Ok(Outcome::Created) => result.synced += 1,
            Ok(Outcome::Resynced) => result.resynced += 1,
            Ok(Outcome::Skipped) => result.skipped += 1,
            Err(e) => result.errors.push(LocalizedMessage::with(
                "canvas.sync.item_failed",
                [
                    ("item", item.display_name.clone()),
                    ("detail", e.to_string()),
                ],
            )),
        }
    }

    let _ = minerva_db::queries::canvas::update_last_synced(&state.db, conn.id).await;

    Ok(result)
}

#[derive(Deserialize)]
struct AutoSyncRequest {
    auto_sync: bool,
}

async fn update_auto_sync(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path((course_id, connection_id)): Path<(Uuid, Uuid)>,
    Json(body): Json<AutoSyncRequest>,
) -> Result<Json<ConnectionResponse>, AppError> {
    require_course_teacher(&state, course_id, &user).await?;

    let conn = minerva_db::queries::canvas::find_connection(&state.db, connection_id)
        .await?
        .ok_or(AppError::NotFound)?;

    if conn.course_id != course_id {
        return Err(AppError::NotFound);
    }

    minerva_db::queries::canvas::set_auto_sync(&state.db, connection_id, body.auto_sync).await?;

    let row = minerva_db::queries::canvas::find_connection(&state.db, connection_id)
        .await?
        .ok_or(AppError::NotFound)?;

    Ok(Json(ConnectionResponse::from(row)))
}

enum Outcome {
    Created,
    Resynced,
    Skipped,
}

fn needs_resync(
    prev: Option<&minerva_db::queries::canvas::SyncLogRow>,
    latest: Option<DateTime<Utc>>,
) -> bool {
    let Some(row) = prev else {
        return false;
    };
    match (row.canvas_updated_at, latest) {
        (Some(p), Some(l)) => l > p,
        (None, Some(_)) => true,
        _ => false,
    }
}

async fn purge_chunks(state: &AppState, course_id: Uuid, doc_id: Uuid) -> Result<(), AppError> {
    let collection = format!("course_{}", course_id);
    let exists = state
        .qdrant
        .collection_exists(&collection)
        .await
        .unwrap_or(false);
    if !exists {
        return Ok(());
    }
    let filter = qdrant_client::qdrant::Filter::must([qdrant_client::qdrant::Condition::matches(
        "document_id",
        doc_id.to_string(),
    )]);
    state
        .qdrant
        .delete_points(
            DeletePointsBuilder::new(&collection)
                .points(filter)
                .wait(true),
        )
        .await
        .map_err(|e| AppError::Internal(format!("qdrant delete failed: {}", e)))?;
    Ok(())
}

async fn sync_file(
    state: &AppState,
    conn: &minerva_db::queries::canvas::ConnectionRow,
    owner_id: Uuid,
    item: &DiscoveredItem,
    prev: Option<&minerva_db::queries::canvas::SyncLogRow>,
) -> Result<Outcome, AppError> {
    // Canvas file id from the prefixed key.
    let file_id: i64 = item
        .key
        .strip_prefix("file:")
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| AppError::Internal(format!("bad canvas file key {}", item.key)))?;

    // If only Modules listed it, pull the full file record now.
    let file = match &item.file {
        Some(f) => f.clone(),
        None => fetch_file_by_id(
            &state.http_client,
            &conn.canvas_base_url,
            &conn.canvas_api_token,
            file_id,
        )
        .await
        .map_err(|e| {
            AppError::bad_request_with(
                "canvas.fetch_file_metadata_failed",
                [("detail", e.to_string())],
            )
        })?,
    };

    if file.hidden || file.locked {
        return Ok(Outcome::Skipped);
    }
    let Some(download_url) = &file.url else {
        return Ok(Outcome::Skipped);
    };

    let resync = needs_resync(prev, file.updated_at);
    if prev.is_some() && !resync {
        return Ok(Outcome::Skipped);
    }

    let data =
        download_canvas_file(&state.http_client, &conn.canvas_api_token, download_url).await?;
    let size_bytes = data.len() as i64;
    if size_bytes > super::documents::MAX_UPLOAD_BYTES {
        return Err(AppError::bad_request_with(
            "canvas.file_too_large",
            [("size_bytes", size_bytes.to_string())],
        ));
    }

    let content_type = file
        .content_type
        .as_deref()
        .unwrap_or("application/octet-stream");
    let dir = format!("{}/{}", state.config.docs_path, conn.course_id);
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| AppError::Internal(format!("mkdir failed: {}", e)))?;
    let ext = super::documents::extension_from_filename(&file.filename);

    if resync {
        let prev = prev.unwrap();
        let Some(doc_id) = prev.minerva_document_id else {
            // Log says synced but the doc row is gone; fall through to create.
            return create_file_doc(state, conn, owner_id, item, &file, &data, content_type, ext)
                .await;
        };
        purge_chunks(state, conn.course_id, doc_id).await?;
        let file_path = format!("{}/{}.{}", dir, doc_id, ext);
        tokio::fs::write(&file_path, &data)
            .await
            .map_err(|e| AppError::Internal(format!("write failed: {}", e)))?;
        minerva_db::queries::documents::reset_for_resync(
            &state.db,
            doc_id,
            &file.display_name,
            content_type,
            size_bytes,
        )
        .await?;
        minerva_db::queries::canvas::upsert_sync_log(
            &state.db,
            Uuid::new_v4(),
            conn.id,
            &item.key,
            &file.display_name,
            Some(content_type),
            Some(doc_id),
            file.updated_at,
        )
        .await?;
        return Ok(Outcome::Resynced);
    }

    create_file_doc(state, conn, owner_id, item, &file, &data, content_type, ext).await
}

#[allow(clippy::too_many_arguments)]
async fn create_file_doc(
    state: &AppState,
    conn: &minerva_db::queries::canvas::ConnectionRow,
    owner_id: Uuid,
    item: &DiscoveredItem,
    file: &CanvasFile,
    data: &[u8],
    content_type: &str,
    ext: &str,
) -> Result<Outcome, AppError> {
    let doc_id = Uuid::new_v4();
    let dir = format!("{}/{}", state.config.docs_path, conn.course_id);
    let file_path = format!("{}/{}.{}", dir, doc_id, ext);
    tokio::fs::write(&file_path, data)
        .await
        .map_err(|e| AppError::Internal(format!("write failed: {}", e)))?;

    minerva_db::queries::documents::insert(
        &state.db,
        doc_id,
        conn.course_id,
        &file.display_name,
        content_type,
        data.len() as i64,
        owner_id,
        None,
    )
    .await?;

    minerva_db::queries::canvas::upsert_sync_log(
        &state.db,
        Uuid::new_v4(),
        conn.id,
        &item.key,
        &file.display_name,
        Some(content_type),
        Some(doc_id),
        file.updated_at,
    )
    .await?;

    Ok(Outcome::Created)
}

async fn sync_page(
    state: &AppState,
    conn: &minerva_db::queries::canvas::ConnectionRow,
    owner_id: Uuid,
    item: &DiscoveredItem,
    prev: Option<&minerva_db::queries::canvas::SyncLogRow>,
) -> Result<Outcome, AppError> {
    let Some(slug) = &item.page_slug else {
        return Err(AppError::Internal("page item missing slug".into()));
    };

    let page = fetch_canvas_page(
        &state.http_client,
        &conn.canvas_base_url,
        &conn.canvas_api_token,
        &conn.canvas_course_id,
        slug,
    )
    .await?;

    if !page.published {
        return Ok(Outcome::Skipped);
    }

    let resync = needs_resync(prev, page.updated_at);
    if prev.is_some() && !resync {
        return Ok(Outcome::Skipped);
    }

    let body = page.body.clone().unwrap_or_default();
    let html = format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>{}</title></head><body><h1>{}</h1>{}</body></html>",
        html_escape(&page.title),
        html_escape(&page.title),
        body
    );
    let data = html.into_bytes();
    let size_bytes = data.len() as i64;
    if size_bytes > super::documents::MAX_UPLOAD_BYTES {
        return Err(AppError::bad_request_with(
            "canvas.page_too_large",
            [("size_bytes", size_bytes.to_string())],
        ));
    }

    let filename = format!("{}.html", safe_filename(&page.title));
    let content_type = "text/html";
    let dir = format!("{}/{}", state.config.docs_path, conn.course_id);
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| AppError::Internal(format!("mkdir failed: {}", e)))?;

    if resync {
        let prev = prev.unwrap();
        if let Some(doc_id) = prev.minerva_document_id {
            purge_chunks(state, conn.course_id, doc_id).await?;
            let file_path = format!("{}/{}.html", dir, doc_id);
            tokio::fs::write(&file_path, &data)
                .await
                .map_err(|e| AppError::Internal(format!("write failed: {}", e)))?;
            minerva_db::queries::documents::reset_for_resync(
                &state.db,
                doc_id,
                &filename,
                content_type,
                size_bytes,
            )
            .await?;
            minerva_db::queries::canvas::upsert_sync_log(
                &state.db,
                Uuid::new_v4(),
                conn.id,
                &item.key,
                &filename,
                Some(content_type),
                Some(doc_id),
                page.updated_at,
            )
            .await?;
            return Ok(Outcome::Resynced);
        }
    }

    let doc_id = Uuid::new_v4();
    let file_path = format!("{}/{}.html", dir, doc_id);
    tokio::fs::write(&file_path, &data)
        .await
        .map_err(|e| AppError::Internal(format!("write failed: {}", e)))?;

    minerva_db::queries::documents::insert(
        &state.db,
        doc_id,
        conn.course_id,
        &filename,
        content_type,
        size_bytes,
        owner_id,
        None,
    )
    .await?;

    minerva_db::queries::canvas::upsert_sync_log(
        &state.db,
        Uuid::new_v4(),
        conn.id,
        &item.key,
        &filename,
        Some(content_type),
        Some(doc_id),
        page.updated_at,
    )
    .await?;

    Ok(Outcome::Created)
}

async fn sync_url(
    state: &AppState,
    conn: &minerva_db::queries::canvas::ConnectionRow,
    owner_id: Uuid,
    course_id: Uuid,
    item: &DiscoveredItem,
    prev: Option<&minerva_db::queries::canvas::SyncLogRow>,
) -> Result<Outcome, AppError> {
    let Some(url) = &item.external_url else {
        return Err(AppError::Internal("url item missing url".into()));
    };

    // The sync log already covers per-connection idempotency. The
    // documents.source_url unique index additionally protects against
    // a manually-added URL doc colliding with the Canvas-sourced one.
    if prev.is_some() {
        return Ok(Outcome::Skipped);
    }

    if let Some(existing) =
        minerva_db::queries::documents::find_by_course_source_url(&state.db, course_id, url).await?
    {
        minerva_db::queries::canvas::upsert_sync_log(
            &state.db,
            Uuid::new_v4(),
            conn.id,
            &item.key,
            &existing.filename,
            Some("text/x-url"),
            Some(existing.id),
            None,
        )
        .await?;
        return Ok(Outcome::Skipped);
    }

    let doc_id = Uuid::new_v4();
    let dir = format!("{}/{}", state.config.docs_path, course_id);
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| AppError::Internal(format!("mkdir failed: {}", e)))?;
    let file_path = format!("{}/{}.url", dir, doc_id);
    tokio::fs::write(&file_path, url.as_bytes())
        .await
        .map_err(|e| AppError::Internal(format!("write failed: {}", e)))?;

    let filename = format!("{}.url", safe_filename(&item.display_name));
    let insert = minerva_db::queries::documents::insert(
        &state.db,
        doc_id,
        course_id,
        &filename,
        "text/x-url",
        url.len() as i64,
        owner_id,
        Some(url.as_str()),
    )
    .await;

    let row = match insert {
        Ok(row) => row,
        Err(sqlx::Error::Database(db_err)) if db_err.is_unique_violation() => {
            let _ = tokio::fs::remove_file(&file_path).await;
            let existing = minerva_db::queries::documents::find_by_course_source_url(
                &state.db, course_id, url,
            )
            .await?
            .ok_or_else(|| {
                AppError::Internal("unique violation on source_url but no match".into())
            })?;
            minerva_db::queries::canvas::upsert_sync_log(
                &state.db,
                Uuid::new_v4(),
                conn.id,
                &item.key,
                &existing.filename,
                Some("text/x-url"),
                Some(existing.id),
                None,
            )
            .await?;
            return Ok(Outcome::Skipped);
        }
        Err(e) => return Err(e.into()),
    };

    minerva_db::queries::canvas::upsert_sync_log(
        &state.db,
        Uuid::new_v4(),
        conn.id,
        &item.key,
        &row.filename,
        Some("text/x-url"),
        Some(row.id),
        None,
    )
    .await?;

    Ok(Outcome::Created)
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn safe_filename(s: &str) -> String {
    let cleaned: String = s
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == ' ' || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let cleaned = cleaned.trim();
    if cleaned.is_empty() {
        "untitled".into()
    } else if cleaned.len() > 180 {
        cleaned[..180].to_string()
    } else {
        cleaned.to_string()
    }
}
