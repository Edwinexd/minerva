//! Background document-processing worker.
//!
//! Instead of spawning an unbounded `tokio::spawn` per upload, documents are
//! inserted as `pending` and this worker polls the `documents` table using
//! `SELECT ... FOR UPDATE SKIP LOCKED`; a standard Postgres job-queue pattern.
//!
//! Concurrency is bounded by a semaphore so we never overwhelm the embedding
//! API, Qdrant, or server memory when a teacher syncs a large course at once.
//!
//! Stuck-doc recovery has two tiers:
//! * Startup: `reset_stale_processing` unconditionally resets anything left
//!   in `processing`; covers crashes/OOMs that skipped graceful shutdown.
//! * Periodic sweep: `reset_stale_processing_older_than(STALE_THRESHOLD_SECS)`
//!   handles docs wedged by a silent task panic inside a still-running pod.

use std::sync::Arc;
use tokio::sync::Semaphore;

use async_trait::async_trait;
use minerva_pipeline::classifier::{ClassifiedKind, Classifier};

use crate::classification::CerebrasClassifier;
use crate::feature_flags;
use crate::relink_scheduler;
use crate::state::AppState;

/// No-op classifier used when KG is gated off for a course. Returns
/// an empty kind (which `process_document` interprets as "don't stamp
/// a kind into Qdrant payload" and `set_classification` ignores).
/// Lets us keep `process_document`'s signature unchanged whether or
/// not KG is enabled; the gating decision lives entirely in the
/// worker, not in the ingest crate.
struct NoopClassifier;

#[async_trait]
impl Classifier for NoopClassifier {
    async fn classify(
        &self,
        _course_id: uuid::Uuid,
        _filename: &str,
        _mime_type: &str,
        _text: &str,
    ) -> Result<ClassifiedKind, String> {
        // Returning an Err means `set_classification` is never called
        // and the doc keeps `kind = NULL`. The chat-time partition
        // (which is also gated on KG) treats NULL as "no classification
        // applied", so for KG-disabled courses chunks just flow through
        // as plain RAG context.
        Err("kg disabled for course".to_string())
    }
}

/// How often the worker checks for new pending documents.
const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(2);

/// How often the periodic sweeper runs.
const SWEEP_INTERVAL: std::time::Duration = std::time::Duration::from_secs(120);

/// All `documents.status` values, so the queue-depth gauge can reset each
/// to zero before overlaying live counts (see `emit_queue_depth`).
const KNOWN_STATUSES: [&str; 7] = [
    "pending",
    "processing",
    "ready",
    "failed",
    "awaiting_transcript",
    "unsupported",
    "tracked",
];

/// Publish the `worker_queue_depth{status=...}` gauge. Zeroes the full
/// known-status set first so a status that drops to zero rows reports 0
/// rather than going stale at its last value (a GROUP BY omits empty
/// groups, so without this a drained backlog would read as still-full).
async fn emit_queue_depth(db: &sqlx::PgPool) {
    for s in KNOWN_STATUSES {
        metrics::gauge!("worker_queue_depth", "status" => s).set(0.0);
    }
    match minerva_db::queries::documents::count_by_status(db).await {
        Ok(counts) => {
            for (status, n) in counts {
                metrics::gauge!("worker_queue_depth", "status" => status).set(n as f64);
            }
        }
        Err(e) => tracing::warn!("worker: queue-depth count failed: {}", e),
    }
}

/// A document whose `processing_started_at` is older than this is considered
/// wedged and will be reset to `pending` by the sweeper. Must comfortably
/// exceed any legitimate processing time (largest transcripts + model load).
const STALE_THRESHOLD_SECS: i64 = 600;

