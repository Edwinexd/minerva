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
    let started_at = std::time::Instant::now();
    let http_client = reqwest::Client::new();
    let collection_name = format!("course_{}", ctx.course_id);

    // RAG lookup (blocks before streaming starts)
    let raw_chunks = common::rag_lookup(
        &http_client,
        &ctx.openai_api_key,
        &ctx.fastembed,
        &ctx.qdrant,
        &collection_name,
        &ctx.user_content,
        ctx.max_chunks,
        ctx.min_score,
        &ctx.embedding_provider,
        &ctx.embedding_model,
    )
    .await;

    // Kind-aware partition: assignment_brief / lab_brief / exam matches
    // become `signals` (the model gets a refusal addendum but never the
    // chunk text); sample_solution leftovers are dropped defensively;
    // unclassified docs are held back for this turn. All gated on
    // `kg_enabled` -- KG-disabled courses bypass the partition and the
    // adversarial filter entirely.
    let unclassified = if ctx.kg_enabled {
        minerva_db::queries::documents::unclassified_doc_ids(&ctx.db, ctx.course_id)
            .await
            .unwrap_or_default()
    } else {
        std::collections::HashSet::new()
    };
    let mut rag = common::partition_chunks(raw_chunks, &unclassified, ctx.kg_enabled);

    // Adversarial pre-retrieval check: drop any per-chunk worked
    // solutions that slipped through the doc-level classifier.
    // Fails open on timeout (see classification::adversarial).
    if ctx.kg_enabled {
        rag.context = crate::classification::adversarial::filter_solution_chunks(
            &http_client,
            &ctx.cerebras_api_key,
            rag.context,
        )
        .await;
    }

    let hidden = minerva_db::queries::documents::hidden_document_ids(&ctx.db, ctx.course_id)
        .await
        .unwrap_or_default();
    // Sources surfaced to the client include both context + signals -- a
    // student should see *that* an assignment matched even though its
    // text is withheld from the model.
    let displayed = rag.all();
    let client_chunks = common::chunks_for_client(&displayed, &hidden);
    let chunks_json = serde_json::to_value(&client_chunks).ok();

    let system = common::build_system_prompt_with_signals(
        &ctx.course_name,
        &ctx.custom_prompt,
        &rag.context,
        &rag.signals,
    );
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
        started_at.elapsed().as_millis() as i64,
        1,
    )
    .await;
}
