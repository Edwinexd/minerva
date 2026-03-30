//! Integration API for external services (e.g. Moodle plugin).
//!
//! Authenticated via `Authorization: Bearer <MINERVA_API_KEY>` header.
//! Provides admin-level access for user management, enrollment sync,
//! course listing, and document upload.

use axum::extract::{Multipart, Path, State};
use axum::http::HeaderMap;
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use uuid::Uuid;

use crate::error::AppError;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/courses", get(list_courses))
        .route("/users/ensure", post(ensure_user))
        .route("/courses/{course_id}/members", post(add_member))
        .route(
            "/courses/{course_id}/members/by-eppn/{eppn}",
            delete(remove_member_by_eppn),
        )
        .route(
            "/courses/{course_id}/documents",
            post(upload_document).get(list_documents),
        )
}

/// Validates the API key from Authorization header.
pub fn validate_api_key(state: &AppState, headers: &HeaderMap) -> Result<(), AppError> {
    let configured_key = state
        .config
        .api_key
        .as_ref()
        .ok_or(AppError::Internal("integration API not configured".into()))?;

    let auth_header = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .ok_or(AppError::Unauthorized)?;

    let token = auth_header
        .strip_prefix("Bearer ")
        .ok_or(AppError::Unauthorized)?;

    if token != configured_key {
        return Err(AppError::Unauthorized);
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

/// List all active courses.
async fn list_courses(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<CourseInfo>>, AppError> {
    validate_api_key(&state, &headers)?;

    let rows = minerva_db::queries::courses::list_all(&state.db).await?;
    Ok(Json(
        rows.into_iter()
            .map(|r| CourseInfo {
                id: r.id,
                name: r.name,
                description: r.description,
                active: r.active,
            })
            .collect(),
    ))
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
    validate_api_key(&state, &headers)?;

    let existing = minerva_db::queries::users::find_by_eppn(&state.db, &body.eppn).await?;
    match existing {
        Some(user) => Ok(Json(UserInfo {
            id: user.id,
            eppn: user.eppn,
            display_name: user.display_name,
            created: false,
        })),
        None => {
            let id = Uuid::new_v4();
            minerva_db::queries::users::insert(
                &state.db,
                id,
                &body.eppn,
                body.display_name.as_deref(),
                "student",
            )
            .await?;
            Ok(Json(UserInfo {
                id,
                eppn: body.eppn,
                display_name: body.display_name,
                created: true,
            }))
        }
    }
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
    validate_api_key(&state, &headers)?;

    // Verify course exists
    minerva_db::queries::courses::find_by_id(&state.db, course_id)
        .await?
        .ok_or(AppError::NotFound)?;

    // Ensure user exists
    let user = minerva_db::queries::users::find_by_eppn(&state.db, &body.eppn).await?;
    let user_id = match user {
        Some(u) => u.id,
        None => {
            let id = Uuid::new_v4();
            minerva_db::queries::users::insert(
                &state.db,
                id,
                &body.eppn,
                body.display_name.as_deref(),
                "student",
            )
            .await?;
            id
        }
    };

    let role = body.role.as_deref().unwrap_or("student");
    minerva_db::queries::courses::add_member(&state.db, course_id, user_id, role).await?;

    Ok(Json(
        serde_json::json!({ "added": true, "user_id": user_id }),
    ))
}

/// Remove a user from a course by eppn.
async fn remove_member_by_eppn(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((course_id, eppn)): Path<(Uuid, String)>,
) -> Result<Json<serde_json::Value>, AppError> {
    validate_api_key(&state, &headers)?;

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
    validate_api_key(&state, &headers)?;

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
    validate_api_key(&state, &headers)?;

    minerva_db::queries::courses::find_by_id(&state.db, course_id)
        .await?
        .ok_or(AppError::NotFound)?;

    let field = multipart
        .next_field()
        .await
        .map_err(|e| AppError::BadRequest(format!("multipart error: {}", e)))?
        .ok_or_else(|| AppError::BadRequest("no file provided".to_string()))?;

    let filename = field.file_name().unwrap_or("document.pdf").to_string();
    let content_type = field
        .content_type()
        .unwrap_or("application/pdf")
        .to_string();
    let data = field
        .bytes()
        .await
        .map_err(|e| AppError::BadRequest(format!("failed to read file: {}", e)))?;

    let size_bytes = data.len() as i64;
    let doc_id = Uuid::new_v4();

    // Save file to disk
    let dir = format!("{}/{}", state.config.docs_path, course_id);
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| AppError::Internal(format!("failed to create directory: {}", e)))?;

    let file_path = format!("{}/{}.pdf", dir, doc_id);
    tokio::fs::write(&file_path, &data)
        .await
        .map_err(|e| AppError::Internal(format!("failed to write file: {}", e)))?;

    // Get course owner as uploader
    let course = minerva_db::queries::courses::find_by_id(&state.db, course_id)
        .await?
        .ok_or(AppError::NotFound)?;

    let row = minerva_db::queries::documents::insert(
        &state.db,
        doc_id,
        course_id,
        &filename,
        &content_type,
        size_bytes,
        course.owner_id,
    )
    .await?;

    // Spawn background processing
    let db = state.db.clone();
    let qdrant = Arc::clone(&state.qdrant);
    let api_key = state.config.openai_api_key.clone();
    let fname = filename.clone();
    let fpath = file_path.clone();

    tokio::spawn(async move {
        let client = reqwest::Client::new();
        let path = std::path::Path::new(&fpath);

        match minerva_ingest::pipeline::process_document(
            &db, &qdrant, &client, &api_key, doc_id, course_id, path, &fname,
        )
        .await
        {
            Ok(result) => {
                tracing::info!(
                    "integration: document {} processed: {} chunks",
                    doc_id,
                    result.chunk_count,
                );
            }
            Err(e) => {
                tracing::error!("integration: document {} processing failed: {}", doc_id, e);
                let _ = sqlx::query(
                    "UPDATE documents SET status = 'failed', error_msg = $1 WHERE id = $2",
                )
                .bind(&e)
                .bind(doc_id)
                .execute(&db)
                .await;
            }
        }
    });

    Ok(Json(DocumentInfo {
        id: row.id,
        filename: row.filename,
        status: row.status,
        chunk_count: row.chunk_count,
        created_at: row.created_at,
    }))
}