/// Spawn the worker-side background tasks: the relink sweeper, the
/// stale-doc sweeper, and the main doc-claim loop.
///
/// The relink sweeper stays with the worker (rather than moving to
/// minerva-scheduler) because its actual sweep does fat work
/// (reads classifications, rebuilds the cross-doc graph, writes back
/// to qdrant) and is tightly coupled to the ingest pipeline's output.
/// The "tick every 60s" shape it shares with the scheduler-bound
/// tasks is superficial. See `docs/ARCHITECTURE.md`.
///
/// The stale-doc sweeper is the recovery half of this binary's claim
/// loop; it must restart with the worker, not the scheduler.
pub fn start_worker_loops(state: AppState, max_concurrent: usize) {
    relink_scheduler::spawn_sweep(state.clone());

    // Periodic sweeper: rescue documents whose processing task died silently
    // (e.g. panic inside the spawned task). Runs independently of the main
    // poll loop so a wedged main loop can't block the safety net.
    {
        let db = state.db.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(SWEEP_INTERVAL).await;
                match minerva_db::queries::documents::reset_stale_processing_older_than(
                    &db,
                    STALE_THRESHOLD_SECS,
                )
                .await
                {
                    Ok(0) => {}
                    Ok(n) => tracing::warn!(
                        "worker: sweeper reset {} document(s) wedged in 'processing' for > {}s",
                        n,
                        STALE_THRESHOLD_SECS,
                    ),
                    Err(e) => tracing::error!("worker: sweeper failed: {}", e),
                }
            }
        });
    }

    spawn_main_claim_loop(state, max_concurrent);
}

