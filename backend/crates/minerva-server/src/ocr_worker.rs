//! OCR + video-indexing pipeline: idempotent submission to RunPod, poll
//! loop, orphan reconciliation, and output application.
//!
//! Three background tasks share the same RunPod-aware machinery and run
//! sequentially via the `start` entrypoint:
//!
//!   * `submit_loop`: scans `awaiting_ocr` and `awaiting_video_index`
//!     docs, submits one RunPod job per doc with a `client_request_id`
//!     idempotency key. Pre-writes a `runpod_jobs` row in 'submitting'
//!     state before the network call so a crash between submit and PATCH
//!     never leaks a billed job we can't reconcile.
//!
//!   * `poll_loop`: every `POLL_INTERVAL`, asks RunPod for the status of
//!     each in-flight job. On COMPLETED, applies the output (writes
//!     markdown to disk, transitions doc state, the existing chunker
//!     takes over). On FAILED, increments retry; dead-letters at
//!     `MAX_RETRIES`.
//!
//!   * `reconcile_loop`: periodic sweep over rows stuck in 'submitting'
//!     for longer than the reconciliation window. Lists recent RunPod
//!     jobs and matches by `client_request_id` to recover any in-flight
//!     work. Rows with no match (network blip BEFORE RunPod received
//!     the request) are flipped to 'failed' so the submit_loop schedules
//!     a fresh attempt.
//!
//! All three are gated by `state.config.ocr_pipeline_enabled` so the
//! whole subsystem is dormant until the flag flips. RunPod credentials
//! missing while the flag is on logs a clear error and skips submissions
//! (poll/reconcile still run; they're cheap and surface configuration
//! issues before docs accumulate in awaiting_*).

use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;
use uuid::Uuid;

use crate::runpod::{self, RunpodConfig, RunpodError};
use crate::state::AppState;

/// How often to look for new docs to submit. Short because the GH
/// ingest worker pushes hourly bursts; a long delay between bundle
/// upload and RunPod submission would inflate end-to-end ingest time.
const SUBMIT_INTERVAL: Duration = Duration::from_secs(30);

/// How often to poll RunPod for in-flight job status.
const POLL_INTERVAL: Duration = Duration::from_secs(30);

/// How often to reconcile orphaned 'submitting' rows.
const RECONCILE_INTERVAL: Duration = Duration::from_secs(120);

/// A row is considered orphaned (worker crashed between insert and PATCH)
/// after this many seconds in 'submitting'. Comfortably exceeds normal
/// network latency so a slow submit doesn't get clobbered.
const RECONCILE_AGE_SECS: i64 = 300;

/// Per-doc retry cap. After this many failures, the doc is dead-lettered
/// and an admin re-OCR is required.
const MAX_RETRIES: i32 = 3;

/// How many docs to submit per submit-loop tick. Ceiling on RunPod queue
/// growth from a single backlog burst; matches the subagent's note about
/// per-course concurrency caps without splitting per course (one course
/// dumping 100 lectures still gets throttled).
const SUBMIT_BATCH_LIMIT: i32 = 5;

/// Public entry point; mirrors `worker::start`. Spawned from `main` after
/// `state` is built. No-op if the flag is off (tasks still spawn but
/// short-circuit on first tick), so toggling the flag at runtime via a
/// pod restart is the only ceremony.
pub fn start(state: AppState) {
    let s = state.clone();
    tokio::spawn(async move {
        if !s.config.ocr_pipeline_enabled {
            tracing::info!("ocr_worker: pipeline disabled, submit loop idle");
            return;
        }
        loop {
            if let Err(e) = run_submit_tick(&s).await {
                tracing::error!("ocr_worker: submit tick failed: {}", e);
            }
            tokio::time::sleep(SUBMIT_INTERVAL).await;
        }
    });

    let s = state.clone();
    tokio::spawn(async move {
        if !s.config.ocr_pipeline_enabled {
            return;
        }
        loop {
            tokio::time::sleep(POLL_INTERVAL).await;
            if let Err(e) = run_poll_tick(&s).await {
                tracing::error!("ocr_worker: poll tick failed: {}", e);
            }
        }
    });

    let s = state;
    tokio::spawn(async move {
        if !s.config.ocr_pipeline_enabled {
            return;
        }
        loop {
            tokio::time::sleep(RECONCILE_INTERVAL).await;
            if let Err(e) = run_reconcile_tick(&s).await {
                tracing::error!("ocr_worker: reconcile tick failed: {}", e);
            }
        }
    });
}

