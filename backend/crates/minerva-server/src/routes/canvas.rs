//! Canvas LMS integration: HTTP route surface.
//!
//! The discovery + sync *engine* (Files API + Modules merge, per-item
//! materialisation, `run_sync`) lives in the axum-free
//! `minerva_app_core::canvas` so the `minerva-scheduler` binary can run
//! Canvas auto-sync without linking this route tree. This module is just
//! the axum handlers + request/response DTOs that call into it.
//!
//! Course-level endpoints (behind auth_middleware, course teacher/owner):
//!   GET    /courses/{course_id}/canvas                     ; List connections
//!   POST   /courses/{course_id}/canvas                     ; Create connection
//!   DELETE /courses/{course_id}/canvas/{connection_id}      ; Remove connection
//!   POST   /courses/{course_id}/canvas/{connection_id}/sync ; Trigger sync
//!   GET    /courses/{course_id}/canvas/{connection_id}/files; Preview items

use axum::extract::{Path, State};
use axum::routing::{delete, get, patch, post};
use axum::{Extension, Json, Router};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::{AppError, LocalizedMessage};
use crate::state::AppState;
use minerva_app_core::canvas::{discover_items, paginate_json, run_sync, ItemKind, SyncResult};
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