/// The main `documents.status = 'pending'` claim loop. Factored out of
/// `start_worker_loops` so the worker's three concerns (relink sweep,
/// stale-doc sweep, claim loop) read as three sibling spawns rather
/// than two helpers around one giant inline future.
fn spawn_main_claim_loop(state: AppState, max_concurrent: usize) {
    tokio::spawn(async move {
        // Crash recovery: any document left in 'processing' was interrupted.
        match minerva_db::queries::documents::reset_stale_processing(&state.db).await {
            Ok(0) => {}
            Ok(n) => tracing::info!(
                "worker: reset {} stale processing document(s) to pending",
                n
            ),
            Err(e) => tracing::error!("worker: failed to reset stale documents: {}", e),
        }

        let semaphore = Arc::new(Semaphore::new(max_concurrent));

        // Classifiers live for the lifetime of the worker and are
        // shared across spawned per-document tasks via Arc. One
        // reqwest::Client per worker is fine; Cerebras requests are
        // cheap and the client owns a connection pool.
        //
        // We hold both a real classifier and a no-op so we can pick
        // per-doc based on the KG feature flag without re-allocating
        // anything in the hot loop.
        let kg_classifier: Arc<dyn Classifier> = Arc::new(CerebrasClassifier::new(
            reqwest::Client::new(),
            state.config.cerebras_api_key.clone(),
            state.db.clone(),
        ));
        let noop_classifier: Arc<dyn Classifier> = Arc::new(NoopClassifier);

        // Throttle the queue-depth gauge: the busy path loops faster than
        // POLL_INTERVAL, and a backlog doesn't need sub-5s resolution.
        let mut last_depth_emit: Option<std::time::Instant> = None;

        loop {
            if last_depth_emit.is_none_or(|t| t.elapsed() >= std::time::Duration::from_secs(5)) {
                emit_queue_depth(&state.db).await;
                last_depth_emit = Some(std::time::Instant::now());
            }

            // Calculate how many slots are free so we only claim what we can process.
            let available = semaphore.available_permits() as i32;
            if available == 0 {
                tokio::time::sleep(POLL_INTERVAL).await;
                continue;
            }

            let docs =
                match minerva_db::queries::documents::claim_pending(&state.db, available).await {
                    Ok(docs) => docs,
                    Err(e) => {
                        tracing::error!("worker: failed to claim pending documents: {}", e);
                        tokio::time::sleep(POLL_INTERVAL).await;
                        continue;
                    }
                };

            if docs.is_empty() {
                tokio::time::sleep(POLL_INTERVAL).await;
                continue;
            }

            tracing::info!("worker: claimed {} document(s) for processing", docs.len());

            for doc in docs {
                let permit = semaphore.clone().acquire_owned().await.unwrap();
                let db = state.db.clone();
                let qdrant = Arc::clone(&state.qdrant);
                let fastembed = Arc::clone(&state.fastembed);
                // Per-doc gate: if the course has the KG feature flag
                // off, swap in the no-op classifier so the ingest
                // pipeline doesn't burn a Cerebras call AND doesn't
                // emit a kind into Qdrant. The mark_dirty for the
                // relink sweeper is also skipped further down.
                let kg_on = feature_flags::course_kg_enabled(&db, doc.course_id).await;
                let classifier = if kg_on {
                    Arc::clone(&kg_classifier)
                } else {
                    Arc::clone(&noop_classifier)
                };
                let openai_api_key = state.config.openai_api_key.clone();
                let docs_path = state.config.docs_path.clone();
                let doc_id = doc.id;
                let course_id_for_relink = doc.course_id;
                let scheduler = state.relink_scheduler.clone();

                // Inner task does the work. Outer task awaits its JoinHandle
                // so panics become explicit log lines + a 'failed' status,
                // instead of silently wedging the document in 'processing'.
                let inner_db = db.clone();
                let inner = tokio::spawn(async move {
                    let _permit = permit; // held until this task completes

                    // Look up course to get embedding config.
                    let course =
                        match minerva_db::queries::courses::find_by_id(&db, doc.course_id).await {
                            Ok(Some(c)) => c,
                            Ok(None) => {
                                tracing::error!(
                                    "worker: course {} not found for doc {}",
                                    doc.course_id,
                                    doc.id
                                );
                                set_failed(&db, doc.id, "course not found").await;
                                return;
                            }
                            Err(e) => {
                                tracing::error!("worker: db error looking up course: {}", e);
                                set_failed(&db, doc.id, &format!("db error: {}", e)).await;
                                return;
                            }
                        };

                    let ext = minerva_pipeline::pipeline::extension_from_filename(&doc.filename);

                    // URL documents: route by URL shape.
                    //
                    // Priority order matters: GitHub PDFs are downloaded
                    // inline (the worker grabs the bytes and re-queues the
                    // doc as a regular PDF), play.dsv.su.se links wait for
                    // the external transcript pipeline, and everything
                    // else is parked as `unsupported`.
                    if ext == "url" {
                        let file_path =
                            format!("{}/{}/{}.{}", docs_path, doc.course_id, doc.id, ext);
                        let raw_url = tokio::fs::read_to_string(&file_path)
                            .await
                            .unwrap_or_default();
                        let url = raw_url.trim();

                        if let Some(gh) = crate::github_url::detect(url) {
                            match download_github_pdf(&db, &doc, &gh, &docs_path).await {
                                Ok((child_id, child_filename)) => {
                                    tracing::info!(
                                        "worker: url doc {} ({}) materialized GitHub PDF {} as child {} ({}); parent now tracked",
                                        doc.id,
                                        doc.filename,
                                        gh.download_url,
                                        child_id,
                                        child_filename,
                                    );
                                }
                                Err(e) => {
                                    tracing::warn!(
                                        "worker: url doc {} ({}) github pdf download failed: {}",
                                        doc.id,
                                        doc.filename,
                                        e,
                                    );
                                    set_failed(
                                        &db,
                                        doc.id,
                                        &format!("github pdf download failed: {}", e),
                                    )
                                    .await;
                                }
                            }
                            return;
                        }

                        if url.contains("play.dsv.su.se") {
                            tracing::info!(
                                "worker: document {} ({}) is a play.dsv.su.se URL, awaiting transcript",
                                doc.id,
                                doc.filename,
                            );
                            let _ = sqlx::query!(
                                "UPDATE documents SET status = 'awaiting_transcript' WHERE id = $1",
                                doc.id,
                            )
                            .execute(&db)
                            .await;
                        } else {
                            tracing::info!(
                                "worker: document {} ({}) is an unsupported URL, marking unsupported",
                                doc.id,
                                doc.filename,
                            );
                            let _ = sqlx::query!(
                                "UPDATE documents SET status = 'unsupported' WHERE id = $1",
                                doc.id,
                            )
                            .execute(&db)
                            .await;
                        }
                        return;
                    }

                    // Only process supported file types; store others as 'unsupported'.
                    let is_supported = matches!(
                        ext,
                        "pdf" | "txt" | "html" | "htm" | "md" | "rst" | "csv" | "tsv"
                    );
                    if !is_supported {
                        tracing::info!(
                            "worker: document {} ({}) is not a supported type, marking as unsupported",
                            doc.id,
                            doc.filename,
                        );
                        let _ = sqlx::query!(
                            "UPDATE documents SET status = 'unsupported' WHERE id = $1",
                            doc.id,
                        )
                        .execute(&db)
                        .await;
                        return;
                    }

                    let file_path = format!("{}/{}/{}.{}", docs_path, doc.course_id, doc.id, ext);
                    let path = std::path::Path::new(&file_path);
                    let client = reqwest::Client::new();

                    let ingest_start = std::time::Instant::now();
                    match minerva_pipeline::pipeline::process_document(
                        &db,
                        &qdrant,
                        &client,
                        &openai_api_key,
                        &fastembed,
                        &classifier,
                        doc.id,
                        doc.course_id,
                        path,
                        &doc.filename,
                        &doc.mime_type,
                        &course.embedding_provider,
                        &course.embedding_model,
                        course.embedding_version,
                    )
                    .await
                    {
                        Ok(result) => {
                            metrics::histogram!("worker_document_ingest_seconds", "outcome" => "success")
                                .record(ingest_start.elapsed().as_secs_f64());
                            metrics::counter!("worker_ingest_total", "outcome" => "success")
                                .increment(1);
                            tracing::info!(
                                "worker: document {} processed: {} chunks, {} embedding tokens",
                                doc.id,
                                result.chunk_count,
                                result.embedding_tokens,
                            );
                            // Auto-trigger a debounced relink for the
                            // course so the knowledge graph stays
                            // fresh after every ingest. Bursty Moodle
                            // syncs coalesce into a single linker
                            // call thanks to the debounce window in
                            // RelinkScheduler; with a hard cap so a
                            // long sustained burst still fires the
                            // linker within MAX_PENDING_AGE.
                            //
                            // Skipped entirely when the course has
                            // KG disabled; nothing classified means
                            // nothing for the linker to chew on.
                            if kg_on {
                                scheduler.mark_dirty(course_id_for_relink).await;
                                tracing::info!(
                                    "worker: marked course {} dirty after doc {} ingest; linker will fire on next sweep tick",
                                    course_id_for_relink,
                                    doc.id,
                                );
                            }
                        }
                        Err(e) => {
                            metrics::histogram!("worker_document_ingest_seconds", "outcome" => "failed")
                                .record(ingest_start.elapsed().as_secs_f64());
                            metrics::counter!("worker_ingest_total", "outcome" => "failed")
                                .increment(1);
                            tracing::error!("worker: document {} processing failed: {}", doc.id, e);
                            set_failed(&db, doc.id, &e).await;
                        }
                    }
                });

                // Supervisor: catch panics so we learn about them (and so the
                // doc doesn't sit in 'processing' until the next pod restart).
                tokio::spawn(async move {
                    match inner.await {
                        Ok(()) => {}
                        Err(e) if e.is_panic() => {
                            metrics::counter!("worker_ingest_total", "outcome" => "panicked")
                                .increment(1);
                            tracing::error!("worker: document {} task panicked: {:?}", doc_id, e,);
                            set_failed(&inner_db, doc_id, "processing task panicked").await;
                        }
                        Err(e) => {
                            metrics::counter!("worker_ingest_total", "outcome" => "cancelled")
                                .increment(1);
                            tracing::error!("worker: document {} task cancelled: {}", doc_id, e,);
                            set_failed(&inner_db, doc_id, "processing task cancelled").await;
                        }
                    }
                });
            }
        }
    });
}

