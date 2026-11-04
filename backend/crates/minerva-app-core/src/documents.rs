//! Adding a document to a workspace from bytes. Shared by the api's upload
//! routes and the scheduler's syllabus loop, so dedup and source-identity
//! replacement behave the same wherever a document comes from.

use uuid::Uuid;

use crate::error::AppError;

/// Idempotent upload helper used by every route that ingests a document.
///
/// Path: compute the bytes' sha256 -> if `(workspace_id, content_hash)` matches
/// an active row, return it without touching disk; else write the file under
/// `{docs_path}/{workspace_id}/{new_doc_id}.{ext}` and insert. The on-disk file
/// for a dedup hit stays where it already was under the existing doc's id;
/// callers do not need to clean up.
///
/// `source_system` / `source_ref` are wired through for slice 2 (Moodle source
/// identity); slice 1 callers pass `None`. `source_url` is the legacy
/// per-URL idempotency key already used by the URL-stub flow; we still pass
/// it through unchanged.
#[allow(clippy::too_many_arguments)]
pub async fn upload_or_dedup(
    db: &sqlx::PgPool,
    docs_path: &str,
    workspace_id: Uuid,
    filename: &str,
    mime_type: &str,
    bytes: &[u8],
    uploaded_by: Uuid,
    source_url: Option<&str>,
    source_system: Option<&str>,
    source_ref: Option<&str>,
) -> Result<minerva_db::queries::documents::DocumentRow, AppError> {
    let content_hash = minerva_pipeline::pipeline::compute_content_hash(bytes);

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
        if let Some(prev) =
            minerva_db::queries::documents::find_active_by_source_ref(db, workspace_id, sys, sref)
                .await?
        {
            if prev.content_hash.as_deref() == Some(content_hash.as_str()) {
                return Ok(prev);
            }
            minerva_db::queries::documents::orphan(db, prev.id).await?;
        }
    }

    if let Some(existing) =
        minerva_db::queries::documents::find_active_by_content_hash(db, workspace_id, &content_hash)
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
    let dir = format!("{}/{}", docs_path, workspace_id);
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| AppError::Internal(format!("failed to create directory: {}", e)))?;

    let ext = minerva_pipeline::pipeline::extension_from_filename(filename);
    let file_path = format!("{}/{}.{}", dir, doc_id, ext);
    tokio::fs::write(&file_path, bytes)
        .await
        .map_err(|e| AppError::Internal(format!("failed to write file: {}", e)))?;

    let size_bytes = bytes.len() as i64;
    let row = minerva_db::queries::documents::insert(
        db,
        minerva_db::queries::documents::NewDocument {
            id: doc_id,
            workspace_id,
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
