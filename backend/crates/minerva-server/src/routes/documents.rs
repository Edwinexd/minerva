use axum::extract::{Extension, Multipart, Path, Query, State};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use futures::StreamExt;
use minerva_core::models::User;
use qdrant_client::qdrant::{DeletePointsBuilder, ScrollPointsBuilder};
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;
use uuid::Uuid;

use crate::error::AppError;
use crate::state::AppState;

/// Maximum upload size for a single document: 50 MB.
pub const MAX_UPLOAD_BYTES: i64 = 50 * 1_000_000;

/// Maximum upload size for a Moodle .mbz backup: 1 GB. Whole-course backups
/// routinely clear 50 MB once video/slide decks are attached, so the regular
/// per-file cap is not a useful ceiling here.
pub const MAX_MBZ_UPLOAD_BYTES: i64 = 1_000_000_000;

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
        .route(
            "/mbz",
            post(upload_mbz).layer(axum::extract::DefaultBodyLimit::max(
                MAX_MBZ_UPLOAD_BYTES as usize,
            )),
        )
        .route("/{doc_id}", delete(delete_document).patch(patch_document))
        .route("/{doc_id}/chunks", get(list_chunks))
        // Course-knowledge-graph endpoints. Teacher-only (course
        // owner / admin / course teacher); auth is enforced inside each
        // handler with the same pattern as `patch_document`.
        .route("/{doc_id}/reclassify", post(reclassify_document))
        .route("/{doc_id}/kind", axum::routing::patch(set_document_kind))
        .route("/{doc_id}/kind/lock", delete(clear_kind_lock))
        .route("/reclassify-all", post(reclassify_all_in_course))
        .route("/knowledge-graph", get(get_knowledge_graph))
        .route("/knowledge-graph/rebuild", post(rebuild_knowledge_graph))
        .route(
            "/knowledge-graph/edges/{edge_id}/reject",
            post(reject_edge).delete(unreject_edge),
        )
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
    /// Knowledge-graph classification. `None` until the classifier has
    /// run for this doc; the chat-time RAG filter holds unclassified
    /// docs out of context (see `partition_chunks`).
    kind: Option<String>,
    kind_confidence: Option<f32>,
    kind_rationale: Option<String>,
    kind_locked_by_teacher: bool,
    classified_at: Option<chrono::DateTime<chrono::Utc>>,
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
            kind: row.kind,
            kind_confidence: row.kind_confidence,
            kind_rationale: row.kind_rationale,
            kind_locked_by_teacher: row.kind_locked_by_teacher,
            classified_at: row.classified_at,
        }
    }
}

