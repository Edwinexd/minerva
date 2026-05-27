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
use minerva_ingest::classifier::{ClassifiedKind, Classifier};

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

/// A document whose `processing_started_at` is older than this is considered
/// wedged and will be reset to `pending` by the sweeper. Must comfortably
/// exceed any legitimate processing time (largest transcripts + model load).
const STALE_THRESHOLD_SECS: i64 = 600;

/// How often the Canvas auto-sync loop checks for due connections. Effective
/// lag is at most this plus `canvas_auto_sync_interval_hours`, so a 24h
/// interval never drifts more than ~25h in practice.
const CANVAS_AUTO_SYNC_CHECK_INTERVAL: std::time::Duration =
    std::time::Duration::from_secs(60 * 60);

/// How often the LTI NRPS reconcile loop checks for due contexts. Same
/// rationale as the Canvas check interval: effective lag is at most this
/// plus `lti_nrps_sync_interval_hours`.
const LTI_NRPS_CHECK_INTERVAL: std::time::Duration = std::time::Duration::from_secs(30 * 60);

/// Start the background document-processing worker.
///
/// On startup it resets any documents stuck in `processing` (crash recovery),
/// then enters a loop that claims pending documents and processes them with
/// bounded concurrency.
///
/// Also spawns the relink sweeper; the debounced background task that
/// drains the per-course dirty queue and re-runs the cross-doc linker.
/// Sibling task to the document worker because both have the same
/// "background sweep over course-scoped state" shape.
pub fn start(state: AppState, max_concurrent: usize) {
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

    // Canvas auto-sync: periodic re-sync for connections with auto_sync=true
    // whose last_synced_at is older than the configured interval. Runs
    // sequentially across due connections so we don't stampede Canvas.
    //
    // `interval_hours` is read from `system_defaults` *inside* the loop
    // so an admin's edit on `/admin/defaults` takes effect on the next
    // check-tick without a restart. 0 disables the sync entirely for
    // this tick; we re-check on the next iteration in case the admin
    // re-enables it.
    {
        let state = state.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(CANVAS_AUTO_SYNC_CHECK_INTERVAL).await;
                let interval_hours =
                    crate::system_defaults::canvas_auto_sync_interval_hours(&state.db).await;
                if interval_hours <= 0 {
                    continue;
                }
                let due = match minerva_db::queries::canvas::find_due_for_auto_sync(
                    &state.db,
                    interval_hours,
                )
                .await
                {
                    Ok(rows) => rows,
                    Err(e) => {
                        tracing::error!("canvas auto-sync: query failed: {}", e);
                        continue;
                    }
                };
                if due.is_empty() {
                    continue;
                }
                tracing::info!(
                    "canvas auto-sync: {} connection(s) due (interval {}h)",
                    due.len(),
                    interval_hours,
                );
                for conn in due {
                    let conn_id = conn.id;
                    let name = conn.name.clone();
                    match crate::routes::canvas::run_sync(&state, &conn).await {
                        Ok(r) => tracing::info!(
                            "canvas auto-sync: connection {} ({}): {} new, {} resynced, {} skipped, {} errors, {} warnings",
                            conn_id,
                            name,
                            r.synced,
                            r.resynced,
                            r.skipped,
                            r.errors.len(),
                            r.warnings.len(),
                        ),
                        Err(e) => tracing::error!(
                            "canvas auto-sync: connection {} ({}) failed: {}",
                            conn_id,
                            name,
                            e,
                        ),
                    }
                }
            }
        });
    }

    // LTI NRPS reconcile: periodically pull each syncable context's roster
    // from the LMS and add/remove course members. Runs sequentially across
    // due contexts so we don't stampede a platform's token + membership
    // endpoints. Removal is LTI-sourced-only (see lti_nrps::reconcile_context).
    //
    // Like the Canvas sweep above, `nrps_interval_hours` is read from
    // `system_defaults` per-tick so admin edits propagate without a
    // restart. 0 = skip this tick.
    {
        let state = state.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(LTI_NRPS_CHECK_INTERVAL).await;
                let nrps_interval_hours =
                    crate::system_defaults::lti_nrps_sync_interval_hours(&state.db).await;
                if nrps_interval_hours <= 0 {
                    continue;
                }
                let due = match minerva_db::queries::lti_nrps::find_due_for_sync(
                    &state.db,
                    nrps_interval_hours,
                )
                .await
                {
                    Ok(rows) => rows,
                    Err(e) => {
                        tracing::error!("lti nrps: due query failed: {}", e);
                        continue;
                    }
                };
                if due.is_empty() {
                    continue;
                }
                tracing::info!(
                    "lti nrps: {} context(s) due (interval {}h)",
                    due.len(),
                    nrps_interval_hours,
                );
                for ctx in due {
                    match crate::lti_nrps::reconcile_context(&state, &ctx).await {
                        Ok(outcome) => {
                            tracing::info!(
                                "lti nrps: context {} (course {}): {} added, {} removed",
                                ctx.id,
                                ctx.course_id,
                                outcome.added,
                                outcome.removed,
                            );
                            if let Some(w) = outcome.warning.as_deref() {
                                tracing::warn!(
                                    "lti nrps: context {} (course {}) warning: {}",
                                    ctx.id,
                                    ctx.course_id,
                                    w
                                );
                            }
                            if let Err(e) = minerva_db::queries::lti_nrps::record_sync_result(
                                &state.db,
                                ctx.id,
                                "ok",
                                None,
                                outcome.warning.as_deref(),
                                Some(outcome.added),
                                Some(outcome.removed),
                            )
                            .await
                            {
                                tracing::error!(
                                    "lti nrps: failed to record sync result for {}: {}",
                                    ctx.id,
                                    e
                                );
                            }
                        }
                        Err(e) => {
                            tracing::error!(
                                "lti nrps: context {} (course {}) failed: {}",
                                ctx.id,
                                ctx.course_id,
                                e,
                            );
                            let _ = minerva_db::queries::lti_nrps::record_sync_result(
                                &state.db,
                                ctx.id,
                                "error",
                                Some(&e.to_string()),
                                None,
                                None,
                                None,
                            )
                            .await;
                        }
                    }
                }
            }
        });
    }

    // Periodic cleanup of unapproved (dynreg-installed) platforms. Anyone
    // can hit `/lti/dynamic-register` so pending rows could otherwise pile
    // up indefinitely. After 7 days of no approval, drop them; the admin
    // either intended to approve and lost track (in which case the LMS
    // admin can re-run dynreg), or never intended to (spam / mistake).
    {
        let state = state.clone();
        tokio::spawn(async move {
            const SWEEP_INTERVAL: std::time::Duration = std::time::Duration::from_secs(60 * 60); // hourly
            const MAX_AGE_HOURS: i32 = 24 * 7;
            loop {
                tokio::time::sleep(SWEEP_INTERVAL).await;
                match minerva_db::queries::lti::delete_stale_pending_platforms(
                    &state.db,
                    MAX_AGE_HOURS,
                )
                .await
                {
                    Ok(0) => {}
                    Ok(n) => tracing::info!(
                        "lti dynreg: dropped {} stale pending platform row(s) older than {}h",
                        n,
                        MAX_AGE_HOURS
                    ),
                    Err(e) => tracing::error!("lti dynreg: stale pending sweep failed: {}", e),
                }
            }
        });
    }

    // Platform-health probe: every active platform's token endpoint is
    // pinged daily with a throwaway client_credentials JWT. If the LMS
    // rejects with `invalid_client` continuously for 30 days, the row
    // is cascade-deleted (bindings + NRPS contexts go with it via FK).
    // This is how we detect "the LMS admin deleted us"; the spec
    // doesn't notify the tool, so we have to ask.
    {
        let state = state.clone();
        tokio::spawn(async move {
            const PROBE_INTERVAL: std::time::Duration =
                std::time::Duration::from_secs(24 * 60 * 60);
            const ORPHAN_GRACE_DAYS: i32 = 30;
            loop {
                tokio::time::sleep(PROBE_INTERVAL).await;
                let platforms = match minerva_db::queries::lti::list_platforms_for_health_check(
                    &state.db,
                )
                .await
                {
                    Ok(p) => p,
                    Err(e) => {
                        tracing::error!("lti health: list query failed: {}", e);
                        continue;
                    }
                };
                for p in &platforms {
                    let status = crate::lti_nrps::probe_platform_health(&state, p).await;
                    if let Err(e) =
                        minerva_db::queries::lti::record_platform_health(&state.db, p.id, &status)
                            .await
                    {
                        tracing::error!(
                            "lti health: failed to record probe for platform {}: {}",
                            p.id,
                            e
                        );
                        continue;
                    }
                    if status != "ok" {
                        tracing::warn!(
                            "lti health: platform {} ({}) probe -> {}",
                            p.id,
                            p.issuer,
                            status
                        );
                    }
                }
                match minerva_db::queries::lti::delete_long_orphaned_platforms(
                    &state.db,
                    ORPHAN_GRACE_DAYS,
                )
                .await
                {
                    Ok(0) => {}
                    Ok(n) => tracing::warn!(
                        "lti health: cascade-deleted {} platform row(s) the LMS has been rejecting for {}+ days",
                        n,
                        ORPHAN_GRACE_DAYS
                    ),
                    Err(e) => tracing::error!("lti health: orphan delete failed: {}", e),
                }
            }
        });
    }

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

        loop {
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

                    let ext = crate::routes::documents::extension_from_filename(&doc.filename);

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

                    match minerva_ingest::pipeline::process_document(
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
                            tracing::error!("worker: document {} task panicked: {:?}", doc_id, e,);
                            set_failed(&inner_db, doc_id, "processing task panicked").await;
                        }
                        Err(e) => {
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
    // /raw/ → raw.githubusercontent.com, and /releases/latest/download/ →
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
    /// from github.com/.../raw/... → raw.githubusercontent.com, User-Agent
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
