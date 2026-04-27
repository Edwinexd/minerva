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

/// Compose the Qdrant collection name for a course at a given
/// embedding-rotation version.
///
/// `version=1` returns the legacy `course_{id}` name so existing
/// production collections keep working without a Qdrant data move.
/// `version>=2` returns `course_{id}_v{version}`; a fresh
/// collection that the lazy-migration path writes to once a teacher
/// switches embedding model.
///
/// One source of truth: every chunk-search, chunk-delete, and ingest
/// upsert in the codebase goes through this helper, so a rotation can
/// never leak references to the wrong collection.
pub fn collection_name(course_id: uuid::Uuid, version: i32) -> String {
    if version <= 1 {
        format!("course_{}", course_id)
    } else {
        format!("course_{}_v{}", course_id, version)
    }
}

/// DB-backed convenience for callers that don't already have the
/// course row in scope (route handlers that purge chunks for a single
/// document, KG linker fan-out, etc.). One round-trip; cheap. If the
/// course is missing, fall back to version=1 so we still hit the
/// legacy collection name (callers are about to fail anyway, but we
/// avoid a panic).
pub async fn collection_name_for_course(
    db: &sqlx::PgPool,
    course_id: uuid::Uuid,
) -> Result<String, sqlx::Error> {
    let version: Option<i32> = sqlx::query_scalar!(
        "SELECT embedding_version FROM courses WHERE id = $1",
        course_id,
    )
    .fetch_optional(db)
    .await?;
    Ok(collection_name(course_id, version.unwrap_or(1)))
}

pub const VALID_EMBEDDING_PROVIDERS: &[&str] = &["openai", "local"];

/// Whitelist of local embedding models a course owner can pick. Each
/// entry is `(huggingface-style id, output dimension)`. The dimension is
/// authoritative: `ensure_collection` reads from this list when creating
/// the per-course Qdrant collection, so a wrong number here means
/// upserts will fail with a vector-size mismatch.
///
/// Sourced from three backends, all dispatched by
/// `fastembed_embedder::FastEmbedder`:
/// * fastembed-rs's `EmbeddingModel` enum (ONNX, the default path);
/// * the Qwen3 candle entry (`Qwen3TextEmbedding`, gated behind
///   fastembed's `qwen3` feature);
/// * "bring your own ONNX" via `UserDefinedEmbeddingModel` for HF repos
///   whose ONNX export works but isn't part of `EmbeddingModel` yet --
///   currently snowflake-arctic-embed-m-v2.0.
///
/// Adding a model here: also add a `parse_fast_model_name` arm (or a
/// `custom_model_spec` arm for the user-defined path) in
/// `fastembed_embedder.rs`, and consider whether it's small enough to
/// warm up at boot (`STARTUP_BENCHMARK_MODELS` below). If unsure, leave
/// it out of startup; admins can run `POST /api/admin/embedding-benchmark`
/// to benchmark on demand without OOMing the box.
pub const VALID_LOCAL_MODELS: &[(&str, u64)] = &[
    // English-only, original set kept for backwards compatibility with
    // courses that picked these before multilingual options existed.
    ("sentence-transformers/all-MiniLM-L6-v2", 384),
    ("BAAI/bge-small-en-v1.5", 384),
    ("BAAI/bge-base-en-v1.5", 768),
    ("nomic-ai/nomic-embed-text-v1.5", 768),
    // Multilingual (Swedish + English, matters for SU/DSV course mix).
    ("intfloat/multilingual-e5-small", 384),
    ("intfloat/multilingual-e5-base", 768),
    ("intfloat/multilingual-e5-large", 1024),
    ("BAAI/bge-m3", 1024),
    ("google/embeddinggemma-300m", 768),
    // Snowflake Arctic Embed M v2.0: multilingual (Swedish + English),
    // 768 dims, ~311 MB int8 ONNX. Not part of fastembed-rs's
    // `EmbeddingModel` enum; loaded via `UserDefinedEmbeddingModel` --
    // see the `Backend::Custom` branch in `fastembed_embedder.rs`.
    ("Snowflake/snowflake-arctic-embed-m-v2.0", 768),
    // English, top-of-MTEB-class upgrades.
    ("mixedbread-ai/mxbai-embed-large-v1", 1024),
    ("Alibaba-NLP/gte-large-en-v1.5", 1024),
    ("snowflake/snowflake-arctic-embed-l", 1024),
    // Qwen3 (candle backend). Dim 1024, multilingual.
    ("Qwen/Qwen3-Embedding-0.6B", 1024),
];

