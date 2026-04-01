use std::path::Path;
use std::sync::Arc;

use qdrant_client::qdrant::{
    CreateCollectionBuilder, Distance, PointStruct, UpsertPointsBuilder, VectorParamsBuilder,
};
use qdrant_client::Qdrant;
use sqlx::PgPool;
use uuid::Uuid;

use crate::chunker::{self, ChunkerConfig};
use crate::embedder;
use crate::fastembed_embedder::FastEmbedder;
use crate::pdf;

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
/// 1. Extract text from PDF
/// 2. Chunk the text
/// 3. Embed chunks (via OpenAI or Qdrant server-side inference)
/// 4. Upsert to Qdrant
/// 5. Update document status in Postgres
#[allow(clippy::too_many_arguments)]
pub async fn process_document(
    db: &PgPool,
    qdrant: &Qdrant,
    http_client: &reqwest::Client,
    openai_api_key: &str,
    fastembed: &Arc<FastEmbedder>,
    document_id: Uuid,
    course_id: Uuid,
    file_path: &Path,
    filename: &str,
    embedding_provider: &str,
    embedding_model: &str,
) -> Result<ProcessResult, String> {
    // 1. Extract text
    let text = pdf::extract_text(file_path).map_err(|e| {
        let msg = format!("text extraction failed: {}", e);
        tracing::error!("{}", msg);
        msg
    })?;

    tracing::info!("extracted {} chars from {}", text.len(), filename,);

    // 2. Chunk
    let chunks = chunker::chunk_text(&text, &ChunkerConfig::default());
    if chunks.is_empty() {
        let msg = "no chunks produced from document".to_string();
        set_status(db, document_id, "failed", Some(&msg)).await;
        return Err(msg);
    }

    tracing::info!("produced {} chunks from {}", chunks.len(), filename);

    // 3 & 4. Embed + Upsert (strategy depends on provider)
    let collection_name = format!("course_{}", course_id);

    let build_payload = |chunk: &chunker::Chunk| -> std::collections::HashMap<String, qdrant_client::qdrant::Value> {
        [
            ("document_id".to_string(), document_id.to_string().into()),
            ("course_id".to_string(), course_id.to_string().into()),
            ("chunk_index".to_string(), (chunk.index as i64).into()),
            ("text".to_string(), chunk.text.clone().into()),
            ("filename".to_string(), filename.to_string().into()),
        ]
        .into_iter()
        .collect()
    };

    let embedding_tokens = match embedding_provider {
        "local" => {
            let dims = local_model_dimensions(embedding_model)
                .ok_or_else(|| format!("unsupported local embedding model: {}", embedding_model))?;
            ensure_collection(qdrant, &collection_name, dims).await?;

            let chunk_texts: Vec<String> = chunks.iter().map(|c| c.text.clone()).collect();
            let embeddings = fastembed.embed(embedding_model, chunk_texts).await?;

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

            0i64
        }
        _ => {
            ensure_collection(qdrant, &collection_name, OPENAI_EMBEDDING_DIMENSIONS).await?;

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

            embedding_result.total_tokens
        }
    };

    // 5. Update status
    let chunk_count = chunks.len() as i32;
    set_status_ready(db, document_id, chunk_count).await;

    tracing::info!(
        "document {} processed: {} chunks stored in collection {}",
        document_id,
        chunk_count,
        collection_name,
    );

    Ok(ProcessResult {
        chunk_count,
        embedding_tokens,
    })
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

    if exists {
        // Verify dimensions match the existing collection
        let info = qdrant
            .collection_info(name)
            .await
            .map_err(|e| format!("qdrant collection info failed: {}", e))?;

        if let Some(config) = info.result.and_then(|r| r.config) {
            if let Some(params) = config.params {
                let existing_dims = params.vectors_config.and_then(|vc| vc.config).and_then(
                    |config| match config {
                        qdrant_client::qdrant::vectors_config::Config::Params(p) => Some(p.size),
                        _ => None,
                    },
                );
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
    } else {
        qdrant
            .create_collection(
                CreateCollectionBuilder::new(name)
                    .vectors_config(VectorParamsBuilder::new(dimensions, Distance::Cosine)),
            )
            .await
            .map_err(|e| format!("qdrant collection creation failed: {}", e))?;

        tracing::info!(
            "created qdrant collection {} (dimensions: {})",
            name,
            dimensions
        );
    }

    Ok(())
}

async fn set_status(db: &PgPool, doc_id: Uuid, status: &str, error_msg: Option<&str>) {
    let _ = sqlx::query(
        "UPDATE documents SET status = $1, error_msg = $2, updated_at = NOW() WHERE id = $3",
    )
    .bind(status)
    .bind(error_msg)
    .bind(doc_id)
    .execute(db)
    .await;
}

async fn set_status_ready(db: &PgPool, doc_id: Uuid, chunk_count: i32) {
    let _ = sqlx::query(
        "UPDATE documents SET status = 'ready', chunk_count = $1, processed_at = NOW() WHERE id = $2",
    )
    .bind(chunk_count)
    .bind(doc_id)
    .execute(db)
    .await;
}
