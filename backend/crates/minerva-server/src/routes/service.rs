//! Service API for automated pipelines (e.g. transcript fetcher).
//!
//! Authenticated via `Authorization: Bearer <key>` where the key matches
//! the `MINERVA_SERVICE_API_KEY` environment variable. This is a global
//! key, not scoped to any course.

use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::routing::{get, post, put};
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
        .route("/play-designations", get(list_play_designations))
        .route(
            "/play-designations/{designation_id}/mark-synced",
            post(mark_designation_synced),
        )
        .route(
            "/courses/{course_id}/documents/url",
            post(create_url_document),
        )
        .route("/play-courses", put(replace_play_course_catalog))
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
        return Err(AppError::bad_request_with(
            "service.wrong_status",
            [("status", doc.status.clone())],
        ));
    }

    if let Some(text) = &body.text {
        if text.is_empty() {
            return Err(AppError::bad_request("service.transcript_empty"));
        }

        // Save transcript as .txt file.
        let dir = format!("{}/{}", state.config.docs_path, doc.course_id);
        let txt_path = format!("{}/{}.txt", dir, doc.id);
        tokio::fs::write(&txt_path, text.as_bytes())
            .await
            .map_err(|e| AppError::Internal(format!("failed to write transcript: {}", e)))?;

        // Update DB: new filename, mime type, size, reset to pending.
        // Prefix with "transcript_" so the classifier's filename_hints
        // recognises this as auto-generated speech-to-text and assigns
        // the `lecture_transcript` kind (lower-quality / messier text
        // than prepared lecture notes; teachers can spot why retrieval
        // surfaces awkward speech blocks).
        let stem = doc.filename.strip_suffix(".url").unwrap_or(&doc.filename);
        let new_filename = if stem.starts_with("transcript_") {
            // already-prefixed (re-submission path); don't double-prefix
            format!("{stem}.txt")
        } else {
            format!("transcript_{stem}.txt")
        };
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
            return Err(AppError::bad_request("service.status_changed_concurrently"));
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
        let _ = sqlx::query!(
            "UPDATE documents SET status = 'failed', error_msg = $1 WHERE id = $2",
            error,
            doc.id,
        )
        .execute(&state.db)
        .await;

        tracing::info!("document {} marked as failed: {}", doc.id, error);

        Ok(Json(
            serde_json::json!({ "status": "failed", "error": error }),
        ))
    } else {
        Err(AppError::bad_request("service.missing_text_or_error"))
    }
}

// -- Play designations (discovery) --

