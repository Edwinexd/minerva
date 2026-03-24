use axum::extract::{Extension, Multipart, Path, State};
use axum::routing::{delete, get};
use axum::{Json, Router};
use minerva_core::models::User;
use serde::Serialize;
use std::sync::Arc;
use uuid::Uuid;

use crate::error::AppError;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(list_documents).post(upload_document))
        .route("/{doc_id}", delete(delete_document))
}

#[derive(Serialize)]
struct DocumentResponse {
    id: Uuid,
    course_id: Uuid,
    filename: String,
    mime_type: String,
    size_bytes: i64,
    status: String,
    chunk_count: Option<i32>,
    error_msg: Option<String>,
    created_at: chrono::NaiveDateTime,
    processed_at: Option<chrono::NaiveDateTime>,
}

impl From<minerva_db::queries::documents::DocumentRow> for DocumentResponse {
    fn from(row: minerva_db::queries::documents::DocumentRow) -> Self {
        Self {
            id: row.id,
            course_id: row.course_id,
            filename: row.filename,
            mime_type: row.mime_type,
            size_bytes: row.size_bytes,
            status: row.status,
            chunk_count: row.chunk_count,
            error_msg: row.error_msg,
            created_at: row.created_at,
            processed_at: row.processed_at,
        }
    }
}

async fn list_documents(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(course_id): Path<Uuid>,
) -> Result<Json<Vec<DocumentResponse>>, AppError> {
    // Verify access
    let course = minerva_db::queries::courses::find_by_id(&state.db, course_id)
        .await?
        .ok_or(AppError::NotFound)?;

    if course.owner_id != user.id && !user.role.is_admin() {
        return Err(AppError::Forbidden);
    }

    let rows = minerva_db::queries::documents::list_by_course(&state.db, course_id).await?;
    Ok(Json(rows.into_iter().map(DocumentResponse::from).collect()))
}

async fn upload_document(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(course_id): Path<Uuid>,
    mut multipart: Multipart,
) -> Result<Json<DocumentResponse>, AppError> {
    let course = minerva_db::queries::courses::find_by_id(&state.db, course_id)
        .await?
        .ok_or(AppError::NotFound)?;

    if course.owner_id != user.id && !user.role.is_admin() {
        return Err(AppError::Forbidden);
    }

    // Read the file from multipart
    let field = multipart
        .next_field()
        .await
        .map_err(|e| AppError::BadRequest(format!("multipart error: {}", e)))?
        .ok_or_else(|| AppError::BadRequest("no file provided".to_string()))?;

    let filename = field
        .file_name()
        .unwrap_or("document.pdf")
        .to_string();
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
    let docs_path = &state.config.docs_path;
    let dir = format!("{}/{}", docs_path, course_id);
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| AppError::Internal(format!("failed to create directory: {}", e)))?;

    let file_path = format!("{}/{}.pdf", dir, doc_id);
    tokio::fs::write(&file_path, &data)
        .await
        .map_err(|e| AppError::Internal(format!("failed to write file: {}", e)))?;

    // Insert document record
    let row = minerva_db::queries::documents::insert(
        &state.db,
        doc_id,
        course_id,
        &filename,
        &content_type,
        size_bytes,
        user.id,
    )
    .await?;

    // Spawn background processing task
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
                    "document {} processed: {} chunks, {} embedding tokens",
                    doc_id,
                    result.chunk_count,
                    result.embedding_tokens,
                );
            }
            Err(e) => {
                tracing::error!("document {} processing failed: {}", doc_id, e);
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

    Ok(Json(DocumentResponse::from(row)))
}

async fn delete_document(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path((course_id, doc_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<serde_json::Value>, AppError> {
    let course = minerva_db::queries::courses::find_by_id(&state.db, course_id)
        .await?
        .ok_or(AppError::NotFound)?;

    if course.owner_id != user.id && !user.role.is_admin() {
        return Err(AppError::Forbidden);
    }

    // Delete from DB
    minerva_db::queries::documents::delete(&state.db, doc_id).await?;

    // Delete file from disk
    let file_path = format!("{}/{}/{}.pdf", state.config.docs_path, course_id, doc_id);
    let _ = tokio::fs::remove_file(&file_path).await;

    // TODO: Also delete vectors from Qdrant for this document

    Ok(Json(serde_json::json!({ "deleted": true })))
}
