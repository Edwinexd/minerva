//! Service routes of the visual extraction pipeline, mounted at
//! `/api/service/visual-extraction`. See "Visual extraction pipeline" in
//! docs/ARCHITECTURE.md.
//!
//! Two kinds of caller:
//!
//! - **GitHub Actions** (service API key): asks for fetch slots, uploads a
//!   lecture's video and timed cues into the bounded staging window, or
//!   releases a slot it could not fill.
//! - **Slurm workers** (signed grants in the path, no other credential): ask
//!   for their next item, download its source, upload the result or report
//!   a failure, and say when they exit.

use std::collections::{HashMap, HashSet};

use axum::body::Body;
use axum::extract::{DefaultBodyLimit, Multipart, Path, Request, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use chrono::{Duration, Utc};
use futures::StreamExt;
use minerva_app_core::visual_extraction::{self as ve, Grant, Scope};
use minerva_db::queries::visual_extraction as queue;
use minerva_pipeline::figures::{self, Cue, Timed};
use qdrant_client::qdrant::{DeletePointsBuilder, PointsIdsList};
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;
use tower::ServiceExt;
use tower_http::services::ServeFile;
use uuid::Uuid;

use super::service::{authenticate_service, read_url_stub};
use crate::error::AppError;
use crate::state::AppState;

/// A lecture video upload. Matches Apache's default `LimitRequestBody`
/// (1 GiB) in front of the api; a three-hour 1080p lecture is about 470 MB.
const MAX_VIDEO_BYTES: u64 = 1024 * 1024 * 1024;
/// A result upload: OCR JSON plus slide frames and figure crops.
const MAX_RESULT_BYTES: usize = 512 * 1024 * 1024;
const MAX_FETCH_SLOTS: i64 = 50;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/fetch-slots", post(fetch_slots))
        .route(
            "/jobs/{job_id}/video",
            put(upload_video).layer(DefaultBodyLimit::disable()),
        )
        .route("/jobs/{job_id}/staged", post(mark_staged))
        .route("/jobs/{job_id}/release", post(release_fetch))
        .route("/workers/{grant}/next", post(next_item))
        .route("/workers/{grant}/exit", post(worker_exit))
        .route("/source/{grant}", get(download_source))
        .route(
            "/result/{grant}",
            put(upload_result).layer(DefaultBodyLimit::max(MAX_RESULT_BYTES)),
        )
        .route("/fail/{grant}", post(report_failure))
}

fn staged_video_path(state: &AppState, job_id: Uuid) -> String {
    format!(
        "{}/{job_id}.mp4",
        state.config.visual_extraction.staging_path
    )
}

fn verify_grant(state: &AppState, token: &str, scope: Scope) -> Result<Grant, AppError> {
    ve::verify(&state.config.hmac_secret, token, scope).map_err(|_| AppError::Unauthorized)
}

/// The job behind a source/result grant, provided that attempt still holds
/// the lease. Anything else (finished, requeued, re-leased) is stale.
async fn leased_job(state: &AppState, grant: &Grant) -> Result<queue::JobRow, AppError> {
    let job = queue::find_job(&state.db, grant.id)
        .await?
        .ok_or(AppError::NotFound)?;
    if job.status != "leased" || job.attempts != grant.attempt {
        return Err(AppError::bad_request("visual_extraction.stale_attempt"));
    }
    Ok(job)
}

// ── GitHub Actions: staging ────────────────────────────────────────────

#[derive(Deserialize)]
struct FetchSlotsBody {
    limit: Option<i64>,
}

#[derive(Serialize)]
struct FetchSlot {
    job_id: Uuid,
    document_id: Uuid,
    course_id: Uuid,
    url: String,
}