async fn list_documents(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(course_id): Path<Uuid>,
) -> Result<Json<Vec<DocumentResponse>>, AppError> {
    // Verify access; owner, admin, teacher, and TA can read the document list.
    let course = minerva_db::queries::courses::find_by_id(&state.db, course_id)
        .await?
        .ok_or(AppError::NotFound)?;

    if course.owner_id != user.id
        && !user.role.is_admin()
        && !minerva_db::queries::courses::is_course_teacher(&state.db, course_id, user.id).await?
    {
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
    if size_bytes > MAX_UPLOAD_BYTES {
        return Err(AppError::bad_request_with(
            "doc.file_too_large",
            [
                ("size_bytes", size_bytes.to_string()),
                ("max_mb", (MAX_UPLOAD_BYTES / 1_000_000).to_string()),
            ],
        ));
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
        None,
    )
    .await?;

    Ok(Json(DocumentResponse::from(row)))
}

#[derive(Serialize)]
struct MbzImportResponse {
    imported: usize,
    skipped_hidden: usize,
}

/// Accept a Moodle course backup (.mbz) and ingest every piece of visible
/// course material as an individual document. Mirrors what the
/// `local_minerva` Moodle plugin would upload over its sync API, but for
/// teachers whose Moodle has no plugin installed.
async fn upload_mbz(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(course_id): Path<Uuid>,
    mut multipart: Multipart,
) -> Result<Json<MbzImportResponse>, AppError> {
    let course = minerva_db::queries::courses::find_by_id(&state.db, course_id)
        .await?
        .ok_or(AppError::NotFound)?;

    if course.owner_id != user.id && !user.role.is_admin() {
        return Err(AppError::Forbidden);
    }

    let field = multipart
        .next_field()
        .await
        .map_err(|e| {
            AppError::bad_request_with("doc.multipart_error", [("detail", e.to_string())])
        })?
        .ok_or_else(|| AppError::bad_request("doc.no_file"))?;

    // Stream the upload straight to disk. Pulling 1 GB into memory via
    // Field::bytes() would crush the pod's RAM; chunked copy keeps usage
    // bounded by the chunk size hyper picked.
    let upload_tmp = tempfile::Builder::new()
        .prefix("minerva-mbz-upload-")
        .suffix(".mbz")
        .tempfile()
        .map_err(|e| AppError::Internal(format!("mbz tempfile alloc failed: {e}")))?;
    let upload_path = upload_tmp.path().to_path_buf();

    let mut out = tokio::fs::File::create(&upload_path)
        .await
        .map_err(|e| AppError::Internal(format!("mbz tempfile open failed: {e}")))?;
    let mut total: i64 = 0;
    let mut stream = field;
    while let Some(chunk) = stream.next().await {
        let bytes = chunk.map_err(|e| {
            AppError::bad_request_with("doc.read_failed", [("detail", e.to_string())])
        })?;
        total += bytes.len() as i64;
        if total > MAX_MBZ_UPLOAD_BYTES {
            return Err(AppError::bad_request_with(
                "doc.file_too_large",
                [
                    ("size_bytes", total.to_string()),
                    ("max_mb", (MAX_MBZ_UPLOAD_BYTES / 1_000_000).to_string()),
                ],
            ));
        }
        out.write_all(&bytes)
            .await
            .map_err(|e| AppError::Internal(format!("mbz tempfile write failed: {e}")))?;
    }
    out.flush()
        .await
        .map_err(|e| AppError::Internal(format!("mbz tempfile flush failed: {e}")))?;
    drop(out);

    // Parse off the blocking thread pool: archive extraction is CPU+fs bound
    // and would otherwise stall the async reactor.
    let parse_path = upload_path.clone();
    let import =
        tokio::task::spawn_blocking(move || minerva_ingest::moodle::import_mbz(&parse_path))
            .await
            .map_err(|e| AppError::Internal(format!("mbz parse task panicked: {e}")))?
            .map_err(|e| {
                AppError::bad_request_with("doc.mbz_parse_failed", [("detail", e.to_string())])
            })?;

    let docs_path = &state.config.docs_path;
    let dir = format!("{}/{}", docs_path, course_id);
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| AppError::Internal(format!("failed to create directory: {}", e)))?;

    let mut imported: usize = 0;
    for item in &import.items {
        let doc_id = Uuid::new_v4();
        let ext = extension_from_filename(&item.filename);
        let file_path = format!("{}/{}.{}", dir, doc_id, ext);

        let size_bytes: i64 = match &item.body {
            minerva_ingest::moodle::ItemBody::Inline(bytes) => {
                tokio::fs::write(&file_path, bytes).await.map_err(|e| {
                    AppError::Internal(format!("failed to write {}: {}", item.filename, e))
                })?;
                bytes.len() as i64
            }
            minerva_ingest::moodle::ItemBody::File(src) => {
                tokio::fs::copy(src, &file_path).await.map_err(|e| {
                    AppError::Internal(format!("failed to copy {}: {}", item.filename, e))
                })? as i64
            }
        };

        minerva_db::queries::documents::insert(
            &state.db,
            doc_id,
            course_id,
            &item.filename,
            &item.mime,
            size_bytes,
            user.id,
            None,
        )
        .await?;
        imported += 1;
    }

    // upload_tmp drops here, removing the source .mbz. The parser's own
    // extraction tempdir is owned by `import` and cleaned up when it drops
    // at function return, which is fine because every File item has already
    // been copied above.
    drop(upload_tmp);

    Ok(Json(MbzImportResponse {
        imported,
        skipped_hidden: import.skipped_hidden,
    }))
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

    // Scope doc_id to this course: the DB helper filters by id only, so
    // without this check a course owner could modify documents in other
    // courses by putting a foreign doc_id in the path.
    let doc = minerva_db::queries::documents::find_by_id(&state.db, doc_id)
        .await?
        .ok_or(AppError::NotFound)?;
    if doc.course_id != course_id {
        return Err(AppError::NotFound);
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

    // Scope doc_id to this course: the DB delete filters by id only, so
    // without this check a course owner could delete documents in other
    // courses by putting a foreign doc_id in the path.
    let doc = minerva_db::queries::documents::find_by_id(&state.db, doc_id)
        .await?
        .ok_or(AppError::NotFound)?;
    if doc.course_id != course_id {
        return Err(AppError::NotFound);
    }

    // Delete vectors from Qdrant first; if this fails we can retry safely
    // without leaving orphaned vectors behind.
    let collection_name =
        minerva_ingest::pipeline::collection_name(course_id, course.embedding_version);
    let collection_exists = state
        .qdrant
        .collection_exists(&collection_name)
        .await
        .unwrap_or(false);
    if collection_exists {
        let filter =
            qdrant_client::qdrant::Filter::must([qdrant_client::qdrant::Condition::matches(
                "document_id",
                doc_id.to_string(),
            )]);
        state
            .qdrant
            .delete_points(
                DeletePointsBuilder::new(&collection_name)
                    .points(filter)
                    .wait(true),
            )
            .await
            .map_err(|e| AppError::Internal(format!("qdrant delete failed: {}", e)))?;
    }

    // Delete from DB
    minerva_db::queries::documents::delete(&state.db, doc_id).await?;

    // Delete file from disk; try common extensions since we don't store the ext in DB.
    for ext in &["pdf", "docx", "doc", "pptx", "ppt", "txt", "html", "url"] {
        let file_path = format!(
            "{}/{}/{}.{}",
            state.config.docs_path, course_id, doc_id, ext
        );
        if tokio::fs::remove_file(&file_path).await.is_ok() {
            break;
        }
    }

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

    if course.owner_id != user.id
        && !user.role.is_admin()
        && !minerva_db::queries::courses::is_course_teacher(&state.db, course_id, user.id).await?
    {
        return Err(AppError::Forbidden);
    }

    let collection_name =
        minerva_ingest::pipeline::collection_name(course_id, course.embedding_version);

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

/// Search chunks by semantic similarity. Teachers and TAs can test RAG queries.
async fn search_chunks(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(course_id): Path<Uuid>,
    Query(params): Query<SearchQuery>,
) -> Result<Json<Vec<SearchResult>>, AppError> {
    let course = minerva_db::queries::courses::find_by_id(&state.db, course_id)
        .await?
        .ok_or(AppError::NotFound)?;

    if course.owner_id != user.id
        && !user.role.is_admin()
        && !minerva_db::queries::courses::is_course_teacher(&state.db, course_id, user.id).await?
    {
        return Err(AppError::Forbidden);
    }

    let collection_name =
        minerva_ingest::pipeline::collection_name(course_id, course.embedding_version);
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

// ── Course-knowledge-graph V1 endpoints ────────────────────────────
//
// Auth: same pattern as `patch_document`; course owner OR admin OR a
// teacher of the course. We don't allow students or TAs to flip a
// document's classification.

/// Shared auth check: caller is course owner, admin, or course teacher.
async fn require_course_teacher(
    state: &AppState,
    course_id: Uuid,
    user: &User,
) -> Result<(), AppError> {
    let course = minerva_db::queries::courses::find_by_id(&state.db, course_id)
        .await?
        .ok_or(AppError::NotFound)?;
    if course.owner_id == user.id || user.role.is_admin() {
        return Ok(());
    }
    if minerva_db::queries::courses::is_course_teacher(&state.db, course_id, user.id).await? {
        return Ok(());
    }
    Err(AppError::Forbidden)
}

/// Gate every KG-related endpoint on the `course_kg` feature flag.
/// Returns 404 (not 403) when off so a non-KG course "looks like"
/// the feature simply doesn't exist; no surface for student or
/// teacher fishing.
async fn require_kg_enabled(state: &AppState, course_id: Uuid) -> Result<(), AppError> {
    if crate::feature_flags::course_kg_enabled(&state.db, course_id).await {
        Ok(())
    } else {
        Err(AppError::NotFound)
    }
}

/// Resolve a `(course_id, doc_id)` pair, ensuring the document actually
/// belongs to the course. Same scope-check as `patch_document`.
async fn load_doc_in_course(
    state: &AppState,
    course_id: Uuid,
    doc_id: Uuid,
) -> Result<minerva_db::queries::documents::DocumentRow, AppError> {
    let doc = minerva_db::queries::documents::find_by_id(&state.db, doc_id)
        .await?
        .ok_or(AppError::NotFound)?;
    if doc.course_id != course_id {
        return Err(AppError::NotFound);
    }
    Ok(doc)
}

/// Run the classifier on a single document and persist the result.
/// Returns the new (kind, confidence, rationale) tuple, or `None` if
/// the document was locked by a teacher (in which case we leave it
/// alone and tell the caller).
///
/// Crate-public so the admin backfill endpoint can fan out across
/// every unclassified doc using the same code path.
pub(crate) async fn run_classify_one(
    state: &AppState,
    doc: &minerva_db::queries::documents::DocumentRow,
) -> Result<Option<(String, f32, Option<String>)>, AppError> {
    if doc.kind_locked_by_teacher {
        return Ok(None);
    }

    let ext = extension_from_filename(&doc.filename);
    let file_path = format!(
        "{}/{}/{}.{}",
        state.config.docs_path, doc.course_id, doc.id, ext
    );
    let path = std::path::Path::new(&file_path);
    let text = minerva_ingest::pipeline::extract_document_text(path)
        .map_err(|e| AppError::Internal(format!("text extraction failed: {}", e)))?;

    let classifier = crate::classification::CerebrasClassifier::new(
        reqwest::Client::new(),
        state.config.cerebras_api_key.clone(),
        state.db.clone(),
    );
    use minerva_ingest::classifier::Classifier;
    let result = classifier
        .classify(doc.course_id, &doc.filename, &doc.mime_type, &text)
        .await
        .map_err(AppError::Internal)?;

    let _ = minerva_db::queries::documents::set_classification(
        &state.db,
        doc.id,
        &result.kind,
        result.confidence,
        result.rationale.as_deref(),
    )
    .await?;

    Ok(Some((result.kind, result.confidence, result.rationale)))
}

#[derive(Serialize)]
struct ReclassifyResponse {
    classified: bool,
    locked: bool,
    kind: Option<String>,
    confidence: Option<f32>,
    rationale: Option<String>,
}

/// Re-run the classifier for a single document. No-op if the doc is
/// locked by a teacher (returns `locked: true` so the UI can surface
/// "unlock first").
///
/// Marks the course dirty for the relink sweeper; a single doc's
/// kind change can shift its `solution_of` / `part_of_unit` edges, so
/// the graph needs refreshing. Debounced (default 60s) so a teacher
/// rapid-fire reclassifying several docs only triggers one linker call.
async fn reclassify_document(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path((course_id, doc_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<ReclassifyResponse>, AppError> {
    require_course_teacher(&state, course_id, &user).await?;
    require_kg_enabled(&state, course_id).await?;
    let doc = load_doc_in_course(&state, course_id, doc_id).await?;

    match run_classify_one(&state, &doc).await? {
        None => Ok(Json(ReclassifyResponse {
            classified: false,
            locked: true,
            kind: doc.kind,
            confidence: doc.kind_confidence,
            rationale: doc.kind_rationale,
        })),
        Some((kind, confidence, rationale)) => {
            state.relink_scheduler.mark_dirty(course_id).await;
            Ok(Json(ReclassifyResponse {
                classified: true,
                locked: false,
                kind: Some(kind),
                confidence: Some(confidence),
                rationale,
            }))
        }
    }
}

#[derive(Deserialize)]
struct SetKindBody {
    kind: String,
}

/// Manually set a document's kind and lock it against future
/// auto-classification. If the new kind is `sample_solution`, also
/// purge any embedded chunks from Qdrant; otherwise stale vectors
/// would still be retrievable even though the doc is now flagged.
async fn set_document_kind(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path((course_id, doc_id)): Path<(Uuid, Uuid)>,
    Json(body): Json<SetKindBody>,
) -> Result<Json<serde_json::Value>, AppError> {
    require_course_teacher(&state, course_id, &user).await?;
    require_kg_enabled(&state, course_id).await?;
    let doc = load_doc_in_course(&state, course_id, doc_id).await?;

    // Reject unknown kinds at the API boundary so the user gets a 400
    // instead of a 500 from the DB CHECK constraint.
    if crate::classification::types::DocumentKind::from_str(&body.kind).is_none() {
        return Err(AppError::bad_request_with(
            "doc.kind_invalid",
            [("kind", body.kind.clone())],
        ));
    }

    minerva_db::queries::documents::set_kind_locked(&state.db, doc_id, &body.kind).await?;

    // If the teacher just declared this doc a sample_solution, purge
    // any Qdrant chunks so retrieval can't surface them. Idempotent --
    // if the collection or doc has no points, this is a no-op.
    if body.kind == "sample_solution" && doc.chunk_count.unwrap_or(0) > 0 {
        // Look up the course's current embedding_version so we hit
        // the live collection rather than a previous-rotation
        // orphan. One quick round-trip; this path is only taken on a
        // teacher's manual lock action so it's not hot.
        let collection_name =
            minerva_ingest::pipeline::collection_name_for_course(&state.db, course_id)
                .await
                .map_err(|e| AppError::Internal(format!("course lookup failed: {}", e)))?;
        if state
            .qdrant
            .collection_exists(&collection_name)
            .await
            .unwrap_or(false)
        {
            let filter =
                qdrant_client::qdrant::Filter::must([qdrant_client::qdrant::Condition::matches(
                    "document_id",
                    doc_id.to_string(),
                )]);
            if let Err(e) = state
                .qdrant
                .delete_points(
                    DeletePointsBuilder::new(&collection_name)
                        .points(filter)
                        .wait(true),
                )
                .await
            {
                tracing::error!(
                    "set_document_kind: qdrant purge failed for doc {} after sample_solution lock: {}",
                    doc_id,
                    e,
                );
                // Non-fatal: the kind is already locked in the DB so
                // partition_chunks will drop these chunks defensively
                // even if Qdrant still has them.
            }
        }
    }

    // A teacher-driven kind change can flip whether a doc participates
    // in `solution_of` / `part_of_unit` edges (e.g. flipping reading ->
    // sample_solution removes its embeddings AND should remove edges
    // pointing at it). Mark the course dirty for the relink sweeper.
    state.relink_scheduler.mark_dirty(course_id).await;

    Ok(Json(serde_json::json!({
        "kind": body.kind,
        "kind_locked_by_teacher": true,
    })))
}

/// Clear the teacher lock so future re-classifications can overwrite
/// the kind. Doesn't trigger a re-run; the teacher can press
/// re-classify after if they want.
async fn clear_kind_lock(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path((course_id, doc_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<serde_json::Value>, AppError> {
    require_course_teacher(&state, course_id, &user).await?;
    require_kg_enabled(&state, course_id).await?;
    let _doc = load_doc_in_course(&state, course_id, doc_id).await?;
    minerva_db::queries::documents::clear_kind_lock(&state.db, doc_id).await?;
    Ok(Json(serde_json::json!({
        "kind_locked_by_teacher": false,
    })))
}

#[derive(Serialize)]
struct ReclassifyAllResponse {
    queued: usize,
}

/// Fan out re-classification across every non-locked document in a
/// course. Runs in a spawned task so the request returns immediately;
/// progress is observable by re-fetching the document list (rows show
/// updated `kind_confidence` / `classified_at` as they finish).
async fn reclassify_all_in_course(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(course_id): Path<Uuid>,
) -> Result<Json<ReclassifyAllResponse>, AppError> {
    require_course_teacher(&state, course_id, &user).await?;
    require_kg_enabled(&state, course_id).await?;

    let docs = minerva_db::queries::documents::list_by_course(&state.db, course_id).await?;
    let candidates: Vec<_> = docs
        .into_iter()
        .filter(|d| !d.kind_locked_by_teacher && d.status == "ready")
        .collect();
    let queued = candidates.len();

    let state_clone = state.clone();
    tokio::spawn(async move {
        for doc in candidates {
            if let Err(e) = run_classify_one(&state_clone, &doc).await {
                tracing::warn!(
                    "reclassify-all: doc {} ({}) failed: {:?}",
                    doc.id,
                    doc.filename,
                    e,
                );
            }
        }
        tracing::info!(
            "reclassify-all: finished course {} ({} docs)",
            course_id,
            queued
        );
    });

    // Hand off to the relink sweeper instead of doing it inline. The
    // per-doc classify loop above already calls `run_classify_one` for
    // each candidate; we additionally mark the course immediate-dirty
    // so the sweeper picks it up on its next tick (typically ~10s,
    // well after the classify task is done).
    state.relink_scheduler.mark_dirty_immediate(course_id).await;

    Ok(Json(ReclassifyAllResponse { queued }))
}

// ── Knowledge graph: cross-doc linking + view ─────────────────────

/// Run the cross-doc linker for a single course and replace its
/// stored edges with the result. Idempotent: each call wipes the
/// existing edges and writes fresh ones, so a no-op course (no
/// confident edges) ends up with an empty graph rather than a stale
/// one.
///
/// Crate-public so the admin backfill task can relink each course it
/// touched after the per-doc classification finishes.
pub(crate) async fn relink_course(state: &AppState, course_id: Uuid) -> Result<usize, AppError> {
    let docs = minerva_db::queries::documents::list_by_course(&state.db, course_id).await?;

    let http = reqwest::Client::new();
    let ctx = crate::classification::linker::LinkContext {
        http: &http,
        api_key: &state.config.cerebras_api_key,
        db: &state.db,
        qdrant: &state.qdrant,
    };
    // The linker now writes edges + decision cache itself per fresh
    // pair, so we don't wipe-and-rewrite here. A pair whose endpoints
    // haven't been re-classified since its prior decision keeps both
    // its cache row AND its existing document_relations edge (if any)
    //; no LLM call, no DB churn.
    let result = crate::classification::linker::link_course(&ctx, course_id, &docs)
        .await
        .map_err(AppError::Internal)?;

    tracing::info!(
        "relink: course {} considered {} doc(s); {} cached, {} re-evaluated, {} live edge(s)",
        course_id,
        result.considered,
        result.cached_hits,
        result.re_evaluated,
        result.edges_written,
    );

    Ok(result.edges_written)
}

#[derive(Serialize)]
struct GraphNode {
    id: Uuid,
    filename: String,
    kind: Option<String>,
    kind_confidence: Option<f32>,
    kind_locked_by_teacher: bool,
    chunk_count: Option<i32>,
}

#[derive(Serialize)]
struct GraphEdge {
    /// Stable id, used as the addressable handle for per-edge reject /
    /// unreject. Returned even for rejected edges so the UI can show them
    /// in a "vetoed" filter.
    id: Uuid,
    src_id: Uuid,
    dst_id: Uuid,
    relation: String,
    confidence: f32,
    rationale: Option<String>,
    /// True when a teacher has explicitly rejected this edge. Rejected
    /// edges are filtered out of the default graph render (the linker
    /// won't even re-propose them on the next pass) but exposed in the
    /// API payload so the UI can show a "show rejected" toggle.
    rejected_by_teacher: bool,
}

#[derive(Serialize)]
struct GraphResponse {
    nodes: Vec<GraphNode>,
    edges: Vec<GraphEdge>,
    /// Whether at least one edge has been computed for this course.
    /// The UI uses this to show "Build the graph" call-to-action vs
    /// rendering the viewer.
    edges_computed: bool,
    /// Re-link status for the UI's "Linking..." pill. `true` iff the
    /// course is currently queued for a relink sweep OR there are
    /// cached pair decisions whose endpoints have moved past their
    /// snapshot timestamps (i.e. the next sweep has real work to do).
    ///
    /// Deliberately a bool, not a count: an honest count of "pairs
    /// the linker is about to re-evaluate" requires running the
    /// embedding-similarity candidate generator, which is precisely
    /// what the linker does on the next tick. The previous count
    /// summed `stale_decisions` (pairs) and `new_doc_count` (docs
    /// that had never been on either side of a cached pair); the
    /// latter went permanently positive for any classified doc whose
    /// nearest neighbour was below `MIN_EMBEDDING_SIMILARITY`, so the
    /// counter never cleared. Both bugs disappear once we stop
    /// trying to invent a number.
    linker_pending: bool,
}

/// Knowledge-graph view for a single course: every doc as a node
/// (typed by `kind`), every linker-asserted edge between them.
async fn get_knowledge_graph(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(course_id): Path<Uuid>,
) -> Result<Json<GraphResponse>, AppError> {
    require_course_teacher(&state, course_id, &user).await?;
    require_kg_enabled(&state, course_id).await?;

    let docs = minerva_db::queries::documents::list_by_course(&state.db, course_id).await?;
    let edges_rows =
        minerva_db::queries::document_relations::list_by_course(&state.db, course_id).await?;

    let nodes: Vec<GraphNode> = docs
        .into_iter()
        .map(|d| GraphNode {
            id: d.id,
            filename: d.filename,
            kind: d.kind,
            kind_confidence: d.kind_confidence,
            kind_locked_by_teacher: d.kind_locked_by_teacher,
            chunk_count: d.chunk_count,
        })
        .collect();

    let edges_computed = !edges_rows.is_empty();
    let edges: Vec<GraphEdge> = edges_rows
        .into_iter()
        .map(|e| GraphEdge {
            id: e.id,
            src_id: e.src_doc_id,
            dst_id: e.dst_doc_id,
            relation: e.relation,
            confidence: e.confidence,
            rationale: e.rationale,
            rejected_by_teacher: e.rejected_by_teacher,
        })
        .collect();

    // "Linking pending" indicator. True iff EITHER:
    //   * the course is in `relink_queue` (a mark_dirty has fired
    //     since the last sweep drain; the sweep is about to run),
    //   * OR there are cached pair decisions whose endpoint
    //     `classified_at` no longer matches (work the next sweep
    //     will redo).
    // Both signals come from authoritative tables. Two cheap
    // queries; no docs join required.
    let queued = minerva_db::queries::relink_queue::is_queued(&state.db, course_id)
        .await
        .unwrap_or(false);
    let stale_pairs =
        minerva_db::queries::linker_decisions::stale_decisions_for_course(&state.db, course_id)
            .await
            .unwrap_or(0);
    let linker_pending = queued || stale_pairs > 0;

    Ok(Json(GraphResponse {
        nodes,
        edges,
        edges_computed,
        linker_pending,
    }))
}

#[derive(Serialize)]
struct EdgeMutationResponse {
    /// Echo of the edge's id so the UI can confirm the mutation
    /// landed against the right row. (Matches the path parameter.)
    id: Uuid,
    rejected_by_teacher: bool,
}

/// Reject an edge: the linker won't re-propose this pair, and the
/// graph-view filter hides it from the default render. Idempotent --
/// rejecting an already-rejected edge just refreshes `rejected_at`.
async fn reject_edge(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path((course_id, edge_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<EdgeMutationResponse>, AppError> {
    require_course_teacher(&state, course_id, &user).await?;
    require_kg_enabled(&state, course_id).await?;

    // Cross-course safety: if the edge id resolves to a different
    // course, surface 404 rather than silently allowing a teacher to
    // mutate edges in courses they don't own.
    let edge = minerva_db::queries::document_relations::find_by_id(&state.db, edge_id)
        .await?
        .ok_or(AppError::NotFound)?;
    if edge.course_id != course_id {
        return Err(AppError::NotFound);
    }

    let updated =
        minerva_db::queries::document_relations::reject_edge(&state.db, edge_id, user.id).await?;
    if !updated {
        return Err(AppError::NotFound);
    }

    Ok(Json(EdgeMutationResponse {
        id: edge_id,
        rejected_by_teacher: true,
    }))
}

/// Undo a rejection. The pair becomes eligible for the next linker
/// pass to re-emit if the model still likes it.
async fn unreject_edge(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path((course_id, edge_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<EdgeMutationResponse>, AppError> {
    require_course_teacher(&state, course_id, &user).await?;
    require_kg_enabled(&state, course_id).await?;

    let edge = minerva_db::queries::document_relations::find_by_id(&state.db, edge_id)
        .await?
        .ok_or(AppError::NotFound)?;
    if edge.course_id != course_id {
        return Err(AppError::NotFound);
    }

    let updated =
        minerva_db::queries::document_relations::unreject_edge(&state.db, edge_id).await?;
    if !updated {
        return Err(AppError::NotFound);
    }

    Ok(Json(EdgeMutationResponse {
        id: edge_id,
        rejected_by_teacher: false,
    }))
}

#[derive(Serialize)]
struct RelinkResponse {
    edges: usize,
}

/// Manually trigger a re-link of the course's knowledge graph. Useful
/// after a teacher edits kinds and wants the edges refreshed without
/// firing a full re-classify.
async fn rebuild_knowledge_graph(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(course_id): Path<Uuid>,
) -> Result<Json<RelinkResponse>, AppError> {
    require_course_teacher(&state, course_id, &user).await?;
    require_kg_enabled(&state, course_id).await?;
    let edges = relink_course(&state, course_id).await?;
    Ok(Json(RelinkResponse { edges }))
}