async fn run_submit_tick(state: &AppState) -> anyhow::Result<()> {
    // Daily budget circuit breaker. Pause submissions if we've crossed
    // the spend cap; rows stay in awaiting_* and resume next day.
    if let Some(reason) = circuit_breaker_reason(state).await? {
        tracing::warn!("ocr_worker: submission paused: {}", reason);
        return Ok(());
    }

    let cfg = match RunpodConfig::from_app(&state.config) {
        Ok(c) => Arc::new(c),
        Err(_) => {
            tracing::error!(
                "ocr_worker: RUNPOD_API_KEY/RUNPOD_ENDPOINT_ID not set; submission disabled while \
                 OCR pipeline flag is on"
            );
            return Ok(());
        }
    };

    let pdf_image =
        minerva_db::queries::documents::list_awaiting_ocr(&state.db, SUBMIT_BATCH_LIMIT).await?;
    for doc in pdf_image {
        if let Err(e) = submit_ocr(state, &cfg, &doc).await {
            tracing::error!("ocr_worker: submit_ocr({}) failed: {:?}", doc.id, e);
        }
    }

    let videos =
        minerva_db::queries::documents::list_awaiting_video_index(&state.db, SUBMIT_BATCH_LIMIT)
            .await?;
    for doc in videos {
        if let Err(e) = submit_video_index(state, &cfg, &doc).await {
            tracing::error!("ocr_worker: submit_video_index({}) failed: {:?}", doc.id, e);
        }
    }

    Ok(())
}

#[derive(Serialize)]
struct OcrPdfInput<'a> {
    task: &'a str,
    client_request_id: &'a str,
    document_id: Uuid,
    source_url: String,
    /// Where RunPod should POST figure crops + metadata.
    figure_upload_url: String,
    service_api_base: &'a str,
}

#[derive(Serialize)]
struct VideoIndexInput<'a> {
    task: &'a str,
    client_request_id: &'a str,
    document_id: Uuid,
    bundle_url: String,
    figure_upload_url: String,
    service_api_base: &'a str,
    sample_fps: &'a str,
    /// Inlined here so the handler doesn't have to fetch it separately;
    /// the bundle's manifest also references it for redundancy.
    vtt_text: String,
}

async fn submit_ocr(
    state: &AppState,
    cfg: &RunpodConfig,
    doc: &minerva_db::queries::documents::DocumentRow,
) -> anyhow::Result<()> {
    if minerva_db::queries::runpod_jobs::has_in_flight_for_document(&state.db, doc.id).await? {
        return Ok(());
    }

    // Pick task by mime; the handler dispatches identically but we keep
    // the explicit name in `runpod_jobs.task` so the admin UI doesn't
    // have to inspect inputs to tell PDFs from images.
    let task = if doc.mime_type.starts_with("image/") {
        "ocr_image"
    } else {
        "ocr_pdf"
    };

    let client_request_id = format!("doc-{}-{}", doc.id, Uuid::new_v4());
    let _row = minerva_db::queries::runpod_jobs::insert_submitting(
        &state.db,
        &client_request_id,
        task,
        doc.id,
    )
    .await?;

    let base = state.config.runpod_callback_base.trim_end_matches('/');
    let input = OcrPdfInput {
        task,
        client_request_id: &client_request_id,
        document_id: doc.id,
        source_url: format!("{}/api/service/documents/{}/source", base, doc.id),
        figure_upload_url: format!("{}/api/service/figure-uploads/{}", base, doc.id),
        service_api_base: base,
    };
    let input_value = serde_json::to_value(&input)?;

    submit_then_patch(
        state,
        cfg,
        &client_request_id,
        doc.id,
        &input_value,
        "processing_ocr",
    )
    .await
}