async fn set_failed(db: &sqlx::PgPool, doc_id: uuid::Uuid, msg: &str) {
    let _ = sqlx::query!(
        "UPDATE documents SET status = 'failed', error_msg = $1 WHERE id = $2",
        msg,
        doc_id,
    )
    .execute(db)
    .await;
}

/// Download a GitHub-hosted PDF inline and materialize it as a child
/// of the URL stub: writes `{child_id}.pdf` to disk, inserts a new doc
/// row with `parent_document_id = url_doc.id`, and flips the parent to
/// `tracked`. The `.url` file on disk and the parent row are left
/// intact so the origin URL stays a first-class record. Returns
/// `(child_id, child_filename)`.
///
/// Size is capped at the admin-tunable `max_upload_bytes`
/// (same ceiling as teacher uploads); non-PDF responses are rejected
/// by the `%PDF-` magic-bytes check (defense against GitHub serving
/// an HTML error page with 200 status for unknown tags via the
/// /releases/latest/download/ redirect). The cap is read live from
/// `system_defaults`, so an admin lowering the upload limit also
/// shrinks the worker-side inline downloader.
async fn download_github_pdf(
    db: &sqlx::PgPool,
    parent: &minerva_db::queries::documents::DocumentRow,
    gh: &crate::github_url::GithubPdfUrl,
    docs_path: &str,
) -> Result<(uuid::Uuid, String), String> {
    use sha2::{Digest, Sha256};

    let max_bytes: usize = crate::system_defaults::max_upload_bytes(db).await as usize;

    // `redirect(Limited(10))` mirrors reqwest's default but is explicit:
    // /raw/ -> raw.githubusercontent.com, and /releases/latest/download/ ->
    // /releases/download/{tag}/ both rely on 302 chains.
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::limited(10))
        .build()
        .map_err(|e| format!("http client init: {}", e))?;

    let mut resp = client
        .get(&gh.download_url)
        .header(reqwest::header::USER_AGENT, "minerva-ingest/1.0")
        .header(reqwest::header::ACCEPT, "application/pdf, */*")
        .send()
        .await
        .map_err(|e| format!("request: {}", e))?;

    let status = resp.status();
    if !status.is_success() {
        return Err(format!("http {}", status.as_u16()));
    }

    // Early reject when the server tells us the body would exceed our cap.
    // We still cap streaming-side too because Content-Length can be absent
    // or wrong.
    if let Some(len) = resp.content_length() {
        if len as usize > max_bytes {
            return Err(format!("response too large ({} bytes)", len));
        }
    }

    let mut buf: Vec<u8> = Vec::new();
    while let Some(chunk) = resp
        .chunk()
        .await
        .map_err(|e| format!("body read: {}", e))?
    {
        if buf.len() + chunk.len() > max_bytes {
            return Err(format!("response exceeds {} byte cap", max_bytes));
        }
        buf.extend_from_slice(&chunk);
    }

    if !buf.starts_with(b"%PDF-") {
        // Most common failure mode is GitHub serving an HTML "404 not
        // found" page (still HTTP 200 for unknown release tags via the
        // /releases/latest/download/ redirect). Magic-bytes guard makes
        // sure we don't hand garbage to the PDF parser.
        return Err("response is not a PDF (missing %PDF- header)".to_string());
    }

    let child_id = uuid::Uuid::new_v4();
    let dir = format!("{}/{}", docs_path, parent.course_id);
    let pdf_path = format!("{}/{}.pdf", dir, child_id);
    tokio::fs::write(&pdf_path, &buf)
        .await
        .map_err(|e| format!("write pdf: {}", e))?;

    let child_filename = derive_pdf_filename(&parent.filename, &gh.suggested_filename);
    let size_bytes = buf.len() as i64;
    let mut hasher = Sha256::new();
    hasher.update(&buf);
    let content_hash = hex::encode(hasher.finalize());

    let result = minerva_db::queries::documents::insert_tracked_child(
        db,
        parent.id,
        "processing",
        minerva_db::queries::documents::NewDocument {
            id: child_id,
            course_id: parent.course_id,
            filename: &child_filename,
            mime_type: "application/pdf",
            size_bytes,
            uploaded_by: parent.uploaded_by,
            // URL identity lives on the parent only. The unique index
            // `idx_documents_course_source_url` enforces one stub per
            // (course, URL); copying the URL onto the child would
            // collide with the parent. Consumers that need the URL
            // follow `parent_document_id` instead.
            source_url: None,
            content_hash: Some(&content_hash),
            // The child is derivative; source identity (Moodle / Canvas)
            // lives on the parent only.
            source_system: None,
            source_ref: None,
            parent_document_id: Some(parent.id),
        },
    )
    .await;

    match result {
        Ok(_) => Ok((child_id, child_filename)),
        Err(sqlx::Error::RowNotFound) => {
            // Race: parent moved out of `processing` between the worker
            // claiming it and our transaction (sweeper rescued it, or it
            // was deleted). Clean up the orphaned PDF.
            let _ = tokio::fs::remove_file(&pdf_path).await;
            Err("parent doc no longer in processing state".to_string())
        }
        Err(e) => {
            let _ = tokio::fs::remove_file(&pdf_path).await;
            Err(format!("db insert: {}", e))
        }
    }
}

