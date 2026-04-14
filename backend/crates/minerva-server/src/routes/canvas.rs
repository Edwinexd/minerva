//! Canvas LMS integration for syncing course files into Minerva.
//!
//! Teachers configure a Canvas API token and course ID, and Minerva
//! fetches files from Canvas via its REST API.
//!
//! Course-level endpoints (behind auth_middleware, course teacher/owner):
//!   GET    /courses/{course_id}/canvas                      -- List connections
//!   POST   /courses/{course_id}/canvas                      -- Create connection
//!   DELETE /courses/{course_id}/canvas/{connection_id}       -- Remove connection
//!   POST   /courses/{course_id}/canvas/{connection_id}/sync  -- Trigger sync
//!   GET    /courses/{course_id}/canvas/{connection_id}/files -- Preview Canvas files

use axum::extract::{Path, State};
use axum::routing::{delete, get, post};
use axum::{Extension, Json, Router};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::AppError;
use crate::state::AppState;
use minerva_core::models::User;

pub fn course_router() -> Router<AppState> {
    Router::new()
        .route("/canvas", get(list_connections).post(create_connection))
        .route("/canvas/{connection_id}", delete(delete_connection))
        .route("/canvas/{connection_id}/sync", post(trigger_sync))
        .route("/canvas/{connection_id}/files", get(list_canvas_files))
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
// Canvas API client helpers
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, Serialize, Clone)]
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
}

async fn fetch_canvas_files(
    http: &reqwest::Client,
    base_url: &str,
    api_token: &str,
    canvas_course_id: &str,
) -> Result<Vec<CanvasFile>, AppError> {
    let base = base_url.trim_end_matches('/');
    let mut all_files = Vec::new();
    let mut url = format!(
        "{}/api/v1/courses/{}/files?per_page=100",
        base, canvas_course_id
    );

    loop {
        let resp = http
            .get(&url)
            .header("Authorization", format!("Bearer {}", api_token))
            .send()
            .await
            .map_err(|e| AppError::BadRequest(format!("Canvas API request failed: {}", e)))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(AppError::BadRequest(format!(
                "Canvas API error ({}): {}",
                status, body
            )));
        }

        let link_header = resp
            .headers()
            .get("link")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());

        let files: Vec<CanvasFile> = resp
            .json()
            .await
            .map_err(|e| AppError::BadRequest(format!("Canvas API parse error: {}", e)))?;

        all_files.extend(files);

        match next_page_url(&link_header) {
            Some(next) => url = next,
            None => break,
        }
    }

    Ok(all_files)
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
// Route handlers
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct ConnectionResponse {
    id: Uuid,
    course_id: Uuid,
    name: String,
    canvas_base_url: String,
    canvas_course_id: String,
    auto_sync: bool,
    created_at: chrono::DateTime<chrono::Utc>,
    updated_at: chrono::DateTime<chrono::Utc>,
    last_synced_at: Option<chrono::DateTime<chrono::Utc>>,
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
        return Err(AppError::BadRequest("name is required".into()));
    }
    if body.canvas_base_url.trim().is_empty() {
        return Err(AppError::BadRequest("canvas_base_url is required".into()));
    }
    if body.canvas_api_token.trim().is_empty() {
        return Err(AppError::BadRequest("canvas_api_token is required".into()));
    }
    if body.canvas_course_id.trim().is_empty() {
        return Err(AppError::BadRequest("canvas_course_id is required".into()));
    }

    let base_url = body.canvas_base_url.trim().trim_end_matches('/');
    fetch_canvas_files(
        &state.http_client,
        base_url,
        body.canvas_api_token.trim(),
        body.canvas_course_id.trim(),
    )
    .await?;

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

#[derive(Serialize)]
struct CanvasFileInfo {
    id: String,
    filename: String,
    content_type: Option<String>,
    size: i64,
    already_synced: bool,
}