/// Reserve up to `limit` lectures to fetch, as many as the disk holds while
/// keeping the configured minimum free. Empty when it is full or nothing
/// needs a video.
async fn fetch_slots(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<FetchSlotsBody>,
) -> Result<Json<Vec<FetchSlot>>, AppError> {
    authenticate_service(&state, &headers)?;
    let limit = body.limit.unwrap_or(10).clamp(1, MAX_FETCH_SLOTS);
    let staging = &state.config.visual_extraction.staging_path;
    tokio::fs::create_dir_all(staging)
        .await
        .map_err(|e| AppError::Internal(format!("staging dir: {e}")))?;
    // No reading means no slots: better to stage nothing than to fill a
    // disk we cannot measure.
    let free_bytes = super::system::disk_usage(staging).map_or(0, |d| d.free_bytes as i64);
    let reserved = queue::reserve_fetch_slots(
        &state.db,
        free_bytes,
        state.config.visual_extraction.staging_min_free_bytes,
        ve::FETCH_ESTIMATE_BYTES,
        limit,
    )
    .await?;

    let mut slots = Vec::with_capacity(reserved.len());
    for slot in reserved {
        let doc = minerva_db::queries::documents::find_by_id(&state.db, slot.document_id).await?;
        let url = match doc {
            Some(doc) => read_url_stub(&state, doc.course_id, doc.id, &doc.filename).await,
            None => None,
        };
        match url {
            Some(url) => slots.push(FetchSlot {
                job_id: slot.job_id,
                document_id: slot.document_id,
                course_id: slot.course_id,
                url,
            }),
            None => {
                queue::release_fetch(&state.db, slot.job_id, "URL stub unreadable", false).await?;
            }
        }
    }
    Ok(Json(slots))
}

/// Stream a lecture's video into staging. Written under a temporary name
/// and renamed only once complete, so a dropped upload never looks staged.
async fn upload_video(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(job_id): Path<Uuid>,
    body: Body,
) -> Result<Json<serde_json::Value>, AppError> {
    authenticate_service(&state, &headers)?;
    let job = queue::find_job(&state.db, job_id)
        .await?
        .ok_or(AppError::NotFound)?;
    if job.status != "fetching" {
        return Err(AppError::bad_request("visual_extraction.not_fetching"));
    }

    let staging = &state.config.visual_extraction.staging_path;
    tokio::fs::create_dir_all(staging)
        .await
        .map_err(|e| AppError::Internal(format!("staging dir: {e}")))?;
    let final_path = staged_video_path(&state, job_id);
    let partial_path = format!("{final_path}.part");
    let mut file = tokio::fs::File::create(&partial_path)
        .await
        .map_err(|e| AppError::Internal(format!("staging write: {e}")))?;

    let mut written: u64 = 0;
    let mut stream = body.into_data_stream();
    while let Some(chunk) = stream.next().await {
        let outcome = match chunk {
            Ok(bytes) => {
                written += bytes.len() as u64;
                if written > MAX_VIDEO_BYTES {
                    Err(AppError::bad_request("visual_extraction.video_too_large"))
                } else {
                    file.write_all(&bytes)
                        .await
                        .map_err(|e| AppError::Internal(format!("staging write: {e}")))
                }
            }
            Err(e) => Err(AppError::Internal(format!("upload interrupted: {e}"))),
        };
        if let Err(e) = outcome {
            drop(file);
            let _ = tokio::fs::remove_file(&partial_path).await;
            return Err(e);
        }
    }
    file.flush()
        .await
        .map_err(|e| AppError::Internal(format!("staging write: {e}")))?;
    drop(file);
    tokio::fs::rename(&partial_path, &final_path)
        .await
        .map_err(|e| AppError::Internal(format!("staging rename: {e}")))?;
    Ok(Json(serde_json::json!({ "bytes": written })))
}

#[derive(Deserialize)]
struct StagedBody {
    cues: Vec<Cue>,
}