#[derive(Serialize)]
struct PlayDesignationServiceInfo {
    id: Uuid,
    course_id: Uuid,
    designation: String,
    last_synced_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// List all configured play.dsv.su.se designations across all courses.
/// Used by the transcript pipeline to discover new presentations.
async fn list_play_designations(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<PlayDesignationServiceInfo>>, AppError> {
    authenticate_service(&state, &headers)?;

    let rows = minerva_db::queries::play_designations::list_all(&state.db).await?;
    Ok(Json(
        rows.into_iter()
            .map(|r| PlayDesignationServiceInfo {
                id: r.id,
                course_id: r.course_id,
                designation: r.designation,
                last_synced_at: r.last_synced_at,
            })
            .collect(),
    ))
}

#[derive(Deserialize)]
struct MarkSyncedRequest {
    /// Optional error message. If absent, sync is marked as successful.
    error: Option<String>,
}

async fn mark_designation_synced(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(designation_id): Path<Uuid>,
    Json(body): Json<MarkSyncedRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    authenticate_service(&state, &headers)?;

    let existing = minerva_db::queries::play_designations::find_by_id(&state.db, designation_id)
        .await?
        .ok_or(AppError::NotFound)?;

    if let Some(err) = &body.error {
        minerva_db::queries::play_designations::mark_synced_error(&state.db, existing.id, err)
            .await?;
        Ok(Json(serde_json::json!({ "status": "error", "error": err })))
    } else {
        minerva_db::queries::play_designations::mark_synced_ok(&state.db, existing.id).await?;
        Ok(Json(serde_json::json!({ "status": "ok" })))
    }
}

#[derive(Deserialize)]
struct CreateUrlDocumentRequest {
    /// URL to index (e.g. `https://play.dsv.su.se/presentation/{uuid}`).
    url: String,
    /// Human-readable filename (without `.url` suffix required).
    filename: String,
}

#[derive(Serialize)]
struct CreateUrlDocumentResponse {
    id: Uuid,
    course_id: Uuid,
    filename: String,
    status: String,
    created: bool,
}

/// Sanitize a filename: strip path separators, disallow `..`, trim whitespace,
/// and cap length. Ensures `.url` suffix.
fn sanitize_url_filename(raw: &str) -> Result<String, AppError> {
    let mut name: String = raw
        .chars()
        .filter(|c| !matches!(c, '/' | '\\' | '\0'))
        .collect::<String>()
        .trim()
        .to_string();

    if name.is_empty() || name == "." || name == ".." {
        return Err(AppError::bad_request("service.filename_empty"));
    }

    // Cap at 200 chars before the .url suffix.
    if !name.ends_with(".url") {
        if name.len() > 200 {
            name.truncate(200);
        }
        name.push_str(".url");
    } else if name.len() > 204 {
        name.truncate(200);
        name.push_str(".url");
    }

    Ok(name)
}

/// Idempotently create a URL document in a course.
///
/// Dedup key is the `source_url` column (enforced atomically by a partial
/// unique index on `(course_id, source_url)`). If a document with the same
/// origin URL already exists -- regardless of its current status or mime_type
/// (a successful transcript fetch rewrites mime_type to text/plain) -- return
/// it with `created=false`.
async fn create_url_document(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(course_id): Path<Uuid>,
    Json(body): Json<CreateUrlDocumentRequest>,
) -> Result<Json<CreateUrlDocumentResponse>, AppError> {
    authenticate_service(&state, &headers)?;

    let url = body.url.trim().to_string();
    if url.is_empty() || url.len() > 2048 {
        return Err(AppError::bad_request("service.url_invalid_length"));
    }

    let filename = sanitize_url_filename(&body.filename)?;

    let course = minerva_db::queries::courses::find_by_id(&state.db, course_id)
        .await?
        .ok_or(AppError::NotFound)?;

    // Fast path: already tracked.
    if let Some(doc) =
        minerva_db::queries::documents::find_by_course_source_url(&state.db, course_id, &url)
            .await?
    {
        return Ok(Json(CreateUrlDocumentResponse {
            id: doc.id,
            course_id: doc.course_id,
            filename: doc.filename,
            status: doc.status,
            created: false,
        }));
    }

    // Create new URL document.
    let doc_id = Uuid::new_v4();
    let dir = format!("{}/{}", state.config.docs_path, course_id);
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| AppError::Internal(format!("failed to create directory: {}", e)))?;

    let file_path = format!("{}/{}.url", dir, doc_id);
    tokio::fs::write(&file_path, url.as_bytes())
        .await
        .map_err(|e| AppError::Internal(format!("failed to write url file: {}", e)))?;

    let size_bytes = url.len() as i64;
    let insert_result = minerva_db::queries::documents::insert(
        &state.db,
        doc_id,
        course_id,
        &filename,
        "text/x-url",
        size_bytes,
        course.owner_id,
        Some(&url),
    )
    .await;

    let row = match insert_result {
        Ok(row) => row,
        Err(sqlx::Error::Database(db_err)) if db_err.is_unique_violation() => {
            // Concurrent creator won the race. Clean up our orphan file and
            // return the winner.
            let _ = tokio::fs::remove_file(&file_path).await;
            let existing = minerva_db::queries::documents::find_by_course_source_url(
                &state.db, course_id, &url,
            )
            .await?
            .ok_or_else(|| {
                AppError::Internal(
                    "unique violation on source_url but no matching row found".into(),
                )
            })?;
            return Ok(Json(CreateUrlDocumentResponse {
                id: existing.id,
                course_id: existing.course_id,
                filename: existing.filename,
                status: existing.status,
                created: false,
            }));
        }
        Err(e) => return Err(e.into()),
    };

    tracing::info!(
        "service created url document {} in course {} ({})",
        row.id,
        course_id,
        url,
    );

    Ok(Json(CreateUrlDocumentResponse {
        id: row.id,
        course_id: row.course_id,
        filename: row.filename,
        status: row.status,
        created: true,
    }))
}

// -- Play course catalog --

#[derive(Deserialize)]
struct PlayCourseEntry {
    code: String,
    name: String,
}

/// Replace/upsert the cached catalog of play.dsv.su.se course designations.
/// Pushed by the transcript pipeline at the start of each run.
async fn replace_play_course_catalog(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Vec<PlayCourseEntry>>,
) -> Result<Json<serde_json::Value>, AppError> {
    authenticate_service(&state, &headers)?;

    let entries: Vec<(String, String)> = body
        .into_iter()
        .filter_map(|e| {
            let code = e.code.trim().to_string();
            let name = e.name.trim().to_string();
            if code.is_empty() || name.is_empty() {
                None
            } else {
                Some((code, name))
            }
        })
        .collect();

    let n = entries.len();
    let upserted =
        minerva_db::queries::play_course_catalog::upsert_many(&state.db, &entries).await?;

    tracing::info!(
        "play catalog upsert: {} submitted, {} rows touched",
        n,
        upserted
    );
    Ok(Json(
        serde_json::json!({ "submitted": n, "upserted": upserted }),
    ))
}
