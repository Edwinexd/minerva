use std::path::Path;

use qdrant_client::qdrant::{
    CreateCollectionBuilder, Distance, Document, PointStruct, UpsertPointsBuilder,
    VectorParamsBuilder,
};
use qdrant_client::Qdrant;
use sqlx::PgPool;
use uuid::Uuid;

use crate::chunker::{self, ChunkerConfig};
use crate::embedder;
use crate::pdf;

/// Known embedding model dimensions for Qdrant server-side inference.
fn qdrant_model_dimensions(model: &str) -> u64 {
    match model {
        "sentence-transformers/all-MiniLM-L6-v2" => 384,
        "BAAI/bge-small-en-v1.5" => 384,
        "BAAI/bge-base-en-v1.5" => 768,
        "nomic-ai/nomic-embed-text-v1.5" => 768,
        "Qdrant/clip-ViT-B-32-vision" => 512,
        _ => 384, // safe default for most small models
    }
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
    document_id: Uuid,
    course_id: Uuid,
    file_path: &Path,
    filename: &str,
    embedding_provider: &str,
    embedding_model: &str,
) -> Result<ProcessResult, String> {
    // Mark as processing
    set_status(db, document_id, "processing", None).await;

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
    let embedding_tokens = match embedding_provider {
        "qdrant" => {
            // Qdrant server-side inference: no client-side embedding needed
            let dims = qdrant_model_dimensions(embedding_model);
            ensure_collection(qdrant, &collection_name, dims).await?;

            let points: Vec<PointStruct> = chunks
                .iter()
                .map(|chunk| {
                    let point_id = Uuid::new_v4().to_string();
                    let payload: std::collections::HashMap<String, qdrant_client::qdrant::Value> = [
                        ("document_id".to_string(), document_id.to_string().into()),
                        ("course_id".to_string(), course_id.to_string().into()),
                        ("chunk_index".to_string(), (chunk.index as i64).into()),
                        ("text".to_string(), chunk.text.clone().into()),
                        ("filename".to_string(), filename.to_string().into()),
                    ]
                    .into_iter()
                    .collect();

                    PointStruct::new(
                        point_id,
                        Document::new(chunk.text.clone(), embedding_model),
                        payload,
                    )
                })
                .collect();

            // Batch upsert in groups of 100
            for batch in points.chunks(100) {
                qdrant
                    .upsert_points(UpsertPointsBuilder::new(&collection_name, batch.to_vec()))
                    .await
                    .map_err(|e| format!("qdrant upsert failed: {}", e))?;
            }

            tracing::info!(
                "upserted {} chunks via qdrant server-side inference (model: {})",
                chunks.len(),
                embedding_model,
            );

            0i64 // no external API tokens used
        }
        _ => {
            // OpenAI client-side embedding (default)
            ensure_collection(qdrant, &collection_name, 1536).await?;

            let chunk_texts: Vec<String> = chunks.iter().map(|c| c.text.clone()).collect();
            let embedding_result =
                embedder::embed_texts(http_client, openai_api_key, &chunk_texts)
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
                    let point_id = Uuid::new_v4().to_string();
                    let payload: std::collections::HashMap<String, qdrant_client::qdrant::Value> = [
                        ("document_id".to_string(), document_id.to_string().into()),
                        ("course_id".to_string(), course_id.to_string().into()),
                        ("chunk_index".to_string(), (chunk.index as i64).into()),
                        ("text".to_string(), chunk.text.clone().into()),
                        ("filename".to_string(), filename.to_string().into()),
                    ]
                    .into_iter()
                    .collect();

                    PointStruct::new(point_id, embedding.clone(), payload)
                })
                .collect();

            // Batch upsert in groups of 100
            for batch in points.chunks(100) {
                qdrant
                    .upsert_points(UpsertPointsBuilder::new(&collection_name, batch.to_vec()))
                    .await
                    .map_err(|e| format!("qdrant upsert failed: {}", e))?;
            }

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

async fn ensure_collection(qdrant: &Qdrant, name: &str, dimensions: u64) -> Result<(), String> {
    let exists = qdrant
        .collection_exists(name)
        .await
        .map_err(|e| format!("qdrant collection check failed: {}", e))?;

    if !exists {
        qdrant
            .create_collection(
                CreateCollectionBuilder::new(name)
                    .vectors_config(VectorParamsBuilder::new(dimensions, Distance::Cosine)),
            )
            .await
            .map_err(|e| format!("qdrant collection creation failed: {}", e))?;

        tracing::info!("created qdrant collection {} (dimensions: {})", name, dimensions);
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