/// Finish staging: record the uploaded video's real size and the timed
/// cues, and make the lecture leasable.
async fn mark_staged(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(job_id): Path<Uuid>,
    Json(body): Json<StagedBody>,
) -> Result<Json<serde_json::Value>, AppError> {
    authenticate_service(&state, &headers)?;
    let path = staged_video_path(&state, job_id);
    let bytes = tokio::fs::metadata(&path)
        .await
        .map_err(|_| AppError::bad_request("visual_extraction.video_missing"))?
        .len() as i64;
    let cues: Vec<serde_json::Value> = body
        .cues
        .iter()
        .map(|c| serde_json::json!({ "start": c.start, "end": c.end, "text": c.text }))
        .collect();
    let staged = queue::mark_staged(
        &state.db,
        job_id,
        &format!("{job_id}.mp4"),
        bytes,
        &serde_json::Value::Array(cues),
    )
    .await?;
    if !staged {
        // A repeated call for a job that is already staged must keep its
        // video; only an upload no job points at (its reservation expired
        // or was released mid-upload) is litter.
        let job = queue::find_job(&state.db, job_id).await?;
        if job.and_then(|j| j.staged_path).is_none() {
            let _ = tokio::fs::remove_file(&path).await;
        }
        return Err(AppError::bad_request("visual_extraction.not_fetching"));
    }
    Ok(Json(
        serde_json::json!({ "status": "ready", "bytes": bytes }),
    ))
}

#[derive(Deserialize)]
struct ReleaseBody {
    error: String,
    /// True for a passing problem (Play error, transcript not generated
    /// yet); false parks the lecture as `unavailable`.
    retry: bool,
}

async fn release_fetch(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(job_id): Path<Uuid>,
    Json(body): Json<ReleaseBody>,
) -> Result<Json<serde_json::Value>, AppError> {
    authenticate_service(&state, &headers)?;
    let released = queue::release_fetch(&state.db, job_id, &body.error, body.retry).await?;
    // Only a reservation that was still fetching owns these files; a late
    // release for a job that has since been staged must not touch its video.
    if released {
        let path = staged_video_path(&state, job_id);
        let _ = tokio::fs::remove_file(format!("{path}.part")).await;
        let _ = tokio::fs::remove_file(&path).await;
    }
    Ok(Json(serde_json::json!({ "released": released })))
}

// ── Workers ────────────────────────────────────────────────────────────

#[derive(Serialize)]
struct WorkItem {
    job_id: Uuid,
    kind: String,
    attempt: i32,
    source_url: String,
    result_url: String,
    fail_url: String,
}

/// Lease the next item to a worker, with its download and upload URLs.
/// 204 when the queue is empty, which tells the worker to exit.
async fn next_item(
    State(state): State<AppState>,
    Path(token): Path<String>,
) -> Result<Response, AppError> {
    let grant = verify_grant(&state, &token, Scope::Worker)?;
    let worker = queue::touch_worker(&state.db, grant.id)
        .await?
        .ok_or(AppError::Unauthorized)?;
    let Some(job) = queue::lease_next(&state.db, worker.id, ve::LEASE_SECS).await? else {
        return Ok(StatusCode::NO_CONTENT.into_response());
    };

    let expires_at = Utc::now() + Duration::seconds(ve::LEASE_SECS);
    let base = format!("{}/api/service/visual-extraction", state.config.base_url);
    let url = |scope: Scope, route: &str| {
        let token = ve::sign(
            &state.config.hmac_secret,
            &Grant::new(scope, job.id, job.attempts, expires_at),
        );
        format!("{base}/{route}/{token}")
    };
    let item = WorkItem {
        job_id: job.id,
        kind: job.kind.clone(),
        attempt: job.attempts,
        source_url: url(Scope::Source, "source"),
        result_url: url(Scope::Result, "result"),
        fail_url: url(Scope::Result, "fail"),
    };
    Ok(Json(item).into_response())
}

#[derive(Deserialize)]
struct ExitBody {
    error: Option<String>,
}