async fn list_canvas_files(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path((course_id, connection_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<Vec<CanvasFileInfo>>, AppError> {
    require_course_teacher(&state, course_id, &user).await?;

    let conn = minerva_db::queries::canvas::find_connection(&state.db, connection_id)
        .await?
        .ok_or(AppError::NotFound)?;

    if conn.course_id != course_id {
        return Err(AppError::NotFound);
    }

    let files = fetch_canvas_files(
        &state.http_client,
        &conn.canvas_base_url,
        &conn.canvas_api_token,
        &conn.canvas_course_id,
    )
    .await?;

    let synced = minerva_db::queries::canvas::synced_file_ids(&state.db, connection_id).await?;

    let result: Vec<CanvasFileInfo> = files
        .into_iter()
        .filter(|f| !f.hidden && !f.locked)
        .map(|f| {
            let fid = f.id.to_string();
            CanvasFileInfo {
                already_synced: synced.contains(&fid),
                id: fid,
                filename: f.display_name,
                content_type: f.content_type,
                size: f.size,
            }
        })
        .collect();

    Ok(Json(result))
}

#[derive(Serialize)]
struct SyncResult {
    synced: usize,
    skipped: usize,
    errors: Vec<String>,
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

    let course = minerva_db::queries::courses::find_by_id(&state.db, course_id)
        .await?
        .ok_or(AppError::NotFound)?;

    let files = fetch_canvas_files(
        &state.http_client,
        &conn.canvas_base_url,
        &conn.canvas_api_token,
        &conn.canvas_course_id,
    )
    .await?;

    let already_synced =
        minerva_db::queries::canvas::synced_file_ids(&state.db, connection_id).await?;

    let mut synced = 0usize;
    let mut skipped = 0usize;
    let mut errors = Vec::new();

    for file in &files {
        if file.hidden || file.locked {
            continue;
        }

        let fid = file.id.to_string();
        if already_synced.contains(&fid) {
            skipped += 1;
            continue;
        }

        let download_url = match &file.url {
            Some(u) => u.clone(),
            None => {
                skipped += 1;
                continue;
            }
        };

        let data =
            match download_canvas_file(&state.http_client, &conn.canvas_api_token, &download_url)
                .await
            {
                Ok(d) => d,
                Err(e) => {
                    errors.push(format!("{}: {}", file.display_name, e));
                    continue;
                }
            };

        let size_bytes = data.len() as i64;
        if size_bytes > super::documents::MAX_UPLOAD_BYTES {
            errors.push(format!(
                "{}: file too large ({} bytes)",
                file.display_name, size_bytes
            ));
            continue;
        }

        let doc_id = Uuid::new_v4();
        let content_type = file
            .content_type
            .as_deref()
            .unwrap_or("application/octet-stream");

        let dir = format!("{}/{}", state.config.docs_path, course_id);
        if let Err(e) = tokio::fs::create_dir_all(&dir).await {
            errors.push(format!("{}: mkdir failed: {}", file.display_name, e));
            continue;
        }

        let ext = super::documents::extension_from_filename(&file.filename);
        let file_path = format!("{}/{}.{}", dir, doc_id, ext);
        if let Err(e) = tokio::fs::write(&file_path, &data).await {
            errors.push(format!("{}: write failed: {}", file.display_name, e));
            continue;
        }

        match minerva_db::queries::documents::insert(
            &state.db,
            doc_id,
            course_id,
            &file.display_name,
            content_type,
            size_bytes,
            course.owner_id,
            None,
        )
        .await
        {
            Ok(_) => {}
            Err(e) => {
                errors.push(format!("{}: db insert failed: {}", file.display_name, e));
                continue;
            }
        }

        let _ = minerva_db::queries::canvas::insert_sync_log(
            &state.db,
            Uuid::new_v4(),
            connection_id,
            &fid,
            &file.display_name,
            file.content_type.as_deref(),
            Some(doc_id),
        )
        .await;

        synced += 1;
    }

    let _ = minerva_db::queries::canvas::update_last_synced(&state.db, connection_id).await;

    Ok(Json(SyncResult {
        synced,
        skipped,
        errors,
    }))
}