/// Build a `.pdf` filename for the re-queued document.
///
/// Strips the `.url` suffix from the stored filename (it was added by the
/// caller when the URL doc was first created). If the result already ends
/// in `.pdf` (case-insensitive), keep it; otherwise fall back to the
/// filename derived from the URL itself. We never let the suggested
/// filename win outright because teachers / Moodle plugins often give
/// URL stubs nicer human-readable names than the basename in the URL.
fn derive_pdf_filename(stored: &str, url_basename: &str) -> String {
    let stripped = stored.strip_suffix(".url").unwrap_or(stored);
    if stripped.to_ascii_lowercase().ends_with(".pdf") && !stripped.is_empty() {
        return stripped.to_string();
    }
    if !stripped.is_empty() {
        return format!("{}.pdf", stripped);
    }
    url_basename.to_string()
}

#[cfg(test)]
mod tests {
    use super::derive_pdf_filename;

    #[test]
    fn keeps_stored_filename_when_already_pdf() {
        assert_eq!(
            derive_pdf_filename("spec.pdf.url", "fallback.pdf"),
            "spec.pdf"
        );
        assert_eq!(
            derive_pdf_filename("Lecture Notes.PDF.url", "fallback.pdf"),
            "Lecture Notes.PDF",
        );
    }

    #[test]
    fn appends_pdf_when_missing() {
        assert_eq!(
            derive_pdf_filename("Lecture Notes.url", "fallback.pdf"),
            "Lecture Notes.pdf",
        );
    }