/// A worker is exiting: cleanly when the queue ran dry or its wall time is
/// near, or with an error. Anything it still held goes back to the queue.
async fn worker_exit(
    State(state): State<AppState>,
    Path(token): Path<String>,
    Json(body): Json<ExitBody>,
) -> Result<Json<serde_json::Value>, AppError> {
    let grant = verify_grant(&state, &token, Scope::Worker)?;
    let reason = body.error.as_deref().unwrap_or("worker exited");
    let terminal = queue::requeue_worker(&state.db, grant.id, reason, ve::MAX_ATTEMPTS).await?;
    remove_staged(&state, &terminal).await;
    let worker_state = if body.error.is_some() {
        "failed"
    } else {
        "finished"
    };
    queue::set_worker_state(&state.db, grant.id, worker_state, body.error.as_deref()).await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

/// Serve the leased item's source: the staged video for a lecture, the
/// stored file for a PDF. Range requests work, so a worker can resume.
async fn download_source(
    State(state): State<AppState>,
    Path(token): Path<String>,
    request: Request,
) -> Result<Response, AppError> {
    let grant = verify_grant(&state, &token, Scope::Source)?;
    let job = leased_job(&state, &grant).await?;
    let path = match job.kind.as_str() {
        "play_lecture" => {
            let staged = job
                .staged_path
                .ok_or(AppError::bad_request("visual_extraction.video_missing"))?;
            format!("{}/{staged}", state.config.visual_extraction.staging_path)
        }
        _ => format!(
            "{}/{}/{}.pdf",
            state.config.docs_path, job.course_id, job.document_id
        ),
    };
    if tokio::fs::metadata(&path).await.is_err() {
        return Err(AppError::NotFound);
    }
    let response = ServeFile::new(path)
        .oneshot(request)
        .await
        .map_err(|e| AppError::Internal(format!("serve source: {e}")))?;
    Ok(response.map(Body::new))
}

#[derive(Deserialize)]
struct FailBody {
    error: String,
}

async fn report_failure(
    State(state): State<AppState>,
    Path(token): Path<String>,
    Json(body): Json<FailBody>,
) -> Result<Json<serde_json::Value>, AppError> {
    let grant = verify_grant(&state, &token, Scope::Result)?;
    let outcome = queue::fail_attempt(
        &state.db,
        grant.id,
        grant.attempt,
        &body.error,
        ve::MAX_ATTEMPTS,
    )
    .await?
    .ok_or(AppError::bad_request("visual_extraction.stale_attempt"))?;
    if let Some(staged) = outcome {
        remove_staged(&state, &[staged]).await;
    }
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn remove_staged(state: &AppState, staged_paths: &[String]) {
    for staged in staged_paths {
        let path = format!("{}/{staged}", state.config.visual_extraction.staging_path);
        let _ = tokio::fs::remove_file(path).await;
    }
}

// ── Figure images ──────────────────────────────────────────────────────

/// Serve a figure image shown with a chat reply. Mounted at
/// `/api/embed/figures/{grant}`, inside Apache's no-Shibboleth carve-out,
/// because the signed URL is the authorisation: it is only minted for
/// someone allowed to read the reply, and it works in the cookieless embed
/// iframe too.
pub(crate) async fn figure_image(
    State(state): State<AppState>,
    Path(token): Path<String>,
    request: Request,
) -> Result<Response, AppError> {
    let grant = verify_grant(&state, &token, Scope::Figure)?;
    let (course_id, document_id, image_path) = queue::figure_image(&state.db, grant.id)
        .await?
        .ok_or(AppError::NotFound)?;
    let path = format!(
        "{}/{image_path}",
        figures::assets_dir(&state.config.docs_path, course_id, document_id)
    );
    let response = ServeFile::new(path)
        .oneshot(request)
        .await
        .map_err(|e| AppError::Internal(format!("serve figure: {e}")))?;
    Ok(response.map(Body::new))
}

// ── Result ingest ──────────────────────────────────────────────────────

/// What a worker uploads as the `result` multipart field. Image paths name
/// the other multipart fields.
#[derive(Deserialize)]
struct ResultPayload {
    visual_model: String,
    pages: Vec<ResultPage>,
    figures: Vec<ResultFigure>,
}

#[derive(Deserialize)]
struct ResultPage {
    position: i32,
    page_number: Option<i32>,
    start: Option<f32>,
    end: Option<f32>,
    image: Option<String>,
    blocks: serde_json::Value,
    text: String,
}

impl Timed for ResultPage {
    fn start(&self) -> f32 {
        self.start.unwrap_or(0.0)
    }
}

#[derive(Deserialize)]
struct ResultFigure {
    page_position: i32,
    bbox: Vec<f32>,
    image: String,
    caption: Option<String>,
    context: String,
    visual_vector: Vec<f32>,
}

fn invalid(reason: impl Into<String>) -> AppError {
    AppError::bad_request_with(
        "visual_extraction.invalid_result",
        [("reason", reason.into())],
    )
}

/// Asset names a worker may upload: a flat file under `slides/` or
/// `figures/`, so nothing can escape the document's asset directory.
fn valid_asset_name(name: &str) -> bool {
    let Some((dir, file)) = name.split_once('/') else {
        return false;
    };
    matches!(dir, "slides" | "figures")
        && !file.is_empty()
        && !file.starts_with('.')
        && file
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
        && (file.ends_with(".jpg") || file.ends_with(".jpeg") || file.ends_with(".png"))
}

fn validate(payload: &ResultPayload, uploaded: &HashSet<String>) -> Result<(), AppError> {
    let positions: HashSet<i32> = payload.pages.iter().map(|p| p.position).collect();
    if positions.len() != payload.pages.len() {
        return Err(invalid("duplicate page position"));
    }
    if payload.visual_model != figures::VISUAL_MODEL {
        return Err(invalid(format!("visual model {}", payload.visual_model)));
    }
    for image in payload.pages.iter().filter_map(|p| p.image.as_ref()) {
        if !uploaded.contains(image) {
            return Err(invalid(format!("missing {image}")));
        }
    }
    for figure in &payload.figures {
        if !positions.contains(&figure.page_position) {
            return Err(invalid("figure on unknown page"));
        }
        if figure.bbox.len() != 4 {
            return Err(invalid("figure box needs four numbers"));
        }
        if figure.visual_vector.len() as u64 != figures::VISUAL_DIMENSIONS {
            return Err(invalid("visual vector has the wrong dimension"));
        }
        if !uploaded.contains(&figure.image) {
            return Err(invalid(format!("missing {}", figure.image)));
        }
    }
    Ok(())
}

/// Accept a worker's result: store its slide frames and figure crops,
/// replace the document's pages and figures, and for a lecture swap in the
/// slides-plus-speech text child. All database changes land in one
/// transaction together with the job's `done`; a stale attempt changes
/// nothing.
async fn upload_result(
    State(state): State<AppState>,
    Path(token): Path<String>,
    mut multipart: Multipart,
) -> Result<Json<serde_json::Value>, AppError> {
    let grant = verify_grant(&state, &token, Scope::Result)?;
    let job = leased_job(&state, &grant).await?;
    let doc = minerva_db::queries::documents::find_by_id(&state.db, job.document_id)
        .await?
        .ok_or(AppError::NotFound)?;

    // Each accepted result gets its own directory, so the previous one keeps
    // serving until this commit and is removed after.
    let assets_root = figures::assets_dir(&state.config.docs_path, job.course_id, job.document_id);
    let version_dir = format!(
        "a{}-{}",
        grant.attempt,
        &Uuid::new_v4().simple().to_string()[..8]
    );
    let version_root = format!("{assets_root}/{version_dir}");

    let stored = receive_result(&mut multipart, &version_root).await;
    let (payload, uploaded) = match stored {
        Ok(stored) => stored,
        Err(e) => {
            let _ = tokio::fs::remove_dir_all(&version_root).await;
            return Err(e);
        }
    };
    if let Err(e) = validate(&payload, &uploaded) {
        let _ = tokio::fs::remove_dir_all(&version_root).await;
        return Err(e);
    }

    match ingest(&state, &job, &doc, &grant, &payload, &version_dir).await {
        Ok(response) => {
            // The new result is live: drop the previous asset directories.
            if let Ok(mut entries) = tokio::fs::read_dir(&assets_root).await {
                while let Ok(Some(entry)) = entries.next_entry().await {
                    if entry.file_name().to_string_lossy() != version_dir {
                        let _ = tokio::fs::remove_dir_all(entry.path()).await;
                    }
                }
            }
            Ok(Json(response))
        }
        Err(e) => {
            let _ = tokio::fs::remove_dir_all(&version_root).await;
            Err(e)
        }
    }
}

async fn receive_result(
    multipart: &mut Multipart,
    version_root: &str,
) -> Result<(ResultPayload, HashSet<String>), AppError> {
    let mut payload = None;
    let mut uploaded = HashSet::new();
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| invalid(format!("multipart: {e}")))?
    {
        let name = field.name().unwrap_or_default().to_string();
        let bytes = field
            .bytes()
            .await
            .map_err(|e| invalid(format!("multipart: {e}")))?;
        if name == "result" {
            payload = Some(
                serde_json::from_slice::<ResultPayload>(&bytes)
                    .map_err(|e| invalid(format!("result json: {e}")))?,
            );
            continue;
        }
        if !valid_asset_name(&name) {
            return Err(invalid(format!("asset name {name}")));
        }
        let path = format!("{version_root}/{name}");
        if let Some(parent) = std::path::Path::new(&path).parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| AppError::Internal(format!("asset dir: {e}")))?;
        }
        tokio::fs::write(&path, &bytes)
            .await
            .map_err(|e| AppError::Internal(format!("asset write: {e}")))?;
        uploaded.insert(name);
    }
    let payload = payload.ok_or_else(|| invalid("no result field"))?;
    Ok((payload, uploaded))
}

