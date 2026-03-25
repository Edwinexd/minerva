use axum::response::sse::Event;
use tokio::sync::mpsc;

use super::common;
use super::GenerationContext;
use crate::error::AppError;

/// Simple strategy: traditional RAG.
/// 1. Embed query
/// 2. Search Qdrant
/// 3. Build prompt with context
/// 4. Stream from Cerebras
pub async fn run(ctx: GenerationContext, tx: mpsc::Sender<Result<Event, AppError>>) {
    let http_client = reqwest::Client::new();
    let collection_name = format!("course_{}", ctx.course_id);

    // RAG lookup (blocks before streaming starts)
    let chunks = common::rag_lookup(
        &http_client,
        &ctx.openai_api_key,
        &ctx.qdrant,
        &collection_name,
        &ctx.user_content,
        ctx.max_chunks,
    )
    .await;

    let hidden = minerva_db::queries::documents::hidden_document_ids(&ctx.db, ctx.course_id)
        .await
        .unwrap_or_default();
    let client_chunks = common::chunks_for_client(&chunks, &hidden);
    let chunks_json = serde_json::to_value(&client_chunks).ok();

    let system = common::build_system_prompt(&ctx.course_name, &ctx.custom_prompt, &chunks);
    let messages = common::build_chat_messages(&system, &ctx.history);

    let mut full_text = String::new();
    let (prompt_tokens, completion_tokens) = match common::stream_cerebras_to_client(
        &http_client,
        &ctx.cerebras_api_key,
        &ctx.model,
        ctx.temperature,
        &messages,
        &tx,
        &mut full_text,
    )
    .await
    {
        Ok(usage) => usage,
        Err(e) => {
            let _ = tx
                .send(Ok(Event::default().data(
                    serde_json::json!({"type": "error", "error": e}).to_string(),
                )))
                .await;
            return;
        }
    };

    common::finalize(
        &ctx,
        &tx,
        &full_text,
        chunks_json.as_ref(),
        prompt_tokens,
        completion_tokens,
        true,
    )
    .await;
}
