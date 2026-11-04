//! Files attached to a single conversation (see migration
//! `20261104000004_conversation_attachments.sql`). Mounted under
//! `/api/workspaces/{workspace_id}/conversations`.
//!
//! Only the conversation's owner can attach, list or remove files, and
//! only in workspace kinds that use attachments (credit transfer: the
//! external syllabus a case is about). Nothing here reaches the shared
//! search index; the strategies give the extracted text to the model as
//! reference material on every turn of the conversation.

use axum::extract::{Extension, Multipart, Path, State};
use axum::routing::{delete, post};
use axum::{Json, Router};
use minerva_core::models::{User, WorkspaceKind};
use serde::Serialize;
use uuid::Uuid;

use crate::error::AppError;
use crate::state::AppState;

/// Per-attachment cap on extracted text. A syllabus is a few thousand
/// characters; this only stops a book-sized upload from flooding every
/// prompt in the conversation.
pub const MAX_ATTACHMENT_CHARS: usize = 60_000;

/// Per-conversation cap, so a case comparing several external courses
/// still fits the model's context next to the retrieved plans.
const MAX_ATTACHMENTS_PER_CONVERSATION: usize = 8;

pub fn router() -> Router<AppState> {
    Router::new()
        // Start a new case from its first file: creates the conversation,
        // so the attachment exists before the first message is sent.
        .route(
            "/conversations/attachments",
            post(start_with_attachment).layer(axum::extract::DefaultBodyLimit::max(
                super::documents::UPLOAD_BODY_LIMIT_CEILING as usize,
            )),
        )
        .route(
            "/conversations/{cid}/attachments",
            post(add_attachment)
                .get(list_attachments)
                .layer(axum::extract::DefaultBodyLimit::max(
                    super::documents::UPLOAD_BODY_LIMIT_CEILING as usize,
                )),
        )
        .route(
            "/conversations/{cid}/attachments/{attachment_id}",
            delete(delete_attachment),
        )
}

/// Workspace kinds whose conversations take attachments.
pub fn kind_uses_attachments(kind: &str) -> bool {
    WorkspaceKind::parse(kind) == Some(WorkspaceKind::CreditTransfer)
}

use minerva_app_core::case_retention::attachment_path;

#[derive(Serialize)]
pub(crate) struct AttachmentResponse {
    id: Uuid,
    conversation_id: Uuid,
    filename: String,
    mime_type: String,
    size_bytes: i64,
    /// Length of the extracted text, so the UI can flag a scan that
    /// yielded next to nothing.
    text_chars: usize,
    created_at: chrono::DateTime<chrono::Utc>,
}

impl From<minerva_db::queries::conversation_attachments::AttachmentRow> for AttachmentResponse {
    fn from(a: minerva_db::queries::conversation_attachments::AttachmentRow) -> Self {
        Self {
            id: a.id,
            conversation_id: a.conversation_id,
            filename: a.filename,
            mime_type: a.mime_type,
            size_bytes: a.size_bytes,
            text_chars: a.extracted_text.chars().count(),
            created_at: a.created_at,
        }
    }
}

#[derive(Serialize)]
struct StartResponse {
    conversation_id: Uuid,
    attachment: AttachmentResponse,
}

async fn start_with_attachment(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(workspace_id): Path<Uuid>,
    multipart: Multipart,
) -> Result<Json<StartResponse>, AppError> {
    let workspace = super::chat::verify_course_access_pub(&state, workspace_id, user.id).await?;
    if !kind_uses_attachments(&workspace.kind) {
        return Err(AppError::NotFound);
    }
    let upload = read_upload(&state, multipart).await?;
    let conversation = minerva_db::queries::conversations::create(
        &state.db,
        Uuid::new_v4(),
        workspace_id,
        user.id,
    )
    .await?;
    let attachment = store(&state, workspace_id, conversation.id, user.id, upload).await?;
    Ok(Json(StartResponse {
        conversation_id: conversation.id,
        attachment,
    }))
}

async fn add_attachment(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path((workspace_id, cid)): Path<(Uuid, Uuid)>,
    multipart: Multipart,
) -> Result<Json<AttachmentResponse>, AppError> {
    own_conversation(&state, workspace_id, cid, &user).await?;
    let existing = minerva_db::queries::conversation_attachments::list(&state.db, cid).await?;
    if existing.len() >= MAX_ATTACHMENTS_PER_CONVERSATION {
        return Err(AppError::bad_request_with(
            "attachment.too_many",
            [("max", MAX_ATTACHMENTS_PER_CONVERSATION.to_string())],
        ));
    }
    let upload = read_upload(&state, multipart).await?;
    Ok(Json(
        store(&state, workspace_id, cid, user.id, upload).await?,
    ))
}

async fn list_attachments(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path((workspace_id, cid)): Path<(Uuid, Uuid)>,
) -> Result<Json<Vec<AttachmentResponse>>, AppError> {
    own_conversation(&state, workspace_id, cid, &user).await?;
    let rows = minerva_db::queries::conversation_attachments::list(&state.db, cid).await?;
    Ok(Json(rows.into_iter().map(Into::into).collect()))
}

