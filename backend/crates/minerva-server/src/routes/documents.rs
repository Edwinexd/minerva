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

/// Hard ceiling on `axum::DefaultBodyLimit::max(...)` for single-doc
/// uploads. Set at router build time so changes require a restart;
/// the *configured* per-upload cap (admin-tunable, read live in the
/// handler via `system_defaults::max_upload_bytes`) lives at or
/// below this. Raising the ceiling lets the admin dial higher; the
/// admin can always dial *lower* without touching this constant.
///
/// Kept in sync with
/// `crate::system_defaults::BODY_LIMIT_CEILING`.
pub const UPLOAD_BODY_LIMIT_CEILING: i64 = crate::system_defaults::BODY_LIMIT_CEILING;

/// Same idea as `UPLOAD_BODY_LIMIT_CEILING`, but for the `.mbz`
/// Moodle-backup route which carries whole-course bundles and so
/// has a much higher ceiling.
pub const MBZ_BODY_LIMIT_CEILING: i64 = crate::system_defaults::MBZ_BODY_LIMIT_CEILING;

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/",
            get(list_documents)
                .post(upload_document)
                .layer(axum::extract::DefaultBodyLimit::max(
                    UPLOAD_BODY_LIMIT_CEILING as usize,
                )),
        )
        .route(
            "/mbz",
            post(upload_mbz).layer(axum::extract::DefaultBodyLimit::max(
                MBZ_BODY_LIMIT_CEILING as usize,
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
    /// Originating system: `"moodle"` / `"canvas"` for plugin uploads,
    /// `"manual"` for teacher-tagged UI uploads, `null` for untagged
    /// UI uploads. Shown in the docs UI so teachers can tell at a glance
    /// which docs are auto-managed.
    source_system: Option<String>,
    /// Opaque per-source identity. Teachers can edit this via PATCH on
    /// `"manual"`-system docs to group versions or repurpose a slot
    /// (re-uploading a new file with the same `source_ref` orphans the
    /// old one).
    source_ref: Option<String>,
    /// Soft-orphan timestamp. Always `null` for active docs; populated
    /// when the doc has been superseded or its upstream source was
    /// deleted. Orphaned docs are excluded from new retrievals but
    /// kept so chat-history citations resolve.
    orphaned_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Parent URL doc id for materialized children. `null` for first-class
    /// docs (teacher uploads, Moodle syncs, URL stubs themselves), set
    /// for the PDF / transcript an ingest worker produced from a URL
    /// stub. The frontend uses this to surface "this PDF came from
    /// {parent URL}" in the docs list.
    parent_document_id: Option<Uuid>,
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
            source_system: row.source_system,
            source_ref: row.source_ref,
            orphaned_at: row.orphaned_at,
            parent_document_id: row.parent_document_id,
        }
    }
}

// `compute_content_hash` moved to `minerva_pipeline` (shared by the api
// upload routes, the worker, and the axum-free Canvas sync engine in
// `minerva-app-core`). Re-exported so the many `compute_content_hash` /
// `super::documents::compute_content_hash` call sites keep resolving.
pub use minerva_pipeline::pipeline::compute_content_hash;

/// Streaming SHA-256 (hex) of a file on disk. Reuses a fixed 64 KiB
/// buffer across chunks so peak memory is constant regardless of file
/// size, instead of slurping a multi-MB doc into a `Vec<u8>` like
/// `compute_content_hash(&fs::read(path)?)` would. Used by the startup
/// `content_hash` backfill, which sweeps every legacy doc on disk and
/// would otherwise leave the glibc heap badly fragmented (cyclic
/// large-Vec churn while fastembed is concurrently filling its model
/// cache - the 6 GiB pod limit is sized for the fastembed cache plus
/// steady-state working set, not for a parallel multi-MB allocation
/// stream).
pub async fn compute_content_hash_streaming(path: &std::path::Path) -> std::io::Result<String> {
    use sha2::{Digest, Sha256};
    use tokio::io::AsyncReadExt;
    let mut file = tokio::fs::File::open(path).await?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}