async fn ingest(
    state: &AppState,
    job: &queue::JobRow,
    doc: &minerva_db::queries::documents::DocumentRow,
    grant: &Grant,
    payload: &ResultPayload,
    version_dir: &str,
) -> Result<serde_json::Value, AppError> {
    let is_lecture = job.kind == "play_lecture";
    let mut pages: Vec<&ResultPage> = payload.pages.iter().collect();
    pages.sort_by_key(|p| p.position);

    // Speech per slide, aligned at ingest from the cues staged with the
    // video, so the worker never needs the transcript.
    let cues: Vec<Cue> = match (&job.cues, is_lecture) {
        (Some(cues), true) => serde_json::from_value(cues.clone())
            .map_err(|e| AppError::Internal(format!("stored cues: {e}")))?,
        _ => Vec::new(),
    };
    let spoken: Vec<String> = if is_lecture {
        let mut by_start = pages.clone();
        by_start.sort_by(|a, b| a.start().total_cmp(&b.start()));
        let aligned = figures::align_cues(&by_start, &cues);
        let by_position: HashMap<i32, String> = by_start
            .iter()
            .zip(aligned.iter())
            .map(|(page, cues)| (page.position, figures::spoken_text(cues)))
            .collect();
        pages
            .iter()
            .map(|p| by_position.get(&p.position).cloned().unwrap_or_default())
            .collect()
    } else {
        vec![String::new(); pages.len()]
    };
    let spoken_by_position: HashMap<i32, &str> = pages
        .iter()
        .zip(spoken.iter())
        .map(|(p, s)| (p.position, s.as_str()))
        .collect();

    let asset_path = |name: &str| format!("{version_dir}/{name}");
    let page_images: Vec<Option<String>> = pages
        .iter()
        .map(|p| p.image.as_deref().map(asset_path))
        .collect();
    let new_pages: Vec<queue::NewPage> = pages
        .iter()
        .zip(page_images.iter())
        .map(|(p, image)| queue::NewPage {
            position: p.position,
            page_number: p.page_number,
            start_seconds: p.start,
            end_seconds: p.end,
            image_path: image.as_deref(),
            blocks: &p.blocks,
            text: &p.text,
        })
        .collect();

    let figure_paths: Vec<String> = payload
        .figures
        .iter()
        .map(|f| asset_path(&f.image))
        .collect();
    let figure_contexts: Vec<String> = payload
        .figures
        .iter()
        .map(|f| match spoken_by_position.get(&f.page_position) {
            Some(spoken) if !spoken.is_empty() => format!("{}\n\nSpoken: {spoken}", f.context),
            _ => f.context.clone(),
        })
        .collect();
    let new_figures: Vec<queue::NewFigure> = payload
        .figures
        .iter()
        .zip(figure_paths.iter().zip(figure_contexts.iter()))
        .map(|(f, (path, context))| queue::NewFigure {
            page_position: f.page_position,
            bbox: &f.bbox,
            image_path: path,
            caption: f.caption.as_deref(),
            context,
            visual_model: &payload.visual_model,
            visual_vector: &f.visual_vector,
        })
        .collect();

    // A lecture's slides-plus-speech document, written before the
    // transaction so the insert can reference real bytes.
    let child = if is_lecture {
        let mut text = String::new();
        for (page, spoken) in pages.iter().zip(spoken.iter()) {
            let label = match page.page_number {
                Some(n) => format!("Slide {n}"),
                None => format!("Slide {}", page.position + 1),
            };
            text.push_str(&figures::lecture_section(
                &label,
                page.start.unwrap_or(0.0),
                page.end.unwrap_or(0.0),
                &page.text,
                spoken,
            ));
        }
        let child_id = Uuid::new_v4();
        let path = format!(
            "{}/{}/{child_id}.txt",
            state.config.docs_path, job.course_id
        );
        tokio::fs::write(&path, text.as_bytes())
            .await
            .map_err(|e| AppError::Internal(format!("lecture text write: {e}")))?;
        Some((child_id, path, text))
    } else {
        None
    };

    let committed = commit_ingest(
        state,
        job,
        doc,
        grant,
        &new_pages,
        &new_figures,
        child.as_ref(),
    )
    .await;
    let (replaced_children, replaced_figures) = match committed {
        Ok(replaced) => replaced,
        Err(e) => {
            if let Some((_, path, _)) = &child {
                let _ = tokio::fs::remove_file(path).await;
            }
            return Err(e);
        }
    };

    // After commit: the previous text child and figure points are gone from
    // the database, so remove their vectors and files too.
    let embedding_version = sqlx::query_scalar!(
        "SELECT embedding_version FROM courses WHERE id = $1",
        job.course_id
    )
    .fetch_one(&state.db)
    .await?;
    if !replaced_children.is_empty() {
        super::documents::purge_document_artifacts(
            state,
            job.course_id,
            embedding_version,
            &replaced_children,
        )
        .await?;
        for id in &replaced_children {
            minerva_db::queries::documents::delete(&state.db, *id).await?;
        }
        super::documents::remove_document_files(state, job.course_id, &replaced_children).await;
    }
    delete_figure_points(state, job.course_id, embedding_version, &replaced_figures).await?;
    // A PDF's chunks came from text extraction so it was searchable at
    // once; now that its OCR text exists, re-chunk from that instead.
    if !is_lecture && new_pages.iter().any(|p| !p.text.trim().is_empty()) {
        queue::requeue_for_reingest(&state.db, job.document_id).await?;
    }
    if let Some(staged) = &job.staged_path {
        remove_staged(state, std::slice::from_ref(staged)).await;
    }

    Ok(serde_json::json!({
        "status": "accepted",
        "pages": new_pages.len(),
        "figures": new_figures.len(),
        "child_id": child.map(|(id, _, _)| id),
    }))
}

