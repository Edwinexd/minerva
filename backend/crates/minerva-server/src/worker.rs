//! Background document-processing worker.
//!
//! Instead of spawning an unbounded `tokio::spawn` per upload, documents are
//! inserted as `pending` and this worker polls the `documents` table using
//! `SELECT ... FOR UPDATE SKIP LOCKED` -- a standard Postgres job-queue pattern.
//!
//! Concurrency is bounded by a semaphore so we never overwhelm the embedding
//! API, Qdrant, or server memory when a teacher syncs a large course at once.

use std::sync::Arc;
use tokio::sync::Semaphore;

use crate::state::AppState;

/// How often the worker checks for new pending documents.
const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(2);

/// Start the background document-processing worker.
///
/// On startup it resets any documents stuck in `processing` (crash recovery),
/// then enters a loop that claims pending documents and processes them with
/// bounded concurrency.
pub fn start(state: AppState, max_concurrent: usize) {
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

                tokio::spawn(async move {
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

                    // Only process PDFs for now; store other types as 'unsupported'.
                    if ext != "pdf" {
                        tracing::info!(
                            "worker: document {} ({}) is not a PDF, marking as unsupported",
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
