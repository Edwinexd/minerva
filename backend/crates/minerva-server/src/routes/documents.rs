use axum::extract::{Extension, Multipart, Path, Query, State};
use axum::routing::{delete, get};
use axum::{Json, Router};
use minerva_core::models::User;
use qdrant_client::qdrant::ScrollPointsBuilder;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::AppError;
use crate::state::AppState;

/// Maximum upload size: 50 MB.
pub const MAX_UPLOAD_BYTES: i64 = 50 * 1_000_000;

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/",
            get(list_documents)
                .post(upload_document)
                .layer(axum::extract::DefaultBodyLimit::max(
                    MAX_UPLOAD_BYTES as usize,
                )),
        )
        .route("/{doc_id}", delete(delete_document).patch(patch_document))
        .route("/{doc_id}/chunks", get(list_chunks))
        .route("/search", get(search_chunks))
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
    displayable: bool,
    created_at: chrono::DateTime<chrono::Utc>,
    processed_at: Option<chrono::DateTime<chrono::Utc>>,
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
            displayable: row.displayable,
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

    let filename = field.file_name().unwrap_or("document").to_string();
    let content_type = field
        .content_type()
        .unwrap_or("application/octet-stream")
        .to_string();
    let data = field
        .bytes()
        .await
        .map_err(|e| AppError::BadRequest(format!("failed to read file: {}", e)))?;

    let size_bytes = data.len() as i64;
    if size_bytes > MAX_UPLOAD_BYTES {
        return Err(AppError::BadRequest(format!(
            "file too large: {} bytes (max {} MB)",
            size_bytes,
            MAX_UPLOAD_BYTES / 1_000_000
        )));
    }

    let doc_id = Uuid::new_v4();

    // Save file to disk
    let docs_path = &state.config.docs_path;
    let dir = format!("{}/{}", docs_path, course_id);
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| AppError::Internal(format!("failed to create directory: {}", e)))?;

    let ext = extension_from_filename(&filename);
    let file_path = format!("{}/{}.{}", dir, doc_id, ext);
    tokio::fs::write(&file_path, &data)
        .await
        .map_err(|e| AppError::Internal(format!("failed to write file: {}", e)))?;

    // Insert document record as 'pending'. The background worker will pick it
    // up and process it with bounded concurrency.
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

    Ok(Json(DocumentResponse::from(row)))
}

#[derive(Deserialize)]
struct PatchDocumentBody {
    displayable: Option<bool>,
}

async fn patch_document(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path((course_id, doc_id)): Path<(Uuid, Uuid)>,
    Json(body): Json<PatchDocumentBody>,
) -> Result<Json<serde_json::Value>, AppError> {
    let course = minerva_db::queries::courses::find_by_id(&state.db, course_id)
        .await?
        .ok_or(AppError::NotFound)?;

    if course.owner_id != user.id && !user.role.is_admin() {
        return Err(AppError::Forbidden);
    }

    if let Some(displayable) = body.displayable {
        minerva_db::queries::documents::update_displayable(&state.db, doc_id, displayable).await?;
    }

    Ok(Json(serde_json::json!({ "ok": true })))
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

    // Delete file from disk -- try common extensions since we don't store the ext in DB.
    for ext in &["pdf", "docx", "doc", "pptx", "ppt", "txt", "html", "url"] {
        let file_path = format!(
            "{}/{}/{}.{}",
            state.config.docs_path, course_id, doc_id, ext
        );
        if tokio::fs::remove_file(&file_path).await.is_ok() {
            break;
        }
    }

    // TODO: Also delete vectors from Qdrant for this document

    Ok(Json(serde_json::json!({ "deleted": true })))
}

#[derive(Serialize)]
struct ChunkResponse {
    chunk_index: i64,
    text: String,
    filename: String,
}

/// List all chunks for a specific document from Qdrant.
async fn list_chunks(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path((course_id, doc_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<Vec<ChunkResponse>>, AppError> {
    let course = minerva_db::queries::courses::find_by_id(&state.db, course_id)
        .await?
        .ok_or(AppError::NotFound)?;

    if course.owner_id != user.id && !user.role.is_admin() {
        return Err(AppError::Forbidden);
    }

    let collection_name = format!("course_{}", course_id);

    // Check if collection exists
    let exists = state
        .qdrant
        .collection_exists(&collection_name)
        .await
        .unwrap_or(false);
    if !exists {
        return Ok(Json(Vec::new()));
    }

    // Scroll through all points with this document_id
    let filter = qdrant_client::qdrant::Filter::must([qdrant_client::qdrant::Condition::matches(
        "document_id",
        doc_id.to_string(),
    )]);

    let result = state
        .qdrant
        .scroll(
            ScrollPointsBuilder::new(&collection_name)
                .filter(filter)
                .with_payload(true)
                .limit(1000),
        )
        .await
        .map_err(|e| AppError::Internal(format!("qdrant scroll failed: {}", e)))?;

    let mut chunks: Vec<ChunkResponse> = result
        .result
        .iter()
        .filter_map(|point| {
            use crate::strategy::common::{payload_int, payload_string};

            let text = payload_string(&point.payload, "text")?;
            Some(ChunkResponse {
                chunk_index: payload_int(&point.payload, "chunk_index").unwrap_or(0),
                text,
                filename: payload_string(&point.payload, "filename").unwrap_or_default(),
            })
        })
        .collect();

    chunks.sort_by_key(|c| c.chunk_index);
    Ok(Json(chunks))
}

#[derive(Deserialize)]
struct SearchQuery {
    q: String,
    limit: Option<u64>,
}

#[derive(Serialize)]
struct SearchResult {
    score: f32,
    text: String,
    filename: String,
    document_id: String,
    chunk_index: i64,
}

/// Search chunks by semantic similarity. Teachers can test RAG queries.
async fn search_chunks(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(course_id): Path<Uuid>,
    Query(params): Query<SearchQuery>,
) -> Result<Json<Vec<SearchResult>>, AppError> {
    let course = minerva_db::queries::courses::find_by_id(&state.db, course_id)
        .await?
        .ok_or(AppError::NotFound)?;

    if course.owner_id != user.id && !user.role.is_admin() {
        return Err(AppError::Forbidden);
    }

    let collection_name = format!("course_{}", course_id);
    let exists = state
        .qdrant
        .collection_exists(&collection_name)
        .await
        .unwrap_or(false);
    if !exists {
        return Ok(Json(Vec::new()));
    }

    let limit = params.limit.unwrap_or(10);
    let client = reqwest::Client::new();

    let scored_points = crate::strategy::common::embedding_search(
        &client,
        &state.config.openai_api_key,
        &state.fastembed,
        &state.qdrant,
        &collection_name,
        &params.q,
        limit,
        None,
        &course.embedding_provider,
        &course.embedding_model,
    )
    .await
    .map_err(AppError::Internal)?;

    let results: Vec<SearchResult> = scored_points
        .iter()
        .filter_map(|point| {
            use crate::strategy::common::{payload_int, payload_string};

            let text = payload_string(&point.payload, "text")?;
            Some(SearchResult {
                score: point.score,
                text,
                filename: payload_string(&point.payload, "filename").unwrap_or_default(),
                document_id: payload_string(&point.payload, "document_id").unwrap_or_default(),
                chunk_index: payload_int(&point.payload, "chunk_index").unwrap_or(0),
            })
        })
        .collect();

    Ok(Json(results))
}

/// Extract file extension from a filename, defaulting to "bin".
pub fn extension_from_filename(filename: &str) -> &str {
    std::path::Path::new(filename)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("bin")
}