    #[test]
    fn falls_back_to_url_basename_when_stripped_is_empty() {
        assert_eq!(derive_pdf_filename(".url", "handbook.pdf"), "handbook.pdf");
    }

    #[test]
    fn handles_missing_url_suffix() {
        // Defensive: even if the stored filename somehow lacks `.url`, we
        // still produce a `.pdf` name.
        assert_eq!(derive_pdf_filename("Notes", "fallback.pdf"), "Notes.pdf");
    }

    /// Live HTTP probe against a real public GitHub-hosted PDF. Ignored by
    /// default so CI without network access stays green; run explicitly
    /// with `cargo test --ignored github_pdf_download_real`.
    ///
    /// Exercises the same reqwest config the worker uses (redirect chain
    /// from github.com/.../raw/... -> raw.githubusercontent.com, User-Agent
    /// header) plus the magic-bytes check the worker relies on to reject
    /// HTML error pages the GitHub raw endpoint sometimes serves with a
    /// 200 status code.
    #[tokio::test]
    #[ignore]
    async fn github_pdf_download_real() {
        let url = "https://github.com/niuxinghua/SpringBooks/raw/master/hbase.pdf";
        let parsed = crate::github_url::detect(url).expect("should detect");
        assert_eq!(parsed.suggested_filename, "hbase.pdf");

        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::limited(10))
            .build()
            .unwrap();
        let resp = client
            .get(&parsed.download_url)
            .header(reqwest::header::USER_AGENT, "minerva-ingest/1.0")
            .header(reqwest::header::ACCEPT, "application/pdf, */*")
            .send()
            .await
            .expect("network");
        assert!(resp.status().is_success(), "status {}", resp.status());
        let bytes = resp.bytes().await.expect("body");
        assert!(
            bytes.starts_with(b"%PDF-"),
            "first bytes were {:?}",
            &bytes[..bytes.len().min(8)]
        );
    }
}