/// The ingest transaction. Returns the replaced text children and figure
/// ids for cleanup after commit.
async fn commit_ingest(
    state: &AppState,
    job: &queue::JobRow,
    doc: &minerva_db::queries::documents::DocumentRow,
    grant: &Grant,
    pages: &[queue::NewPage<'_>],
    figures_new: &[queue::NewFigure<'_>],
    child: Option<&(Uuid, String, String)>,
) -> Result<(Vec<Uuid>, Vec<Uuid>), AppError> {
    let mut tx = state.db.begin().await?;
    let mut replaced_children = Vec::new();
    if let Some((child_id, _, text)) = child {
        replaced_children =
            minerva_db::queries::documents::orphan_active_children_in(&mut tx, doc.id).await?;
        let filename = format!("{}.txt", doc.filename.trim_end_matches(".url"));
        let hash = minerva_pipeline::pipeline::compute_content_hash(text.as_bytes());
        // `failed` too: a stub whose transcript fetch failed still gets the
        // slides, with the cues staged alongside its video.
        minerva_db::queries::documents::insert_tracked_child_in(
            &mut tx,
            doc.id,
            &["awaiting_transcript", "tracked", "failed"],
            minerva_db::queries::documents::NewDocument {
                id: *child_id,
                course_id: doc.course_id,
                filename: &filename,
                mime_type: "text/plain",
                size_bytes: text.len() as i64,
                uploaded_by: doc.uploaded_by,
                // The parent keeps the URL; the unique stub index would
                // otherwise collide.
                source_url: None,
                content_hash: Some(&hash),
                source_system: None,
                source_ref: None,
                parent_document_id: Some(doc.id),
            },
        )
        .await
        .map_err(|e| match e {
            sqlx::Error::RowNotFound => {
                AppError::bad_request("visual_extraction.parent_unavailable")
            }
            other => other.into(),
        })?;
    }
    let replaced_figures =
        queue::replace_layout_in(&mut tx, job.document_id, job.course_id, pages, figures_new)
            .await?;
    if !queue::complete(&mut tx, job.id, grant.attempt).await? {
        return Err(AppError::bad_request("visual_extraction.stale_attempt"));
    }
    tx.commit().await?;
    Ok((replaced_children, replaced_figures))
}

async fn delete_figure_points(
    state: &AppState,
    course_id: Uuid,
    embedding_version: i32,
    figure_ids: &[Uuid],
) -> Result<(), AppError> {
    if figure_ids.is_empty() {
        return Ok(());
    }
    let ids = PointsIdsList {
        ids: figure_ids.iter().map(|id| id.to_string().into()).collect(),
    };
    for collection in [
        figures::COLLECTION.to_string(),
        figures::context_collection_name(course_id, embedding_version),
    ] {
        if !state
            .qdrant
            .collection_exists(&collection)
            .await
            .unwrap_or(false)
        {
            continue;
        }
        state
            .qdrant
            .delete_points(
                DeletePointsBuilder::new(&collection)
                    .points(ids.clone())
                    .wait(true),
            )
            .await
            .map_err(|e| AppError::Internal(format!("qdrant delete failed: {e}")))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asset_names_stay_inside_their_directory() {
        assert!(valid_asset_name("slides/abc123.jpg"));
        assert!(valid_asset_name("figures/d9d1653d518d.fig0.jpg"));
        assert!(!valid_asset_name("../etc/passwd"));
        assert!(!valid_asset_name("slides/../x.jpg"));
        assert!(!valid_asset_name("slides/sub/x.jpg"));
        assert!(!valid_asset_name("other/x.jpg"));
        assert!(!valid_asset_name("slides/.hidden.jpg"));
        assert!(!valid_asset_name("slides/x.svg"));
        assert!(!valid_asset_name("x.jpg"));
    }
}
