//! Retention for credit transfer cases. A case is a conversation in a
//! credit transfer workspace, and its attachments often carry an
//! applicant's personal data, so each one is deleted, files and all, once
//! it has gone untouched for the workspace's `case_retention_days`.

use uuid::Uuid;

use crate::state::AppState;

/// Cases deleted per sweep; the next tick picks up any remainder.
const BATCH_SIZE: i64 = 50;

/// Where an attachment's bytes live. Shared with the api's upload route.
pub fn attachment_path(docs_path: &str, workspace_id: Uuid, id: Uuid, filename: &str) -> String {
    format!(
        "{docs_path}/{workspace_id}/attachments/{id}.{}",
        minerva_pipeline::pipeline::extension_from_filename(filename)
    )
}

/// Delete every expired case. Files go first, then the conversation, whose
/// messages and attachment rows cascade. Returns how many were deleted.
pub async fn sweep(state: &AppState) -> Result<usize, sqlx::Error> {
    use minerva_db::queries::conversation_attachments as q;
    let expired = q::find_expired_cases(&state.db, BATCH_SIZE).await?;
    let mut deleted = 0;
    for case in expired {
        let attachments = q::list(&state.db, case.conversation_id).await?;
        for a in attachments {
            let path = attachment_path(
                &state.config.docs_path,
                case.workspace_id,
                a.id,
                &a.filename,
            );
            if let Err(e) = tokio::fs::remove_file(&path).await {
                if e.kind() != std::io::ErrorKind::NotFound {
                    // Keep the row so the next sweep retries the file.
                    tracing::warn!("case retention: removing {} failed: {}", path, e);
                    continue;
                }
            }
        }
        if q::delete_conversation(&state.db, case.conversation_id).await? {
            deleted += 1;
        }
    }
    Ok(deleted)
}