/// Idempotent upload helper used by every route that ingests a document.
///
/// Path: compute the bytes' sha256 -> if `(course_id, content_hash)` matches
/// an active row, return it without touching disk; else write the file under
/// `{docs_path}/{course_id}/{new_doc_id}.{ext}` and insert. The on-disk file
/// for a dedup hit stays where it already was under the existing doc's id;
/// callers do not need to clean up.
///
/// `source_system` / `source_ref` are wired through for slice 2 (Moodle source
/// identity); slice 1 callers pass `None`. `source_url` is the legacy
/// per-URL idempotency key already used by the URL-stub flow; we still pass
/// it through unchanged.
#[allow(clippy::too_many_arguments)]
pub async fn upload_or_dedup(
    state: &AppState,
    course_id: Uuid,
    filename: &str,
    mime_type: &str,
    bytes: &[u8],
    uploaded_by: Uuid,
    source_url: Option<&str>,
    source_system: Option<&str>,
    source_ref: Option<&str>,
) -> Result<minerva_db::queries::documents::DocumentRow, AppError> {
    let content_hash = compute_content_hash(bytes);

    // Source-identity branch (slice 2): the plugin tells us which Moodle
    // object this upload represents. When that object already has an
    // active doc with *different* bytes, the Moodle-side material was
    // edited; orphan the previous doc so the source-identity unique
    // index is free for the new row. The previous doc's chunks stay
    // in Qdrant + DB so old chat-history citations still resolve; the
    // retrieval-time filter (`orphaned_doc_ids`) keeps them out of new
    // turns. If the existing doc has the same bytes, we fall through
    // to the content-hash dedup below and return that same row.
    if let (Some(sys), Some(sref)) = (source_system, source_ref) {
        if let Some(prev) = minerva_db::queries::documents::find_active_by_source_ref(
            &state.db, course_id, sys, sref,
        )
        .await?
        {
            if prev.content_hash.as_deref() == Some(content_hash.as_str()) {
                return Ok(prev);
            }
            minerva_db::queries::documents::orphan(&state.db, prev.id).await?;
        }
    }

    if let Some(existing) = minerva_db::queries::documents::find_active_by_content_hash(
        &state.db,
        course_id,
        &content_hash,
    )
    .await?
    {
        // Same bytes already in this course as an active doc; reuse it.
        // Slice 2 caveat: when a `source_ref` collision was orphaned
        // above and the new bytes match a *different* active doc, we
        // return that other doc and do NOT re-tag it with the
        // caller's source_ref (per-doc source_ref is a single value;
        // a many-to-many table would change the schema shape and is
        // out of scope for this slice). Net effect: cross-source
        // content collisions stay tracked under whichever source
        // first registered them.
        return Ok(existing);
    }

    let doc_id = Uuid::new_v4();
    let dir = format!("{}/{}", state.config.docs_path, course_id);
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| AppError::Internal(format!("failed to create directory: {}", e)))?;

    let ext = extension_from_filename(filename);
    let file_path = format!("{}/{}.{}", dir, doc_id, ext);
    tokio::fs::write(&file_path, bytes)
        .await
        .map_err(|e| AppError::Internal(format!("failed to write file: {}", e)))?;

    let size_bytes = bytes.len() as i64;
    let row = minerva_db::queries::documents::insert(
        &state.db,
        minerva_db::queries::documents::NewDocument {
            id: doc_id,
            course_id,
            filename,
            mime_type,
            size_bytes,
            uploaded_by,
            source_url,
            content_hash: Some(&content_hash),
            source_system,
            source_ref,
            // Teacher / Moodle / Canvas uploads are first-class; only the
            // worker materializing a `text/x-url` stub sets a parent.
            parent_document_id: None,
        },
    )
    .await?;
    Ok(row)
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

    // Teacher-facing upload accepts an optional `source_ref` multipart
    // field. When set, the doc is tagged with `source_system = "manual"`
    // (UI uploads, as opposed to the integration `"moodle"` /
    // `"canvas"` paths). This gives teachers a manual versioning
    // story: re-uploading a new file under the same `source_ref`
    // orphans the previous active doc and the new one supersedes it,
    // mirroring the plugin's update flow. Empty `source_ref` =
    // untagged, no source-identity behavior.
    let mut file_bytes: Option<axum::body::Bytes> = None;
    let mut filename = String::from("document");
    let mut content_type = String::from("application/octet-stream");
    let mut source_ref: Option<String> = None;

    while let Some(field) = multipart.next_field().await.map_err(|e| {
        AppError::bad_request_with("doc.multipart_error", [("detail", e.to_string())])
    })? {
        match field.name() {
            // Accept both the named `file` field (recommended) and an
            // unnamed first field (legacy / curl-friendly callers).
            Some("file") | None => {
                filename = field.file_name().unwrap_or("document").to_string();
                content_type = field
                    .content_type()
                    .unwrap_or("application/octet-stream")
                    .to_string();
                file_bytes = Some(field.bytes().await.map_err(|e| {
                    AppError::bad_request_with("doc.read_failed", [("detail", e.to_string())])
                })?);
            }
            Some("source_ref") => {
                let v = field.text().await.map_err(|e| {
                    AppError::bad_request_with("doc.read_failed", [("detail", e.to_string())])
                })?;
                let v = v.trim();
                if !v.is_empty() {
                    source_ref = Some(v.to_string());
                }
            }
            Some(_) => {
                // Unknown fields are dropped silently so callers stay
                // forward-compatible with future additions.
                let _ = field.bytes().await;
            }
        }
    }

    let data = file_bytes.ok_or_else(|| AppError::bad_request("doc.no_file"))?;

    let size_bytes = data.len() as i64;
    let max_upload_bytes = crate::system_defaults::max_upload_bytes(&state.db).await;
    if size_bytes > max_upload_bytes {
        return Err(AppError::bad_request_with(
            "doc.file_too_large",
            [
                ("size_bytes", size_bytes.to_string()),
                ("max_mb", (max_upload_bytes / 1_000_000).to_string()),
            ],
        ));
    }

    // Server-side dedup: re-uploading the same bytes (e.g. a teacher
    // dragging the same PDF in twice) returns the existing doc instead
    // of inserting a duplicate. See `upload_or_dedup` for the full
    // contract.
    let source_system = source_ref.as_ref().map(|_| "manual");
    let row = upload_or_dedup(
        &state,
        course_id,
        &filename,
        &content_type,
        &data,
        user.id,
        None,
        source_system,
        source_ref.as_deref(),
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
    // Snapshot the cap once at the start of the stream. We could
    // re-read per chunk but that would only matter if an admin lowered
    // the cap mid-upload, and the surrounding bytes-already-on-disk
    // would be wasted either way.
    let max_mbz_upload_bytes = crate::system_defaults::max_mbz_upload_bytes(&state.db).await;
    while let Some(chunk) = stream.next().await {
        let bytes = chunk.map_err(|e| {
            AppError::bad_request_with("doc.read_failed", [("detail", e.to_string())])
        })?;
        total += bytes.len() as i64;
        if total > max_mbz_upload_bytes {
            return Err(AppError::bad_request_with(
                "doc.file_too_large",
                [
                    ("size_bytes", total.to_string()),
                    ("max_mb", (max_mbz_upload_bytes / 1_000_000).to_string()),
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
    let import = tokio::task::spawn_blocking(move || minerva_mbz::import_mbz(&parse_path))
        .await
        .map_err(|e| AppError::Internal(format!("mbz parse task panicked: {e}")))?
        .map_err(|e| {
            AppError::bad_request_with("doc.mbz_parse_failed", [("detail", e.to_string())])
        })?;

    let mut imported: usize = 0;
    for item in &import.items {
        // Read bytes into memory so we can hash them for dedup. .mbz items
        // are individual course resources (bounded by Moodle's per-file
        // cap, in practice ≪ MAX_UPLOAD_BYTES); buffering them here is
        // fine even on the biggest backups we've seen in production.
        let bytes: Vec<u8> = match &item.body {
            minerva_mbz::ItemBody::Inline(bytes) => bytes.clone(),
            minerva_mbz::ItemBody::File(src) => tokio::fs::read(src).await.map_err(|e| {
                AppError::Internal(format!("failed to read {}: {}", item.filename, e))
            })?,
        };

        upload_or_dedup(
            &state,
            course_id,
            &item.filename,
            &item.mime,
            &bytes,
            user.id,
            None,
            None,
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
    /// Teacher-editable source reference. Two-state semantics:
    ///
    /// - field absent (`None`): leave source_ref unchanged.
    /// - field present + empty: clear the source_ref (back to untagged).
    /// - field present + non-empty: set source_ref; auto-tags
    ///   source_system="manual" when the doc had no system yet.
    ///
    /// Plugin-owned docs (source_system in {"moodle","canvas"}) are
    /// protected: the route returns 409 rather than letting a teacher
    /// re-tag them, since that would silently break the plugin's
    /// reconcile semantics on the next sweep.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    source_ref: Option<String>,
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

    if let Some(new_ref_raw) = body.source_ref {
        // Refuse to edit refs owned by a plugin: changing them would
        // silently break the next reconcile sweep ("the moodle plugin
        // listed source_ref X; minerva has Y; orphan everything").
        // Teachers can re-tag UI uploads and other manually-tagged
        // docs freely.
        let owner = doc.source_system.as_deref();
        let editable = matches!(owner, None | Some("manual"));
        if !editable {
            return Err(AppError::bad_request_with(
                "doc.source_ref_plugin_owned",
                [("source_system", owner.unwrap_or("").to_string())],
            ));
        }
        let trimmed = new_ref_raw.trim();
        let new_ref: Option<&str> = if trimmed.is_empty() {
            None
        } else {
            Some(trimmed)
        };
        // Setting source_ref auto-tags source_system="manual"; clearing
        // it also clears source_system so the row goes back to looking
        // like an untagged UI upload.
        let new_sys: Option<&str> = new_ref.map(|_| "manual");
        minerva_db::queries::documents::set_source_identity(&state.db, doc_id, new_sys, new_ref)
            .await?;
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

    // URL parents cascade-delete their children at the DB level
    // (ON DELETE CASCADE), but Qdrant vectors and on-disk files don't
    // know about the cascade. Walk children first so a deleted URL stub
    // doesn't leave orphaned PDF/transcript bytes + vectors behind.
    let children = minerva_db::queries::documents::list_children(&state.db, doc_id).await?;
    let collection_name =
        minerva_pipeline::pipeline::collection_name(course_id, course.embedding_version);
    let collection_exists = state
        .qdrant
        .collection_exists(&collection_name)
        .await
        .unwrap_or(false);

    let mut all_ids: Vec<Uuid> = children.iter().map(|c| c.id).collect();
    all_ids.push(doc_id);

    if collection_exists {
        for id in &all_ids {
            let filter =
                qdrant_client::qdrant::Filter::must([qdrant_client::qdrant::Condition::matches(
                    "document_id",
                    id.to_string(),
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
    }

    // Delete the parent from DB; FK cascade removes child rows.
    minerva_db::queries::documents::delete(&state.db, doc_id).await?;

    // Delete files from disk; try common extensions since we don't store
    // the ext in DB. Walk every (parent + child) id so the cascade doesn't
    // leak bytes onto the filesystem.
    for id in &all_ids {
        for ext in &["pdf", "docx", "doc", "pptx", "ppt", "txt", "html", "url"] {
            let file_path = format!("{}/{}/{}.{}", state.config.docs_path, course_id, id, ext);
            if tokio::fs::remove_file(&file_path).await.is_ok() {
                break;
            }
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
        minerva_pipeline::pipeline::collection_name(course_id, course.embedding_version);

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
        minerva_pipeline::pipeline::collection_name(course_id, course.embedding_version);
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

    // Admin search UI: exclude orphaned docs at the Qdrant layer so the
    // top-N contract is preserved (same reasoning as the chat path).
    let orphaned = minerva_db::queries::documents::orphaned_doc_ids(&state.db, course_id)
        .await
        .unwrap_or_default();
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
        &orphaned,
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

// `extension_from_filename` moved to `minerva_pipeline` (shared by the
// worker, which must not depend on the axum route tree). Re-exported
// here so the in-crate upload routes keep calling it via the old path.
pub use minerva_pipeline::pipeline::extension_from_filename;

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
    let text = minerva_pipeline::pipeline::extract_document_text(path)
        .map_err(|e| AppError::Internal(format!("text extraction failed: {}", e)))?;

    let classifier = crate::classification::LlmClassifier::new(
        reqwest::Client::new(),
        state.utility_model().await,
        state.db.clone(),
    );
    use minerva_pipeline::classifier::Classifier;
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
            minerva_pipeline::pipeline::collection_name_for_course(&state.db, course_id)
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
// `relink_course` moved to `crate::relink_scheduler` (shared with the
// relink sweeper, which runs outside the axum route tree). The route
// handler below calls it via `crate::relink_scheduler::relink_course`.

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
    let edges = crate::relink_scheduler::relink_course(&state, course_id)
        .await
        .map_err(AppError::Internal)?;
    Ok(Json(RelinkResponse { edges }))
}