async fn delete_attachment(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path((workspace_id, cid, attachment_id)): Path<(Uuid, Uuid, Uuid)>,
) -> Result<Json<serde_json::Value>, AppError> {
    own_conversation(&state, workspace_id, cid, &user).await?;
    let removed =
        minerva_db::queries::conversation_attachments::delete(&state.db, cid, attachment_id)
            .await?
            .ok_or(AppError::NotFound)?;
    let path = attachment_path(
        &state.config.docs_path,
        workspace_id,
        removed.id,
        &removed.filename,
    );
    if let Err(e) = tokio::fs::remove_file(&path).await {
        if e.kind() != std::io::ErrorKind::NotFound {
            tracing::warn!("attachment {}: removing {} failed: {}", removed.id, path, e);
        }
    }
    Ok(Json(serde_json::json!({ "deleted": true })))
}

/// The conversation must be the caller's own, in a workspace kind that
/// uses attachments. Teachers reviewing conversations do not get to read
/// or change a case's files: they can carry an applicant's personal data.
async fn own_conversation(
    state: &AppState,
    workspace_id: Uuid,
    cid: Uuid,
    user: &User,
) -> Result<(), AppError> {
    let workspace = super::chat::verify_course_access_pub(state, workspace_id, user.id).await?;
    if !kind_uses_attachments(&workspace.kind) {
        return Err(AppError::NotFound);
    }
    let conversation = minerva_db::queries::conversations::find_by_id(&state.db, cid)
        .await?
        .ok_or(AppError::NotFound)?;
    if conversation.workspace_id != workspace_id || conversation.user_id != user.id {
        return Err(AppError::NotFound);
    }
    Ok(())
}

struct Upload {
    filename: String,
    mime_type: String,
    bytes: axum::body::Bytes,
}

async fn read_upload(state: &AppState, mut multipart: Multipart) -> Result<Upload, AppError> {
    let mut upload: Option<Upload> = None;
    while let Some(field) = multipart.next_field().await.map_err(|e| {
        AppError::bad_request_with("doc.multipart_error", [("detail", e.to_string())])
    })? {
        if matches!(field.name(), Some("file") | None) {
            let filename = field.file_name().unwrap_or("attachment").to_string();
            let mime_type = field
                .content_type()
                .unwrap_or("application/octet-stream")
                .to_string();
            let bytes = field.bytes().await.map_err(|e| {
                AppError::bad_request_with("doc.read_failed", [("detail", e.to_string())])
            })?;
            upload = Some(Upload {
                filename,
                mime_type,
                bytes,
            });
        } else {
            let _ = field.bytes().await;
        }
    }
    let upload = upload.ok_or_else(|| AppError::bad_request("doc.no_file"))?;
    let max_upload_bytes = crate::system_defaults::max_upload_bytes(&state.db).await;
    if upload.bytes.len() as i64 > max_upload_bytes {
        return Err(AppError::bad_request_with(
            "doc.file_too_large",
            [
                ("size_bytes", upload.bytes.len().to_string()),
                ("max_mb", (max_upload_bytes / 1_000_000).to_string()),
            ],
        ));
    }
    Ok(upload)
}

/// Write the file, extract its text and record it. The file is removed
/// again if extraction or the insert fails, so no orphan bytes remain.
async fn store(
    state: &AppState,
    workspace_id: Uuid,
    conversation_id: Uuid,
    uploaded_by: Uuid,
    upload: Upload,
) -> Result<AttachmentResponse, AppError> {
    let ext = minerva_pipeline::pipeline::extension_from_filename(&upload.filename);
    if !matches!(ext, "pdf" | "txt" | "md" | "html" | "htm") {
        return Err(AppError::bad_request_with(
            "attachment.unsupported_type",
            [("ext", ext.to_string())],
        ));
    }
    let id = Uuid::new_v4();
    let path = attachment_path(&state.config.docs_path, workspace_id, id, &upload.filename);
    let dir = std::path::Path::new(&path)
        .parent()
        .expect("attachment path has a directory")
        .to_owned();
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| AppError::Internal(format!("failed to create directory: {e}")))?;
    tokio::fs::write(&path, &upload.bytes)
        .await
        .map_err(|e| AppError::Internal(format!("failed to write file: {e}")))?;

    let result = async {
        let path_buf = std::path::PathBuf::from(&path);
        let text = tokio::task::spawn_blocking(move || {
            minerva_pipeline::pipeline::extract_document_text(&path_buf)
        })
        .await
        .map_err(|e| AppError::Internal(format!("extraction task failed: {e}")))?
        .map_err(|_| AppError::bad_request("attachment.no_text"))?;
        let text = text.trim();
        if text.is_empty() {
            return Err(AppError::bad_request("attachment.no_text"));
        }
        let text: String = text.chars().take(MAX_ATTACHMENT_CHARS).collect();
        let row = minerva_db::queries::conversation_attachments::insert(
            &state.db,
            &minerva_db::queries::conversation_attachments::NewAttachment {
                id,
                conversation_id,
                uploaded_by,
                filename: &upload.filename,
                mime_type: &upload.mime_type,
                size_bytes: upload.bytes.len() as i64,
                content_hash: &minerva_pipeline::pipeline::compute_content_hash(&upload.bytes),
                extracted_text: &text,
            },
        )
        .await?;
        Ok(AttachmentResponse::from(row))
    }
    .await;
    if result.is_err() {
        let _ = tokio::fs::remove_file(&path).await;
    }
    result
}
