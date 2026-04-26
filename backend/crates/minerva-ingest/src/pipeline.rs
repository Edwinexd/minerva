use std::path::Path;
use std::sync::Arc;

use qdrant_client::qdrant::{
    CreateCollectionBuilder, CreateFieldIndexCollectionBuilder, Distance, FieldType, PointStruct,
    UpsertPointsBuilder, VectorParamsBuilder,
};
use qdrant_client::Qdrant;
use sqlx::PgPool;
use uuid::Uuid;

use crate::chunker::{self, ChunkerConfig};
use crate::classifier::Classifier;
use crate::embedder;
use crate::fastembed_embedder::FastEmbedder;
use crate::pdf;

/// Classifier output kind that triggers the embed-skip short-circuit.
/// Hard-coded here rather than imported from `minerva-server::classification`
/// to keep the dependency edge one-way.
const KIND_SAMPLE_SOLUTION: &str = "sample_solution";

enum TextSource {
    Plain,
    Html,
    Pdf,
}

fn text_source(path: &Path) -> TextSource {
    match path.extension().and_then(|e| e.to_str()) {
        Some("txt" | "md" | "rst" | "csv" | "tsv") => TextSource::Plain,
        Some("html" | "htm") => TextSource::Html,
        _ => TextSource::Pdf,
    }
}

