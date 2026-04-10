//! Service API for automated pipelines (e.g. transcript fetcher).
//!
//! Authenticated via `Authorization: Bearer <key>` where the key matches
//! the `MINERVA_SERVICE_API_KEY` environment variable. This is a global
//! key, not scoped to any course.

use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::AppError;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/pending-transcripts", get(pending_transcripts))
        .route(
            "/documents/{document_id}/transcript",
            post(submit_transcript),
        )
}

/// Authenticate using the global service API key (MINERVA_SERVICE_API_KEY).
fn authenticate_service(state: &AppState, headers: &HeaderMap) -> Result<(), AppError> {
    let configured_key = state
        .config
        .service_api_key
        .as_deref()
        .ok_or(AppError::Unauthorized)?;

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

#[derive(Serialize)]
struct PendingTranscriptInfo {
    id: Uuid,
    course_id: Uuid,
    filename: String,
    url: String,
}

/// List URL documents that are waiting for external transcript processing.
/// Returns the URL content from each `.url` file so the caller knows what to fetch.
async fn pending_transcripts(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<PendingTranscriptInfo>>, AppError> {
    authenticate_service(&state, &headers)?;

    let docs = minerva_db::queries::documents::list_awaiting_transcripts(&state.db).await?;
    let mut result = Vec::new();

    for doc in docs {
        let ext = super::documents::extension_from_filename(&doc.filename);
        let file_path = format!(
            "{}/{}/{}.{}",
            state.config.docs_path, doc.course_id, doc.id, ext
        );
        let url = match tokio::fs::read_to_string(&file_path).await {
            Ok(content) => content.trim().to_string(),
            Err(_) => continue,
        };
        result.push(PendingTranscriptInfo {
            id: doc.id,
            course_id: doc.course_id,
            filename: doc.filename,
            url,
        });
    }

    Ok(Json(result))
}

#[derive(Deserialize)]
struct SubmitTranscriptRequest {
    /// Transcript text content. If provided, the document is re-queued for ingestion.
    text: Option<String>,
    /// Error message. If provided (and text is absent), the document is marked as failed.
    error: Option<String>,
}

/// Submit a transcript for a URL document, or report that no transcript is available.
async fn submit_transcript(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(document_id): Path<Uuid>,
    Json(body): Json<SubmitTranscriptRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    authenticate_service(&state, &headers)?;

    let doc = minerva_db::queries::documents::find_by_id(&state.db, document_id)
        .await?
        .ok_or(AppError::NotFound)?;

    if doc.status != "awaiting_transcript" {
        return Err(AppError::BadRequest(format!(
            "document status is '{}', expected 'awaiting_transcript'",
            doc.status,
        )));
    }

    if let Some(text) = &body.text {
        if text.is_empty() {
            return Err(AppError::BadRequest("transcript text is empty".to_string()));
        }

        // Save transcript as .txt file.
        let dir = format!("{}/{}", state.config.docs_path, doc.course_id);
        let txt_path = format!("{}/{}.txt", dir, doc.id);
        tokio::fs::write(&txt_path, text.as_bytes())
            .await
            .map_err(|e| AppError::Internal(format!("failed to write transcript: {}", e)))?;

        // Update DB: new filename, mime type, size, reset to pending.
        let new_filename = doc
            .filename
            .strip_suffix(".url")
            .unwrap_or(&doc.filename)
            .to_string()
            + ".txt";
        let size_bytes = text.len() as i64;

        let updated = minerva_db::queries::documents::replace_with_transcript(
            &state.db,
            doc.id,
            &new_filename,
            "text/plain",
            size_bytes,
        )
        .await?;

        if !updated {
            return Err(AppError::BadRequest(
                "document status changed concurrently".to_string(),
            ));
        }

        tracing::info!(
            "transcript submitted for document {} ({} bytes), re-queued for ingestion",
            doc.id,
            size_bytes,
        );

        Ok(Json(
            serde_json::json!({ "status": "queued", "filename": new_filename }),
        ))
    } else if let Some(error) = &body.error {
        // Mark as failed so we don't retry.
        let _ = sqlx::query("UPDATE documents SET status = 'failed', error_msg = $1 WHERE id = $2")
            .bind(error)
            .bind(doc.id)
            .execute(&state.db)
            .await;

        tracing::info!("document {} marked as failed: {}", doc.id, error);

        Ok(Json(
            serde_json::json!({ "status": "failed", "error": error }),
        ))
    } else {
        Err(AppError::BadRequest(
            "provide either 'text' or 'error'".to_string(),
        ))
    }
}
