//! Per-course configuration of play.dsv.su.se designations to watch.
//!
//! A designation (e.g. `PROG1`) is a course code on play.dsv.su.se.
//! The transcript pipeline periodically scans each watched designation,
//! discovers new presentations, and auto-creates URL documents that the
//! existing transcript flow then processes.

use axum::extract::{Extension, Path, State};
use axum::routing::{delete, get};
use axum::{Json, Router};
use minerva_core::models::User;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::AppError;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/play-designations",
            get(list_play_designations).post(create_play_designation),
        )
        .route(
            "/play-designations/{designation_id}",
            delete(delete_play_designation),
        )
}

/// Router for the global (non-course-scoped) play course catalog.
/// Any authenticated user may read it; teachers use it for autocomplete.
pub fn catalog_router() -> Router<AppState> {
    Router::new().route("/play-courses-catalog", get(list_catalog))
}

#[derive(Serialize)]
struct PlayCourseCatalogEntry {
    code: String,
    name: String,
    updated_at: chrono::DateTime<chrono::Utc>,
}

async fn list_catalog(
    State(state): State<AppState>,
) -> Result<Json<Vec<PlayCourseCatalogEntry>>, AppError> {
    let rows = minerva_db::queries::play_course_catalog::list_all(&state.db).await?;
    Ok(Json(
        rows.into_iter()
            .map(|r| PlayCourseCatalogEntry {
                code: r.code,
                name: r.name,
                updated_at: r.updated_at,
            })
            .collect(),
    ))
}

#[derive(Serialize)]
struct PlayDesignationResponse {
    id: Uuid,
    designation: String,
    created_at: chrono::DateTime<chrono::Utc>,
    last_synced_at: Option<chrono::DateTime<chrono::Utc>>,
    last_error: Option<String>,
}

impl From<minerva_db::queries::play_designations::PlayDesignationRow> for PlayDesignationResponse {
    fn from(r: minerva_db::queries::play_designations::PlayDesignationRow) -> Self {
        Self {
            id: r.id,
            designation: r.designation,
            created_at: r.created_at,
            last_synced_at: r.last_synced_at,
            last_error: r.last_error,
        }
    }
}

#[derive(Deserialize)]
struct CreatePlayDesignationRequest {
    designation: String,
}

async fn authorize(state: &AppState, user: &User, course_id: Uuid) -> Result<(), AppError> {
    let course = minerva_db::queries::courses::find_by_id(&state.db, course_id)
        .await?
        .ok_or(AppError::NotFound)?;
    if course.owner_id != user.id && !user.role.is_admin() {
        return Err(AppError::Forbidden);
    }
    Ok(())
}

async fn list_play_designations(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(course_id): Path<Uuid>,
) -> Result<Json<Vec<PlayDesignationResponse>>, AppError> {
    authorize(&state, &user, course_id).await?;
    let rows = minerva_db::queries::play_designations::list_by_course(&state.db, course_id).await?;
    Ok(Json(rows.into_iter().map(Into::into).collect()))
}

async fn create_play_designation(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(course_id): Path<Uuid>,
    Json(body): Json<CreatePlayDesignationRequest>,
) -> Result<Json<PlayDesignationResponse>, AppError> {
    authorize(&state, &user, course_id).await?;

    let designation = body.designation.trim().to_string();
    if designation.is_empty() || designation.len() > 64 {
        return Err(AppError::bad_request("play.designation_invalid_length"));
    }
    // Designation codes can contain spaces (e.g. some play.dsv.su.se codes
    // include a term suffix like `IB907V VT26`). Slashes are still rejected
    // because the code ends up in URL paths downstream.
    if designation.chars().any(|c| c == '/' || c == '\\') {
        return Err(AppError::bad_request("play.designation_invalid_chars"));
    }

    let id = Uuid::new_v4();
    let row = match minerva_db::queries::play_designations::insert(
        &state.db,
        id,
        course_id,
        &designation,
        user.id,
    )
    .await
    {
        Ok(row) => row,
        Err(sqlx::Error::Database(db_err)) if db_err.is_unique_violation() => {
            return Err(AppError::bad_request("play.designation_duplicate"));
        }
        Err(e) => return Err(AppError::Database(e)),
    };

    Ok(Json(row.into()))
}

async fn delete_play_designation(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path((course_id, designation_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<serde_json::Value>, AppError> {
    authorize(&state, &user, course_id).await?;
    let deleted =
        minerva_db::queries::play_designations::delete(&state.db, designation_id, course_id)
            .await?;
    Ok(Json(serde_json::json!({ "deleted": deleted })))
}