fn html_to_text(html: &str) -> String {
    let document = scraper::Html::parse_document(html);
    // Collect all text nodes separated by whitespace, then normalize runs.
    let raw: String = document
        .root_element()
        .text()
        .flat_map(|t| [t, " "])
        .collect();
    // Collapse runs of whitespace into a single space and trim.
    raw.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Read and extract text from a document file, dispatching to the right
/// parser by extension. Exposed so the reclassify endpoint can run
/// classification on demand without re-chunking or re-embedding.
pub fn extract_document_text(file_path: &Path) -> Result<String, String> {
    match text_source(file_path) {
        TextSource::Plain => std::fs::read_to_string(file_path)
            .map_err(|e| format!("failed to read text file: {}", e)),
        TextSource::Html => {
            let raw = std::fs::read_to_string(file_path)
                .map_err(|e| format!("failed to read html file: {}", e))?;
            Ok(html_to_text(&raw))
        }
        TextSource::Pdf => {
            pdf::extract_text(file_path).map_err(|e| format!("text extraction failed: {}", e))
        }
    }
}

pub const VALID_EMBEDDING_PROVIDERS: &[&str] = &["openai", "local"];

pub const VALID_LOCAL_MODELS: &[(&str, u64)] = &[
    ("sentence-transformers/all-MiniLM-L6-v2", 384),
    ("BAAI/bge-small-en-v1.5", 384),
    ("BAAI/bge-base-en-v1.5", 768),
    ("nomic-ai/nomic-embed-text-v1.5", 768),
];

pub const OPENAI_EMBEDDING_MODEL: &str = "text-embedding-3-small";
const OPENAI_EMBEDDING_DIMENSIONS: u64 = 1536;

fn local_model_dimensions(model: &str) -> Option<u64> {
    VALID_LOCAL_MODELS
        .iter()
        .find(|(name, _)| *name == model)
        .map(|(_, dims)| *dims)
}

/// Run the full document ingestion pipeline:
/// 1. Extract text from PDF / plain / html
/// 2. Classify the document into a `kind` (lecture, assignment_brief,
///    sample_solution, …) via the supplied [`Classifier`]. Persisted
///    immediately so the chat-time RAG filter can see it the moment any
///    chunks land in Qdrant.
/// 3. **Short-circuit for `sample_solution`**: do not chunk or embed.
///    These docs must never appear in retrieval context. We still mark
///    the doc `ready` so the teacher UI can display it, but with
///    `chunk_count = 0`.
/// 4. Otherwise: chunk, embed, upsert (with `kind` baked into each
///    Qdrant point's payload so the filter is a payload check rather
///    than a DB roundtrip per retrieved chunk).
/// 5. Update document status in Postgres.
#[allow(clippy::too_many_arguments)]
pub async fn process_document(
    db: &PgPool,
    qdrant: &Qdrant,
    http_client: &reqwest::Client,
    openai_api_key: &str,
    fastembed: &Arc<FastEmbedder>,
    classifier: &Arc<dyn Classifier>,
    document_id: Uuid,
    course_id: Uuid,
    file_path: &Path,
    filename: &str,
    mime_type: &str,
    embedding_provider: &str,
    embedding_model: &str,
) -> Result<ProcessResult, String> {
    // 1. Extract text
    let text = match text_source(file_path) {
        TextSource::Plain => std::fs::read_to_string(file_path).map_err(|e| {
            let msg = format!("failed to read text file: {}", e);
            tracing::error!("{}", msg);
            msg
        })?,
        TextSource::Html => {
            let raw = std::fs::read_to_string(file_path).map_err(|e| {
                let msg = format!("failed to read html file: {}", e);
                tracing::error!("{}", msg);
                msg
            })?;
            html_to_text(&raw)
        }
        TextSource::Pdf => pdf::extract_text(file_path).map_err(|e| {
            let msg = format!("text extraction failed: {}", e);
            tracing::error!("{}", msg);
            msg
        })?,
    };

    tracing::info!("extracted {} chars from {}", text.len(), filename);

    // Zero-text fast-path: if the extractor produced nothing usable
    // (scanned PDF without OCR, empty .txt, parse failure that returned
    // an empty string) we still want a classified row so the graph viewer
    // shows the doc as "unclassified" rather than hiding it. We mark it
    // ready with chunk_count=0 and let the classifier short-circuit to
    // `unknown` with the `no_text_extracted` flag. No embedding, no
    // Qdrant upsert.
    if text.trim().is_empty() {
        tracing::warn!(
            "no usable text extracted from {}; persisting as unknown/no-content",
            filename
        );
        if let Ok(c) = classifier
            .classify(course_id, filename, mime_type, &text)
            .await
        {
            let _ = minerva_db::queries::documents::set_classification(
                db,
                document_id,
                &c.kind,
                c.confidence,
                c.rationale.as_deref(),
            )
            .await;
        }
        set_status_ready(db, document_id, 0).await;
        return Ok(ProcessResult {
            chunk_count: 0,
            embedding_tokens: 0,
        });
    }

    // 2. Classify. Errors here are not fatal -- we still ingest the doc as
    // unclassified. The chat-time filter excludes unclassified docs from
    // prompt context, so leaking is bounded; teacher can also re-trigger
    // classification from the UI.
    let kind_str: String = match classifier
        .classify(course_id, filename, mime_type, &text)
        .await
    {
        Ok(c) => {
            tracing::info!(
                "classifier: {} -> {} (confidence {:.2}, flags {:?})",
                filename,
                c.kind,
                c.confidence,
                c.suspicious_flags,
            );
            // Persist; the query is a no-op if the row is locked by a teacher.
            let _ = minerva_db::queries::documents::set_classification(
                db,
                document_id,
                &c.kind,
                c.confidence,
                c.rationale.as_deref(),
            )
            .await;
            c.kind
        }
        Err(e) => {
            tracing::warn!(
                "classifier failed for {} ({}); ingesting as unclassified",
                filename,
                e
            );
            String::new()
        }
    };

    // 3. Chunk. We chunk EVEN sample_solution docs (which won't be
    // indexed in Qdrant) -- we still need their embedding for the
    // knowledge-graph linker so a sample_solution can find its
    // assignment partner via embedding similarity, not just
    // filenames.
    let chunks = chunker::chunk_text(&text, &ChunkerConfig::default());
    if chunks.is_empty() {
        let msg = "no chunks produced from document".to_string();
        set_status(db, document_id, "failed", Some(&msg)).await;
        return Err(msg);
    }

    tracing::info!("produced {} chunks from {}", chunks.len(), filename);

    // 4. Embed + Upsert (strategy depends on provider)
    let collection_name = format!("course_{}", course_id);
    let is_sample_solution = kind_str == KIND_SAMPLE_SOLUTION;
    if is_sample_solution {
        tracing::info!(
            "embedding {} for KG only (kind=sample_solution; no Qdrant upsert)",
            filename
        );
    }

    // Capture-by-clone for the closure so it can be called per-chunk.
    let kind_for_payload = kind_str.clone();
    let build_payload = |chunk: &chunker::Chunk| -> std::collections::HashMap<String, qdrant_client::qdrant::Value> {
        let mut payload: std::collections::HashMap<String, qdrant_client::qdrant::Value> = [
            ("document_id".to_string(), document_id.to_string().into()),
            ("course_id".to_string(), course_id.to_string().into()),
            ("chunk_index".to_string(), (chunk.index as i64).into()),
            ("text".to_string(), chunk.text.clone().into()),
            ("filename".to_string(), filename.to_string().into()),
        ]
        .into_iter()
        .collect();
        // Only stamp `kind` when classification succeeded; otherwise the
        // chunk lacks the field and the chat-time filter falls through
        // to the DB-side `unclassified_doc_ids` check.
        if !kind_for_payload.is_empty() {
            payload.insert("kind".to_string(), kind_for_payload.clone().into());
        }
        payload
    };

    // Compute chunk embeddings under whichever provider this course
    // uses. We KEEP the embedding vectors in memory after upsert so
    // we can mean-pool them for the doc-level KG embedding -- one
    // pass over the data instead of re-fetching from Qdrant later.
    let (chunk_embeddings, embedding_tokens): (Vec<Vec<f32>>, i64) = match embedding_provider {
        "local" => {
            let dims = local_model_dimensions(embedding_model)
                .ok_or_else(|| format!("unsupported local embedding model: {}", embedding_model))?;
            // Only ensure the Qdrant collection exists if we're going
            // to upsert to it; sample_solution path doesn't.
            if !is_sample_solution {
                ensure_collection(qdrant, &collection_name, dims).await?;
            }

            let chunk_texts: Vec<String> = chunks.iter().map(|c| c.text.clone()).collect();
            let embeddings = fastembed.embed(embedding_model, chunk_texts).await?;

            if !is_sample_solution {
                let points: Vec<PointStruct> = chunks
                    .iter()
                    .zip(embeddings.iter())
                    .map(|(chunk, embedding)| {
                        PointStruct::new(
                            Uuid::new_v4().to_string(),
                            embedding.clone(),
                            build_payload(chunk),
                        )
                    })
                    .collect();

                upsert_batched(qdrant, &collection_name, points).await?;
                tracing::info!(
                    "upserted {} chunks via fastembed (model: {})",
                    chunks.len(),
                    embedding_model,
                );
            }

            (embeddings, 0i64)
        }
        _ => {
            if !is_sample_solution {
                ensure_collection(qdrant, &collection_name, OPENAI_EMBEDDING_DIMENSIONS).await?;
            }

            let chunk_texts: Vec<String> = chunks.iter().map(|c| c.text.clone()).collect();
            let embedding_result = embedder::embed_texts(http_client, openai_api_key, &chunk_texts)
                .await
                .map_err(|e| {
                    let msg = format!("embedding failed: {}", e);
                    tracing::error!("{}", msg);
                    msg
                })?;

            tracing::info!(
                "embedded {} chunks using {} tokens",
                embedding_result.embeddings.len(),
                embedding_result.total_tokens,
            );

            if !is_sample_solution {
                let points: Vec<PointStruct> = chunks
                    .iter()
                    .zip(embedding_result.embeddings.iter())
                    .map(|(chunk, embedding)| {
                        PointStruct::new(
                            Uuid::new_v4().to_string(),
                            embedding.clone(),
                            build_payload(chunk),
                        )
                    })
                    .collect();

                upsert_batched(qdrant, &collection_name, points).await?;
            }

            (embedding_result.embeddings, embedding_result.total_tokens)
        }
    };

    // 5. Mean-pool chunk embeddings into a single doc-level vector,
    // L2-normalize, and persist. The KG linker uses this for
    // embedding-based candidate generation: cosine similarity between
    // two L2-normalized vectors is just their dot product, so the
    // pre-normalization here saves work in the linker's pairwise loop.
    if let Some(pooled) = mean_pool_normalized(&chunk_embeddings) {
        if let Err(e) =
            minerva_db::queries::documents::set_pooled_embedding(db, document_id, &pooled).await
        {
            // Non-fatal: the linker has a Qdrant-fallback path for
            // docs without a stored pooled embedding, so we still
            // ingest successfully and just lose this optimisation.
            tracing::warn!(
                "failed to persist pooled embedding for {}: {}",
                document_id,
                e
            );
        }
    }

    // 6. Update status. sample_solution gets chunk_count=0 since no
    // chunks landed in Qdrant -- the teacher UI / RAG retrieval keys
    // off this to know there's nothing searchable.
    let chunk_count = if is_sample_solution {
        0
    } else {
        chunks.len() as i32
    };
    set_status_ready(db, document_id, chunk_count).await;

    tracing::info!(
        "document {} processed: {} chunks{}",
        document_id,
        chunk_count,
        if is_sample_solution {
            " (sample_solution; embedded for KG only, not in Qdrant)".to_string()
        } else {
            format!(" stored in collection {}", collection_name)
        },
    );

    Ok(ProcessResult {
        chunk_count,
        embedding_tokens,
    })
}

/// Mean-pool chunk embeddings into a single doc-level vector and
/// L2-normalize. Returns None if there are no chunks (caller treats
/// missing pooled embedding as "skip persist"). Pre-normalizing here
/// means cosine similarity in the linker is a single dot product.
fn mean_pool_normalized(embeddings: &[Vec<f32>]) -> Option<Vec<f32>> {
    if embeddings.is_empty() {
        return None;
    }
    let dim = embeddings[0].len();
    if dim == 0 {
        return None;
    }
    let mut sum = vec![0.0f32; dim];
    for e in embeddings {
        if e.len() != dim {
            // Inconsistent dims would be a serious bug; skip rather
            // than panic. The linker's fallback path will recompute.
            tracing::warn!(
                "mean_pool_normalized: inconsistent dim ({} vs {}); skipping",
                e.len(),
                dim
            );
            return None;
        }
        for (i, v) in e.iter().enumerate() {
            sum[i] += v;
        }
    }
    let n = embeddings.len() as f32;
    for v in sum.iter_mut() {
        *v /= n;
    }
    let norm_sq: f32 = sum.iter().map(|v| v * v).sum();
    let norm = norm_sq.sqrt();
    if norm > 0.0 {
        for v in sum.iter_mut() {
            *v /= norm;
        }
    }
    Some(sum)
}

pub struct ProcessResult {
    pub chunk_count: i32,
    pub embedding_tokens: i64,
}

async fn upsert_batched(
    qdrant: &Qdrant,
    collection_name: &str,
    points: Vec<PointStruct>,
) -> Result<(), String> {
    for batch in points.chunks(100) {
        qdrant
            .upsert_points(UpsertPointsBuilder::new(collection_name, batch.to_vec()))
            .await
            .map_err(|e| format!("qdrant upsert failed: {}", e))?;
    }
    Ok(())
}

async fn ensure_collection(qdrant: &Qdrant, name: &str, dimensions: u64) -> Result<(), String> {
    let exists = qdrant
        .collection_exists(name)
        .await
        .map_err(|e| format!("qdrant collection check failed: {}", e))?;

    if !exists {
        // Bursty Moodle / MBZ ingests fire many parallel worker tasks
        // against a brand-new course at once. Each task does a check-
        // then-create on the collection, so if N tasks race past the
        // `collection_exists` line concurrently they all decide it
        // doesn't exist and all try to create it. One wins; the others
        // see Qdrant's "already exists" error and used to fail the
        // doc. We now treat that error as "lost the race" and fall
        // through to the same dimension-verification path the
        // already-existed branch uses.
        match qdrant
            .create_collection(
                CreateCollectionBuilder::new(name)
                    .vectors_config(VectorParamsBuilder::new(dimensions, Distance::Cosine)),
            )
            .await
        {
            Ok(_) => {
                // Create payload index up-front so the very first
                // delete/scroll on this collection benefits. Only
                // done at creation time so we don't pay a round-trip
                // on every subsequent upload.
                ensure_document_id_index(qdrant, name).await;
                tracing::info!(
                    "created qdrant collection {} (dimensions: {})",
                    name,
                    dimensions
                );
                // Freshly created with the requested dimensions --
                // skip the dim verification round-trip on the happy
                // path.
                return Ok(());
            }
            Err(e) => {
                let msg = e.to_string();
                // Qdrant returns this as a tonic AlreadyExists status
                // with the substring "already exists" in the message.
                // We don't need a more structural check than this --
                // the only path that produces it is the race we're
                // handling, and any other "already exists"-flavoured
                // failure ALSO means the collection now exists and
                // we should verify dims rather than bail.
                if !msg.contains("already exists") {
                    return Err(format!("qdrant collection creation failed: {}", e));
                }
                tracing::info!(
                    "qdrant: collection {} created concurrently by another worker -- verifying dims",
                    name
                );
            }
        }
    }

    // Reached either because the collection already existed at
    // check time, or because we lost the create race. Verify the
    // existing collection has the dimensions this provider needs --
    // a mismatch (e.g. switched embedding model after some docs
    // were uploaded) is a hard failure that requires manual
    // intervention.
    let info = qdrant
        .collection_info(name)
        .await
        .map_err(|e| format!("qdrant collection info failed: {}", e))?;
    if let Some(config) = info.result.and_then(|r| r.config) {
        if let Some(params) = config.params {
            let existing_dims = params
                .vectors_config
                .and_then(|vc| vc.config)
                .and_then(|config| match config {
                    qdrant_client::qdrant::vectors_config::Config::Params(p) => Some(p.size),
                    _ => None,
                });
            if let Some(existing) = existing_dims {
                if existing != dimensions {
                    return Err(format!(
                        "collection {} has dimension {} but provider requires {}; re-upload documents after deleting existing ones",
                        name, existing, dimensions
                    ));
                }
            }
        }
    }
    Ok(())
}

/// Idempotent keyword payload index on `document_id` -- the field we filter
/// on when deleting or scrolling chunks for a single document. Without it
/// those operations do a full collection scan.
///
/// Errors are logged at debug level and swallowed: an existing index causes
/// Qdrant to return an error, and a genuinely missing one just degrades
/// performance rather than breaking anything.
pub async fn ensure_document_id_index(qdrant: &Qdrant, collection: &str) {
    if let Err(e) = qdrant
        .create_field_index(CreateFieldIndexCollectionBuilder::new(
            collection,
            "document_id",
            FieldType::Keyword,
        ))
        .await
    {
        tracing::debug!(
            "qdrant: document_id index on {} not created (likely already exists): {}",
            collection,
            e
        );
    }
}

async fn set_status(db: &PgPool, doc_id: Uuid, status: &str, error_msg: Option<&str>) {
    let _ = sqlx::query!(
        "UPDATE documents SET status = $1, error_msg = $2 WHERE id = $3",
        status,
        error_msg,
        doc_id,
    )
    .execute(db)
    .await;
}

async fn set_status_ready(db: &PgPool, doc_id: Uuid, chunk_count: i32) {
    let _ = sqlx::query!(
        "UPDATE documents SET status = 'ready', chunk_count = $1, processed_at = NOW() WHERE id = $2",
        chunk_count,
        doc_id,
    )
    .execute(db)
    .await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mean_pool_returns_none_for_empty() {
        assert!(mean_pool_normalized(&[]).is_none());
    }

    #[test]
    fn mean_pool_returns_unit_vector() {
        let v = mean_pool_normalized(&[vec![3.0, 4.0]]).unwrap();
        // Already a single vector; mean is itself; L2-normalized to
        // (0.6, 0.8).
        assert!((v[0] - 0.6).abs() < 1e-6);
        assert!((v[1] - 0.8).abs() < 1e-6);
    }

    #[test]
    fn mean_pool_averages_then_normalizes() {
        let v = mean_pool_normalized(&[vec![1.0, 0.0], vec![0.0, 1.0]]).unwrap();
        let expected = 1.0 / 2f32.sqrt();
        assert!((v[0] - expected).abs() < 1e-6);
        assert!((v[1] - expected).abs() < 1e-6);
    }

    #[test]
    fn mean_pool_rejects_dim_mismatch() {
        let v = mean_pool_normalized(&[vec![1.0, 2.0], vec![1.0, 2.0, 3.0]]);
        assert!(v.is_none());
    }
}