/// Models the server warms up + benchmarks at boot. Subset of
/// `VALID_LOCAL_MODELS`: small/fast ONNX models the pod can hold in RAM
/// simultaneously without touching the cache budget too hard.
/// Everything else gets benchmarked on demand via the admin endpoint
/// so a single boot doesn't try to load every candidate at once and
/// OOM-kill the pod.
///
/// Arctic-m-v2.0 is in the warm set despite its ~311 MB int8 footprint
/// because (a) it's the multilingual default we now recommend for new
/// SU/DSV courses and (b) on first benchmark its session takes 30-60 s
/// to materialize from the freshly-downloaded ONNX; warming at boot
/// shifts that cost off the first teacher's "Run benchmark" click.
///
/// `BAAI/bge-base-en-v1.5` is intentionally not warmed: it's English-
/// only and overlapping with bge-small-en (also warmed). Existing
/// courses on bge-base still work; teachers who want a benchmark can
/// trigger one from the admin page.
pub const STARTUP_BENCHMARK_MODELS: &[(&str, u64)] = &[
    ("sentence-transformers/all-MiniLM-L6-v2", 384),
    ("BAAI/bge-small-en-v1.5", 384),
    ("nomic-ai/nomic-embed-text-v1.5", 768),
    ("Snowflake/snowflake-arctic-embed-m-v2.0", 768),
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
    embedding_version: i32,
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

    // 2. Classify. Errors here are not fatal; we still ingest the doc as
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
    // indexed in Qdrant); we still need their embedding for the
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

    // 4. Embed + Upsert (strategy depends on provider). Collection
    // name is composed from `embedding_version` so a model rotation
    // (which bumps the version in courses.rs::rotate_embedding) lands
    // chunks in a fresh collection without colliding with the
    // previous-model vectors.
    let collection_name = collection_name(course_id, embedding_version);
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
    // we can mean-pool them for the doc-level KG embedding; one
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
    // chunks landed in Qdrant; the teacher UI / RAG retrieval keys
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
                    "qdrant: collection {} created concurrently by another worker; verifying dims",
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

/// Idempotent keyword payload index on `document_id`; the field we filter
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

    #[test]
    fn collection_name_v1_uses_legacy_unsuffixed_name() {
        // Production collections were created as `course_{id}` before
        // the rotation feature landed. Version=1 must keep matching
        // them so the migration is a no-op for existing data.
        let id = uuid::Uuid::nil();
        assert_eq!(
            collection_name(id, 1),
            "course_00000000-0000-0000-0000-000000000000"
        );
    }

    #[test]
    fn collection_name_v2_plus_uses_suffixed_name() {
        let id = uuid::Uuid::nil();
        assert_eq!(
            collection_name(id, 2),
            "course_00000000-0000-0000-0000-000000000000_v2"
        );
        assert_eq!(
            collection_name(id, 7),
            "course_00000000-0000-0000-0000-000000000000_v7"
        );
    }

    #[test]
    fn collection_name_zero_or_negative_falls_back_to_legacy() {
        // Defensive: a stale row read with version=0 (shouldn't
        // happen given the DEFAULT 1 + NOT NULL on the column) still
        // points at the legacy collection rather than producing
        // `course_{id}_v0`, which has no useful semantics.
        let id = uuid::Uuid::nil();
        assert_eq!(
            collection_name(id, 0),
            "course_00000000-0000-0000-0000-000000000000"
        );
        assert_eq!(
            collection_name(id, -1),
            "course_00000000-0000-0000-0000-000000000000"
        );
    }
}
