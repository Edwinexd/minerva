//! Service API for automated pipelines (e.g. transcript fetcher).
//!
//! Authenticated via `Authorization: Bearer <key>` where the key matches
//! the `MINERVA_SERVICE_API_KEY` environment variable. This is a global
//! key, not scoped to any course.

use axum::body::Body;
use axum::extract::{Multipart, Path, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use tokio_util::io::ReaderStream;
use uuid::Uuid;

use crate::error::AppError;
use crate::state::AppState;

/// Maximum size of a video frames bundle uploaded by the GH ingest worker.
/// At fps=1/5 with the blank-frame pre-filter, an hour-long lecture is
/// typically 50-150 MB compressed; 500 MB gives ample headroom for longer
/// lectures and higher sample rates without opening the door to abuse.
pub const MAX_BUNDLE_UPLOAD_BYTES: i64 = 500 * 1_000_000;

/// Maximum size of a single figure crop PNG uploaded by RunPod.
pub const MAX_FIGURE_UPLOAD_BYTES: i64 = 20 * 1_000_000;

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
        // OCR + video-indexing pipeline: RunPod fetches sources from these
        // endpoints, the GH ingest worker uploads bundles, the backend's
        // submitter consumes the pending-* listings.
        .route("/pending-ocr", get(pending_ocr))
        .route("/pending-video-index", get(pending_video_index))
        .route("/documents/{document_id}/source", get(get_source))
        .route(
            "/documents/{document_id}/video-bundle",
            get(get_video_bundle).post(post_video_bundle).layer(
                axum::extract::DefaultBodyLimit::max(MAX_BUNDLE_UPLOAD_BYTES as usize),
            ),
        )
        .route(
            "/figure-uploads/{document_id}",
            post(post_figure_upload).layer(axum::extract::DefaultBodyLimit::max(
                MAX_FIGURE_UPLOAD_BYTES as usize,
            )),
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
        // The classifier never sees filenames; it decides
        // lecture_transcript vs lecture from the actual content (a VTT
        // transcript is recognisable by its disfluencies and lack of
        // structure). So we just swap .url for .txt without injecting
        // any marker token.
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

//; Play designations (discovery) --

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
/// origin URL already exists; regardless of its current status or mime_type
/// (a successful transcript fetch rewrites mime_type to text/plain); return
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

//; Play course catalog --

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

//; OCR + video-indexing pipeline --

/// Where on disk we stash a doc's primary blob (PDF, image, or `.url`).
/// Mirrors the convention used by `routes::documents::upload_document`.
fn doc_source_path(docs_path: &str, course_id: Uuid, doc_id: Uuid, ext: &str) -> String {
    format!("{}/{}/{}.{}", docs_path, course_id, doc_id, ext)
}

fn doc_bundle_path(docs_path: &str, course_id: Uuid, doc_id: Uuid) -> String {
    format!("{}/{}/{}.bundle.tar.zst", docs_path, course_id, doc_id)
}

fn doc_vtt_path(docs_path: &str, course_id: Uuid, doc_id: Uuid) -> String {
    format!("{}/{}/{}.vtt", docs_path, course_id, doc_id)
}

fn figures_dir(docs_path: &str) -> String {
    format!("{}/figures", docs_path)
}

fn figure_storage_path(docs_path: &str, figure_id: Uuid) -> String {
    format!("{}/figures/{}.png", docs_path, figure_id)
}

#[derive(Serialize)]
struct PendingOcrInfo {
    id: Uuid,
    course_id: Uuid,
    filename: String,
    mime_type: String,
    /// Absolute URL of `GET /api/service/documents/{id}/source`. The RunPod
    /// handler fetches the PDF/image bytes from here using its service key.
    source_url: String,
}

/// List PDFs and images waiting on a RunPod OCR pass. Cap with `?limit=N`
/// (default 50) so a tight worker poll doesn't repeatedly scan a huge
/// queue.
async fn pending_ocr(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::extract::Query(params): axum::extract::Query<PendingQuery>,
) -> Result<Json<Vec<PendingOcrInfo>>, AppError> {
    authenticate_service(&state, &headers)?;
    let limit = params.limit.unwrap_or(50).clamp(1, 500);
    let docs = minerva_db::queries::documents::list_awaiting_ocr(&state.db, limit).await?;
    let base = &state.config.runpod_callback_base;
    Ok(Json(
        docs.into_iter()
            .map(|d| PendingOcrInfo {
                id: d.id,
                course_id: d.course_id,
                filename: d.filename,
                mime_type: d.mime_type,
                source_url: format!("{}/api/service/documents/{}/source", base, d.id),
            })
            .collect(),
    ))
}

#[derive(Deserialize)]
struct PendingQuery {
    limit: Option<i32>,
}

#[derive(Serialize)]
struct PendingVideoIndexInfo {
    id: Uuid,
    course_id: Uuid,
    filename: String,
    bundle_url: String,
    /// Pre-fetched VTT text shipped with the bundle. The handler doesn't
    /// re-pull from play.dsv; same VTT applies for the lifetime of the doc.
    vtt_text: String,
    sample_fps: String,
}

async fn pending_video_index(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::extract::Query(params): axum::extract::Query<PendingQuery>,
) -> Result<Json<Vec<PendingVideoIndexInfo>>, AppError> {
    authenticate_service(&state, &headers)?;
    let limit = params.limit.unwrap_or(50).clamp(1, 500);
    let docs = minerva_db::queries::documents::list_awaiting_video_index(&state.db, limit).await?;
    let base = &state.config.runpod_callback_base;
    let default_fps = state.config.video_sample_fps.clone();

    let mut out = Vec::with_capacity(docs.len());
    for doc in docs {
        let vtt_path = doc_vtt_path(&state.config.docs_path, doc.course_id, doc.id);
        let vtt_text = tokio::fs::read_to_string(&vtt_path)
            .await
            .unwrap_or_default();
        out.push(PendingVideoIndexInfo {
            id: doc.id,
            course_id: doc.course_id,
            filename: doc.filename,
            bundle_url: format!("{}/api/service/documents/{}/video-bundle", base, doc.id),
            vtt_text,
            sample_fps: default_fps.clone(),
        });
    }
    Ok(Json(out))
}

/// Stream the original blob (PDF, image, or `.url`) so RunPod can fetch
/// it without us inlining megabytes into the job input.
async fn get_source(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(document_id): Path<Uuid>,
) -> Result<Response, AppError> {
    authenticate_service(&state, &headers)?;

    let doc = minerva_db::queries::documents::find_by_id(&state.db, document_id)
        .await?
        .ok_or(AppError::NotFound)?;

    let ext = super::documents::extension_from_filename(&doc.filename);
    let path = doc_source_path(&state.config.docs_path, doc.course_id, doc.id, ext);
    stream_file(&path, &doc.mime_type).await
}

async fn get_video_bundle(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(document_id): Path<Uuid>,
) -> Result<Response, AppError> {
    authenticate_service(&state, &headers)?;

    let doc = minerva_db::queries::documents::find_by_id(&state.db, document_id)
        .await?
        .ok_or(AppError::NotFound)?;

    let path = doc_bundle_path(&state.config.docs_path, doc.course_id, doc.id);
    stream_file(&path, "application/zstd").await
}

async fn stream_file(path: &str, mime: &str) -> Result<Response, AppError> {
    let file = match tokio::fs::File::open(path).await {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(AppError::NotFound),
        Err(e) => return Err(AppError::Internal(format!("stat failed: {}", e))),
    };
    let meta = file
        .metadata()
        .await
        .map_err(|e| AppError::Internal(format!("metadata failed: {}", e)))?;
    let stream = ReaderStream::new(file);
    let body = Body::from_stream(stream);
    let mut resp = Response::new(body);
    resp.headers_mut().insert(
        header::CONTENT_TYPE,
        header::HeaderValue::from_str(mime)
            .unwrap_or(header::HeaderValue::from_static("application/octet-stream")),
    );
    resp.headers_mut().insert(
        header::CONTENT_LENGTH,
        header::HeaderValue::from(meta.len()),
    );
    Ok(resp.into_response())
}

#[derive(Deserialize, Default)]
struct VideoBundleMetadata {
    /// Index into the play.dsv presentation's track list (0-based).
    selected_track_index: Option<i32>,
    /// Aggregate visual classifier score for the chosen track.
    slide_track_score: Option<f32>,
    /// {x,y,w,h} in original-frame pixel coords; null = no crop applied.
    #[serde(default)]
    crop_bbox: Option<serde_json::Value>,
    /// ffmpeg sample rate fraction the bundle was extracted at.
    sample_fps: Option<String>,
    /// VTT transcript shipped alongside frames. Empty = caption not yet
    /// available (state -> vtt_pending).
    #[serde(default)]
    vtt_text: Option<String>,
    /// Set by the GH worker when classification rejected every candidate
    /// track. Backend records the flag and falls back to transcript-only
    /// via the existing pipeline (caller is expected to also POST the
    /// transcript to /transcript on the same document, not via this route).
    #[serde(default)]
    slide_track_missing: bool,
}

/// Multipart upload of a video frames bundle.
///
/// Fields:
///   * `metadata` (text): JSON matching `VideoBundleMetadata`.
///   * `bundle` (file): tar.zst containing manifest.json + frames/*.png.
///   * `vtt` (file, optional): pre-fetched VTT text. Same as
///     metadata.vtt_text but lets the GH worker stream large captions
///     instead of inlining them in JSON.
async fn post_video_bundle(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(document_id): Path<Uuid>,
    mut multipart: Multipart,
) -> Result<Json<serde_json::Value>, AppError> {
    authenticate_service(&state, &headers)?;

    let doc = minerva_db::queries::documents::find_by_id(&state.db, document_id)
        .await?
        .ok_or(AppError::NotFound)?;

    // Status guard: only accept bundles for docs the worker actually parked
    // in awaiting_video_index. This protects against stale GH retries
    // overwriting an already-processed timeline.
    if !matches!(
        doc.status.as_str(),
        "awaiting_video_index" | "vtt_pending" | "video_index_failed"
    ) {
        return Err(AppError::bad_request_with(
            "service.wrong_status",
            [("status", doc.status.clone())],
        ));
    }

    let mut bundle_bytes: Option<bytes::Bytes> = None;
    let mut vtt_text: Option<String> = None;
    let mut metadata = VideoBundleMetadata::default();

    while let Some(field) = multipart.next_field().await.map_err(|e| {
        AppError::bad_request_with("service.multipart_error", [("detail", e.to_string())])
    })? {
        let name = field.name().unwrap_or("").to_string();
        match name.as_str() {
            "metadata" => {
                let text = field.text().await.map_err(|e| {
                    AppError::bad_request_with("service.read_failed", [("detail", e.to_string())])
                })?;
                metadata = serde_json::from_str(&text).map_err(|e| {
                    AppError::bad_request_with(
                        "service.metadata_invalid",
                        [("detail", e.to_string())],
                    )
                })?;
            }
            "bundle" => {
                let bytes = field.bytes().await.map_err(|e| {
                    AppError::bad_request_with("service.read_failed", [("detail", e.to_string())])
                })?;
                bundle_bytes = Some(bytes);
            }
            "vtt" => {
                let text = field.text().await.map_err(|e| {
                    AppError::bad_request_with("service.read_failed", [("detail", e.to_string())])
                })?;
                vtt_text = Some(text);
            }
            _ => {
                // Ignore unknown fields rather than 400ing so we can add
                // optional ones (e.g. provenance receipts) without
                // breaking older GH workers mid-deploy.
            }
        }
    }

    if metadata.slide_track_missing {
        // Caller flagged that no track had usable slides. Persist the
        // signal and bounce them to use the legacy transcript route.
        // Don't accept a bundle here so we don't waste disk.
        minerva_db::queries::documents::set_video_track_metadata(
            &state.db,
            doc.id,
            metadata.selected_track_index,
            metadata.slide_track_score,
            metadata.crop_bbox.as_ref(),
            metadata.sample_fps.as_deref(),
            true,
        )
        .await?;
        return Ok(Json(
            serde_json::json!({ "status": "slide_track_missing", "next": "post transcript via /documents/{id}/transcript" }),
        ));
    }

    let bundle = bundle_bytes.ok_or_else(|| AppError::bad_request("service.bundle_missing"))?;

    let dir = format!("{}/{}", state.config.docs_path, doc.course_id);
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| AppError::Internal(format!("mkdir failed: {}", e)))?;

    let bundle_path = doc_bundle_path(&state.config.docs_path, doc.course_id, doc.id);
    tokio::fs::write(&bundle_path, &bundle)
        .await
        .map_err(|e| AppError::Internal(format!("bundle write failed: {}", e)))?;

    // Resolve VTT precedence: explicit file beats inline metadata.
    let resolved_vtt = vtt_text.or(metadata.vtt_text.clone());
    if let Some(ref text) = resolved_vtt {
        let vtt_path = doc_vtt_path(&state.config.docs_path, doc.course_id, doc.id);
        tokio::fs::write(&vtt_path, text.as_bytes())
            .await
            .map_err(|e| AppError::Internal(format!("vtt write failed: {}", e)))?;
    }

    minerva_db::queries::documents::set_video_track_metadata(
        &state.db,
        doc.id,
        metadata.selected_track_index,
        metadata.slide_track_score,
        metadata.crop_bbox.as_ref(),
        metadata.sample_fps.as_deref(),
        false,
    )
    .await?;

    // Flip status: vtt_pending if no caption yet, awaiting_video_index
    // (the OCR submitter's queue) if we have everything.
    let next_status = if resolved_vtt
        .as_ref()
        .map(|s| !s.trim().is_empty())
        .unwrap_or(false)
    {
        "awaiting_video_index"
    } else {
        "vtt_pending"
    };
    sqlx::query!(
        "UPDATE documents SET status = $1 WHERE id = $2",
        next_status,
        doc.id,
    )
    .execute(&state.db)
    .await?;

    tracing::info!(
        "service: video bundle stored for document {} ({} bytes), status -> {}",
        doc.id,
        bundle.len(),
        next_status,
    );

    Ok(Json(serde_json::json!({
        "status": next_status,
        "bytes": bundle.len(),
    })))
}

#[derive(Deserialize)]
struct FigureUploadMetadata {
    /// Stable identifier the RunPod handler picked. The metadata table
    /// uses it as PK so the handler can reference figures from the
    /// markdown body (e.g. `![Fig 1](minerva-figure:abc-123)`).
    figure_id: Uuid,
    /// 1-based PDF page or null for video-derived figures.
    #[serde(default)]
    page: Option<i32>,
    #[serde(default)]
    t_start_seconds: Option<f32>,
    #[serde(default)]
    t_end_seconds: Option<f32>,
    /// {x,y,w,h} normalized 0..1 within the OCRed image.
    #[serde(default)]
    bbox: Option<serde_json::Value>,
    #[serde(default)]
    caption: Option<String>,
}

/// Multipart endpoint the RunPod handler hits to upload a figure crop.
/// Writes the PNG to `figures/<figure_id>.png` and inserts a row in
/// `document_figures`. Idempotent on `figure_id` collision: the second
/// upload overwrites the file and refreshes the metadata. Used by both
/// `ocr_pdf` (page figures) and `video_index` (slide thumbnails).
async fn post_figure_upload(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(document_id): Path<Uuid>,
    mut multipart: Multipart,
) -> Result<Json<serde_json::Value>, AppError> {
    authenticate_service(&state, &headers)?;

    let doc = minerva_db::queries::documents::find_by_id(&state.db, document_id)
        .await?
        .ok_or(AppError::NotFound)?;

    let mut metadata: Option<FigureUploadMetadata> = None;
    let mut png_bytes: Option<bytes::Bytes> = None;

    while let Some(field) = multipart.next_field().await.map_err(|e| {
        AppError::bad_request_with("service.multipart_error", [("detail", e.to_string())])
    })? {
        let name = field.name().unwrap_or("").to_string();
        match name.as_str() {
            "metadata" => {
                let text = field.text().await.map_err(|e| {
                    AppError::bad_request_with("service.read_failed", [("detail", e.to_string())])
                })?;
                metadata = Some(serde_json::from_str(&text).map_err(|e| {
                    AppError::bad_request_with(
                        "service.metadata_invalid",
                        [("detail", e.to_string())],
                    )
                })?);
            }
            "png" => {
                png_bytes = Some(field.bytes().await.map_err(|e| {
                    AppError::bad_request_with("service.read_failed", [("detail", e.to_string())])
                })?);
            }
            _ => {}
        }
    }

    let metadata = metadata.ok_or_else(|| AppError::bad_request("service.metadata_missing"))?;
    let png = png_bytes.ok_or_else(|| AppError::bad_request("service.png_missing"))?;

    let dir = figures_dir(&state.config.docs_path);
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| AppError::Internal(format!("mkdir failed: {}", e)))?;

    let storage_path = figure_storage_path(&state.config.docs_path, metadata.figure_id);
    tokio::fs::write(&storage_path, &png)
        .await
        .map_err(|e| AppError::Internal(format!("figure write failed: {}", e)))?;

    // Idempotent insert: same figure_id + same doc means RunPod retried
    // the upload. Delete the old row so the new caption/bbox wins.
    sqlx::query!(
        "DELETE FROM document_figures WHERE id = $1",
        metadata.figure_id,
    )
    .execute(&state.db)
    .await?;

    minerva_db::queries::document_figures::insert(
        &state.db,
        minerva_db::queries::document_figures::NewFigure {
            id: metadata.figure_id,
            document_id: doc.id,
            page: metadata.page,
            t_start_seconds: metadata.t_start_seconds,
            t_end_seconds: metadata.t_end_seconds,
            bbox: metadata.bbox.as_ref(),
            caption: metadata.caption.as_deref(),
            storage_path: &storage_path,
        },
    )
    .await?;

    Ok(Json(
        serde_json::json!({ "figure_id": metadata.figure_id, "bytes": png.len() }),
    ))
}

// 405 is more useful than 404 here: the route exists, the method doesn't.
#[allow(dead_code)]
async fn method_not_allowed() -> StatusCode {
    StatusCode::METHOD_NOT_ALLOWED
}
