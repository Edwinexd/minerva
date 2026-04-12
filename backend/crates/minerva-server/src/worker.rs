//! Background document-processing worker.
//!
//! Instead of spawning an unbounded `tokio::spawn` per upload, documents are
//! inserted as `pending` and this worker polls the `documents` table using
//! `SELECT ... FOR UPDATE SKIP LOCKED` -- a standard Postgres job-queue pattern.
//!
//! Concurrency is bounded by a semaphore so we never overwhelm the embedding
//! API, Qdrant, or server memory when a teacher syncs a large course at once.
//!
//! Stuck-doc recovery has two tiers:
//! * Startup: `reset_stale_processing` unconditionally resets anything left
//!   in `processing` -- covers crashes/OOMs that skipped graceful shutdown.
//! * Periodic sweep: `reset_stale_processing_older_than(STALE_THRESHOLD_SECS)`
//!   handles docs wedged by a silent task panic inside a still-running pod.

use std::sync::Arc;
use tokio::sync::Semaphore;

use crate::state::AppState;

/// How often the worker checks for new pending documents.
const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(2);

/// How often the periodic sweeper runs.
const SWEEP_INTERVAL: std::time::Duration = std::time::Duration::from_secs(120);

/// A document whose `processing_started_at` is older than this is considered
/// wedged and will be reset to `pending` by the sweeper. Must comfortably
/// exceed any legitimate processing time (largest transcripts + model load).
const STALE_THRESHOLD_SECS: i64 = 600;

/// Start the background document-processing worker.
///
/// On startup it resets any documents stuck in `processing` (crash recovery),
/// then enters a loop that claims pending documents and processes them with
/// bounded concurrency.
pub fn start(state: AppState, max_concurrent: usize) {
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
                let openai_api_key = state.config.openai_api_key.clone();
                let docs_path = state.config.docs_path.clone();
                let doc_id = doc.id;

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

                    // URL documents: check if they're play.dsv.su.se links that
                    // the external transcript pipeline can handle.
                    if ext == "url" {
                        let file_path =
                            format!("{}/{}/{}.{}", docs_path, doc.course_id, doc.id, ext);
                        let url = tokio::fs::read_to_string(&file_path)
                            .await
                            .unwrap_or_default();
                        if url.contains("play.dsv.su.se") {
                            tracing::info!(
                                "worker: document {} ({}) is a play.dsv.su.se URL, awaiting transcript",
                                doc.id,
                                doc.filename,
                            );
                            let _ = sqlx::query(
                                "UPDATE documents SET status = 'awaiting_transcript' WHERE id = $1",
                            )
                            .bind(doc.id)
                            .execute(&db)
                            .await;
                        } else {
                            tracing::info!(
                                "worker: document {} ({}) is a non-play URL, marking as unsupported",
                                doc.id,
                                doc.filename,
                            );
                            let _ = sqlx::query(
                                "UPDATE documents SET status = 'unsupported' WHERE id = $1",
                            )
                            .bind(doc.id)
                            .execute(&db)
                            .await;
                        }
                        return;
                    }

                    // Only process supported file types; store others as 'unsupported'.
                    if ext != "pdf" && ext != "txt" {
                        tracing::info!(
                            "worker: document {} ({}) is not a supported type, marking as unsupported",
                            doc.id,
                            doc.filename,
                        );
                        let _ = sqlx::query(
                            "UPDATE documents SET status = 'unsupported' WHERE id = $1",
                        )
                        .bind(doc.id)
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
                        doc.id,
                        doc.course_id,
                        path,
                        &doc.filename,
                        &course.embedding_provider,
                        &course.embedding_model,
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
    let _ = sqlx::query("UPDATE documents SET status = 'failed', error_msg = $1 WHERE id = $2")
        .bind(msg)
        .bind(doc_id)
        .execute(db)
        .await;
}