async fn submit_video_index(
    state: &AppState,
    cfg: &RunpodConfig,
    doc: &minerva_db::queries::documents::DocumentRow,
) -> anyhow::Result<()> {
    if minerva_db::queries::runpod_jobs::has_in_flight_for_document(&state.db, doc.id).await? {
        return Ok(());
    }

    let client_request_id = format!("doc-{}-{}", doc.id, Uuid::new_v4());
    let _row = minerva_db::queries::runpod_jobs::insert_submitting(
        &state.db,
        &client_request_id,
        "video_index",
        doc.id,
    )
    .await?;

    let base = state.config.runpod_callback_base.trim_end_matches('/');
    let vtt_path = format!(
        "{}/{}/{}.vtt",
        state.config.docs_path, doc.course_id, doc.id
    );
    let vtt_text = tokio::fs::read_to_string(&vtt_path)
        .await
        .unwrap_or_default();

    let input = VideoIndexInput {
        task: "video_index",
        client_request_id: &client_request_id,
        document_id: doc.id,
        bundle_url: format!("{}/api/service/documents/{}/video-bundle", base, doc.id),
        figure_upload_url: format!("{}/api/service/figure-uploads/{}", base, doc.id),
        service_api_base: base,
        sample_fps: &state.config.video_sample_fps,
        vtt_text,
    };
    let input_value = serde_json::to_value(&input)?;

    submit_then_patch(
        state,
        cfg,
        &client_request_id,
        doc.id,
        &input_value,
        "processing_video_index",
    )
    .await
}

async fn submit_then_patch(
    state: &AppState,
    cfg: &RunpodConfig,
    client_request_id: &str,
    doc_id: Uuid,
    input: &serde_json::Value,
    next_doc_status: &str,
) -> anyhow::Result<()> {
    let resp = match runpod::submit(&state.http_client, cfg, input).await {
        Ok(r) => r,
        Err(e) => {
            // Submission never completed cleanly. Mark the row failed so
            // the next submit tick treats the doc as eligible again.
            // Reconciler will catch the case where RunPod actually
            // received it (different from the network-blip case).
            let _ = minerva_db::queries::runpod_jobs::mark_failed_by_client_id(
                &state.db,
                client_request_id,
                &format!("submit_failed: {}", e),
            )
            .await;
            return Err(anyhow::anyhow!(e));
        }
    };

    minerva_db::queries::runpod_jobs::mark_in_queue(&state.db, client_request_id, &resp.id).await?;
    let transitioned = if next_doc_status == "processing_ocr" {
        minerva_db::queries::documents::mark_processing_ocr(&state.db, doc_id).await?
    } else {
        minerva_db::queries::documents::mark_processing_video_index(&state.db, doc_id).await?
    };

    tracing::info!(
        "ocr_worker: submitted RunPod job {} for doc {} ({}); doc status {}",
        resp.id,
        doc_id,
        client_request_id,
        if transitioned {
            next_doc_status
        } else {
            "unchanged (concurrent state change)"
        },
    );
    Ok(())
}

async fn run_poll_tick(state: &AppState) -> anyhow::Result<()> {
    let cfg = match RunpodConfig::from_app(&state.config) {
        Ok(c) => c,
        Err(_) => return Ok(()),
    };

    let in_flight = minerva_db::queries::runpod_jobs::list_in_flight(&state.db).await?;
    for job in in_flight {
        let runpod_id = match &job.runpod_job_id {
            Some(id) => id.clone(),
            None => continue, // reconciler handles these
        };
        let status = match runpod::get_status(&state.http_client, &cfg, &runpod_id).await {
            Ok(s) => s,
            Err(RunpodError::Status { status, body }) => {
                tracing::warn!(
                    "ocr_worker: poll {} returned {}: {}",
                    runpod_id,
                    status,
                    body
                );
                continue;
            }
            Err(e) => {
                tracing::warn!("ocr_worker: poll {} failed: {}", runpod_id, e);
                continue;
            }
        };

        match status.status.as_str() {
            "COMPLETED" => {
                if let Err(e) = apply_completion(state, &job, &status).await {
                    tracing::error!("ocr_worker: apply_completion({}) failed: {}", runpod_id, e);
                }
            }
            "FAILED" | "CANCELLED" | "TIMED_OUT" => {
                let err_text = status
                    .error
                    .as_ref()
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| status.status.clone());
                let retries =
                    minerva_db::queries::runpod_jobs::mark_failed(&state.db, &runpod_id, &err_text)
                        .await?;
                if retries >= MAX_RETRIES {
                    let dead = match job.task.as_str() {
                        "video_index" => "video_index_failed",
                        _ => "ocr_failed",
                    };
                    minerva_db::queries::documents::mark_ocr_failed(
                        &state.db,
                        job.document_id,
                        dead,
                        &err_text,
                    )
                    .await?;
                    tracing::error!(
                        "ocr_worker: doc {} dead-lettered after {} attempts: {}",
                        job.document_id,
                        retries,
                        err_text
                    );
                } else {
                    // Walk the doc back to awaiting_* so submit_loop picks
                    // it up again. The has_in_flight_for_document guard
                    // will see no active row and let the resubmit through.
                    let revert_to = match job.task.as_str() {
                        "video_index" => "awaiting_video_index",
                        _ => "awaiting_ocr",
                    };
                    sqlx::query!(
                        "UPDATE documents SET status = $1 WHERE id = $2",
                        revert_to,
                        job.document_id,
                    )
                    .execute(&state.db)
                    .await?;
                    tracing::warn!(
                        "ocr_worker: doc {} retry {}/{}: {}",
                        job.document_id,
                        retries,
                        MAX_RETRIES,
                        err_text
                    );
                }
            }
            "IN_QUEUE" | "IN_PROGRESS" => {
                let _ = minerva_db::queries::runpod_jobs::update_runtime_status(
                    &state.db,
                    &runpod_id,
                    &status.status.to_lowercase(),
                )
                .await;
            }
            other => {
                tracing::warn!(
                    "ocr_worker: unexpected RunPod status for {}: {}",
                    runpod_id,
                    other
                );
            }
        }
    }
    Ok(())
}

/// Markdown body produced by the `ocr_pdf` / `ocr_image` tasks.
#[derive(serde::Deserialize)]
struct OcrOutput {
    /// One page per element for `ocr_pdf`; single-element array for
    /// `ocr_image`. Each entry's markdown is concatenated with a page
    /// separator before being written to disk.
    #[serde(default)]
    pages: Vec<OcrPage>,
}

#[derive(serde::Deserialize)]
struct OcrPage {
    markdown: String,
}

#[derive(serde::Deserialize)]
struct VideoIndexOutput {
    /// Already-deduped, VTT-fused timeline spans.
    #[serde(default)]
    timeline: Vec<TimelineSpan>,
}

#[derive(serde::Deserialize)]
struct TimelineSpan {
    t_start: f32,
    t_end: f32,
    markdown: String,
    #[serde(default)]
    vtt_text: Option<String>,
}

async fn apply_completion(
    state: &AppState,
    job: &minerva_db::queries::runpod_jobs::RunpodJobRow,
    status: &runpod::StatusResponse,
) -> anyhow::Result<()> {
    let output = match &status.output {
        Some(v) => v.clone(),
        None => {
            return Err(anyhow::anyhow!("COMPLETED but output payload missing"));
        }
    };

    let doc = minerva_db::queries::documents::find_by_id(&state.db, job.document_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("doc {} vanished", job.document_id))?;

    let dir = format!("{}/{}", state.config.docs_path, doc.course_id);
    tokio::fs::create_dir_all(&dir).await?;
    let md_path = format!("{}/{}.md", dir, doc.id);

    let body = match job.task.as_str() {
        "ocr_pdf" | "ocr_image" => render_ocr_markdown(&output)?,
        "video_index" => render_video_index_markdown(&output)?,
        other => return Err(anyhow::anyhow!("unknown task type: {}", other)),
    };
    tokio::fs::write(&md_path, body.as_bytes()).await?;

    // Filename rewrites: <stem>.md so the existing chunker picks it up
    // through the `md` extension branch on the next worker poll.
    let stem = doc
        .filename
        .rsplit_once('.')
        .map(|(s, _)| s)
        .unwrap_or(&doc.filename)
        .to_string();
    let new_filename = format!("{}.md", stem);
    let size_bytes = body.len() as i64;
    let gpu_seconds = status.gpu_seconds().unwrap_or(0.0);
    let cost_estimate = gpu_seconds as f64 * state.config.runpod_per_second_usd;

    minerva_db::queries::documents::replace_with_ocr_output(
        &state.db,
        doc.id,
        &new_filename,
        "text/markdown",
        size_bytes,
        "high",
        gpu_seconds,
    )
    .await?;

    minerva_db::queries::runpod_jobs::mark_completed(
        &state.db,
        status.id.as_str(),
        &output,
        Some(gpu_seconds),
        Some(cost_estimate as f32),
    )
    .await?;

    tracing::info!(
        "ocr_worker: applied completion for doc {} ({} bytes, {} gpu_s, ~${:.4})",
        doc.id,
        size_bytes,
        gpu_seconds,
        cost_estimate
    );
    Ok(())
}

fn render_ocr_markdown(output: &serde_json::Value) -> anyhow::Result<String> {
    let parsed: OcrOutput = serde_json::from_value(output.clone())?;
    if parsed.pages.is_empty() {
        return Err(anyhow::anyhow!("ocr output has no pages"));
    }
    let mut buf = String::new();
    for (i, page) in parsed.pages.iter().enumerate() {
        if i > 0 {
            buf.push_str("\n\n---\n\n");
        }
        buf.push_str(&format!("## Page {}\n\n", i + 1));
        buf.push_str(page.markdown.trim());
        buf.push('\n');
    }
    Ok(buf)
}

fn render_video_index_markdown(output: &serde_json::Value) -> anyhow::Result<String> {
    let parsed: VideoIndexOutput = serde_json::from_value(output.clone())?;
    if parsed.timeline.is_empty() {
        return Err(anyhow::anyhow!("video_index output has no timeline spans"));
    }
    let mut buf = String::new();
    for (i, span) in parsed.timeline.iter().enumerate() {
        if i > 0 {
            buf.push_str("\n\n---\n\n");
        }
        let start = format_timestamp(span.t_start);
        let end = format_timestamp(span.t_end);
        buf.push_str(&format!("## Slide [{} - {}]\n\n", start, end));
        buf.push_str(span.markdown.trim());
        if let Some(vtt) = &span.vtt_text {
            if !vtt.trim().is_empty() {
                buf.push_str("\n\n### Spoken during this slide\n\n");
                buf.push_str(vtt.trim());
            }
        }
        buf.push('\n');
    }
    Ok(buf)
}

fn format_timestamp(secs: f32) -> String {
    let total = secs.max(0.0) as u64;
    format!("{:02}:{:02}", total / 60, total % 60)
}

async fn run_reconcile_tick(state: &AppState) -> anyhow::Result<()> {
    let cfg = match RunpodConfig::from_app(&state.config) {
        Ok(c) => c,
        Err(_) => return Ok(()),
    };

    let orphans =
        minerva_db::queries::runpod_jobs::list_orphaned_submissions(&state.db, RECONCILE_AGE_SECS)
            .await?;
    if orphans.is_empty() {
        return Ok(());
    }

    tracing::info!(
        "ocr_worker: reconciling {} orphaned submission(s)",
        orphans.len()
    );

    for orphan in orphans {
        match runpod::find_by_client_id(&state.http_client, &cfg, &orphan.client_request_id).await {
            Ok(Some(found)) => {
                minerva_db::queries::runpod_jobs::mark_in_queue(
                    &state.db,
                    &orphan.client_request_id,
                    &found.id,
                )
                .await?;
                tracing::info!(
                    "ocr_worker: recovered orphan {} -> RunPod job {}",
                    orphan.client_request_id,
                    found.id
                );
            }
            Ok(None) => {
                // Submission never reached RunPod. Walk the doc back so
                // the next submit tick re-attempts it from a clean slate.
                minerva_db::queries::runpod_jobs::mark_failed_by_client_id(
                    &state.db,
                    &orphan.client_request_id,
                    "submit_orphaned: not found at RunPod after reconcile window",
                )
                .await?;
                let revert_to = match orphan.task.as_str() {
                    "video_index" => "awaiting_video_index",
                    _ => "awaiting_ocr",
                };
                sqlx::query!(
                    "UPDATE documents SET status = $1
                     WHERE id = $2 AND status IN ('processing_ocr', 'processing_video_index')",
                    revert_to,
                    orphan.document_id,
                )
                .execute(&state.db)
                .await?;
                tracing::warn!(
                    "ocr_worker: orphan {} discarded; doc {} -> {}",
                    orphan.client_request_id,
                    orphan.document_id,
                    revert_to
                );
            }
            Err(e) => {
                tracing::warn!(
                    "ocr_worker: reconcile lookup for {} failed: {}",
                    orphan.client_request_id,
                    e
                );
            }
        }
    }
    Ok(())
}

/// Pause submissions when today's RunPod spend exceeds the configured
/// daily budget. Returns Some("reason") to skip a submit tick;
/// None to proceed.
///
/// `runpod_daily_budget_usd = 0` disables the cap (useful for dev).
async fn circuit_breaker_reason(state: &AppState) -> anyhow::Result<Option<String>> {
    let budget = state.config.runpod_daily_budget_usd;
    if budget <= 0.0 {
        return Ok(None);
    }
    let spent = minerva_db::queries::runpod_jobs::estimated_cost_last_24h(&state.db).await?;
    if spent >= budget {
        Ok(Some(format!(
            "daily RunPod budget reached: spent ~${:.2} of ${:.2}",
            spent, budget
        )))
    } else {
        Ok(None)
    }
}
