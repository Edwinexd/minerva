use axum::response::sse::Event;
use futures::StreamExt;
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot};

use super::common;
use super::common::RagChunk;
use super::GenerationContext;
use crate::error::AppError;

/// Parallel strategy: start streaming immediately without RAG, inject context mid-stream.
/// 1. Start Cerebras immediately (no context) for instant first token
/// 2. In parallel, embed query + search Qdrant
/// 3. When RAG arrives: abort stream, inject context + partial output, restart to continue
pub async fn run(ctx: GenerationContext, tx: mpsc::Sender<Result<Event, AppError>>) {
    let started_at = std::time::Instant::now();
    let http_client = reqwest::Client::new();
    let collection_name = format!("course_{}", ctx.course_id);

    // Build initial prompt WITHOUT RAG. No chunks → no signals → no
    // addendum yet; if RAG arrives mid-stream we rebuild with the right
    // signals before phase-2.
    let system_no_rag = common::build_system_prompt(&ctx.course_name, &ctx.custom_prompt, &[]);
    let initial_messages = common::build_chat_messages(&system_no_rag, &ctx.history);

    // Spawn RAG lookup in parallel
    let (rag_tx, mut rag_rx) = oneshot::channel::<Vec<RagChunk>>();
    {
        let client = http_client.clone();
        let key = ctx.openai_api_key.clone();
        let fastembed = Arc::clone(&ctx.fastembed);
        let qdrant = Arc::clone(&ctx.qdrant);
        let query = ctx.user_content.clone();
        let max_chunks = ctx.max_chunks;
        let min_score = ctx.min_score;
        let coll = collection_name.clone();
        let emb_provider = ctx.embedding_provider.clone();
        let emb_model = ctx.embedding_model.clone();

        tokio::spawn(async move {
            let chunks = common::rag_lookup(
                &client,
                &key,
                &fastembed,
                &qdrant,
                &coll,
                &query,
                max_chunks,
                min_score,
                &emb_provider,
                &emb_model,
            )
            .await;
            let _ = rag_tx.send(chunks);
        });
    }

    let mut full_text = String::new();
    let mut total_prompt = 0i32;
    let mut total_completion = 0i32;
    let mut chunks_json: Option<serde_json::Value> = None;
    let mut rag_injected = false;
    // Populated inside the `RagArrived` branch below (the only path
    // where we have signals + context to evaluate). Stays None for
    // the no-RAG completion path -- nothing to guard against when
    // the model never saw assignment material.
    let mut guard_decision: Option<super::extraction_guard::GuardDecision> = None;

    // Phase 1: stream without RAG, checking for RAG results between chunks
    let phase1 = stream_with_rag_check(
        &http_client,
        &ctx.cerebras_api_key,
        &ctx.model,
        ctx.temperature,
        &initial_messages,
        &tx,
        &mut full_text,
        &mut rag_rx,
    )
    .await;

    match phase1 {
        StreamResult::Completed {
            prompt_tokens,
            completion_tokens,
        } => {
            total_prompt += prompt_tokens;
            total_completion += completion_tokens;
        }
        StreamResult::RagArrived {
            prompt_tokens,
            completion_tokens,
            rag_chunks,
        } => {
            total_prompt += prompt_tokens;
            total_completion += completion_tokens;
            rag_injected = true;

            // Kind-aware partition before showing anything to the
            // model or to the client. Skipped entirely when the KG
            // feature flag is off for this course -- partition_chunks
            // sees `kg_enabled=false` and returns every chunk as
            // context with no signals, and we don't run the
            // adversarial filter (which is part of the same KG
            // bundle). The unclassified lookup is also skipped to
            // save a roundtrip on the hot chat path.
            let unclassified = if ctx.kg_enabled {
                minerva_db::queries::documents::unclassified_doc_ids(&ctx.db, ctx.course_id)
                    .await
                    .unwrap_or_default()
            } else {
                std::collections::HashSet::new()
            };
            let mut rag = common::partition_chunks(rag_chunks, &unclassified, ctx.kg_enabled);

            // Adversarial pre-retrieval check on context chunks
            // (gated on kg_enabled -- it's defence in depth on top
            // of the per-doc kind classifier, so it only matters
            // when classification is on at all).
            if ctx.kg_enabled {
                rag.context = crate::classification::adversarial::filter_solution_chunks(
                    &http_client,
                    &ctx.cerebras_api_key,
                    &ctx.db,
                    ctx.course_id,
                    rag.context,
                )
                .await;
            }

            // Graph-aware enrichment: pull representative chunks
            // from each top hit's KG partners (part_of_unit / theory
            // -> applied_in dst). Adds material the embedding search
            // alone might have missed -- e.g. a student asking about
            // a concept matches the lecture, the graph adds the
            // lecture's tutorial / section summary as supporting
            // context. Gated on kg_enabled; the helper itself is a
            // best-effort no-op on errors.
            if ctx.kg_enabled {
                let collection_name = format!("course_{}", ctx.course_id);
                let extra = common::expand_context_via_graph(
                    &ctx.db,
                    &ctx.qdrant,
                    &ctx.fastembed,
                    &http_client,
                    &ctx.openai_api_key,
                    ctx.course_id,
                    &collection_name,
                    &ctx.embedding_provider,
                    &ctx.embedding_model,
                    &ctx.user_content,
                    &rag.context,
                )
                .await;
                rag.context.extend(extra);
            }

            let hidden =
                minerva_db::queries::documents::hidden_document_ids(&ctx.db, ctx.course_id)
                    .await
                    .unwrap_or_default();
            let displayed = rag.all();
            let client_chunks = common::chunks_for_client(&displayed, &hidden);
            chunks_json = serde_json::to_value(&client_chunks).ok();

            tracing::info!(
                "parallel: RAG arrived after {} chars, continuing with {} context chunk(s) and {} signal(s)",
                full_text.len(),
                rag.context.len(),
                rag.signals.len(),
            );

            // Extraction guard evaluation: piggybacks on the just-
            // finished RAG (signals + context) to decide whether
            // this turn's reply needs the post-generation output
            // check. Stashed for use after phase-2 streaming. None
            // when the feature flag is off; Some(_) otherwise. Same
            // contract as simple.rs / flare.rs.
            guard_decision = super::extraction_guard::evaluate_for_turn(
                &ctx.db,
                &http_client,
                &ctx.cerebras_api_key,
                ctx.course_id,
                ctx.conversation_id,
                &ctx.history,
                &ctx.user_content,
                &rag.signals,
                &rag.context,
            )
            .await;

            // Build continued prompt with RAG context + signal-driven
            // refusal addendum (when applicable) + partial assistant output.
            let system_with_rag = common::build_system_prompt_with_signals(
                &ctx.course_name,
                &ctx.custom_prompt,
                &rag.context,
                &rag.signals,
            );
            let mut continued = common::build_chat_messages(&system_with_rag, &ctx.history);

            if !full_text.is_empty() {
                // Add continuation instruction to system prompt
                if let Some(sys_msg) = continued.first_mut() {
                    if let Some(content) = sys_msg.get("content").and_then(|c| c.as_str()) {
                        let new_content = format!(
                            "{}\n\nYou are continuing a response that was already started. Additional context has been retrieved. Continue seamlessly from where the response left off. Do not repeat anything already written.",
                            content,
                        );
                        sys_msg["content"] = serde_json::Value::String(new_content);
                    }
                }
                continued.push(serde_json::json!({
                    "role": "assistant",
                    "content": full_text,
                }));
            }

            // Phase 2: continue with RAG context
            match common::stream_cerebras_to_client(
                &http_client,
                &ctx.cerebras_api_key,
                &ctx.model,
                ctx.temperature,
                &continued,
                &tx,
                &mut full_text,
            )
            .await
            {
                Ok((pt, ct)) => {
                    total_prompt += pt;
                    total_completion += ct;
                }
                Err(e) => {
                    let _ = tx
                        .send(Ok(Event::default().data(
                            serde_json::json!({"type": "error", "error": e}).to_string(),
                        )))
                        .await;
                }
            }
        }
        StreamResult::Error(e) => {
            let _ = tx
                .send(Ok(Event::default().data(
                    serde_json::json!({"type": "error", "error": e}).to_string(),
                )))
                .await;
        }
    }

    // Post-generation extraction-guard intercept. No-op when the
    // guard wasn't enabled (decision is None) or when the
    // constraint isn't active for this turn. Otherwise: runs the
    // output-side check, generates a Socratic rewrite if positive,
    // emits an SSE `rewrite` event so the frontend can swap the
    // streamed message, logs a `conversation_flag`, and returns the
    // text that should be persisted (original or rewrite).
    let final_text = super::extraction_guard::intercept_reply(
        &ctx.db,
        &http_client,
        &ctx.cerebras_api_key,
        ctx.course_id,
        ctx.conversation_id,
        &guard_decision,
        &ctx.user_content,
        &full_text,
        &tx,
    )
    .await;

    common::finalize(
        &ctx,
        &tx,
        &final_text,
        chunks_json.as_ref(),
        total_prompt,
        total_completion,
        rag_injected,
        started_at.elapsed().as_millis() as i64,
        if rag_injected { 1 } else { 0 },
    )
    .await;
}

enum StreamResult {
    Completed {
        prompt_tokens: i32,
        completion_tokens: i32,
    },
    RagArrived {
        prompt_tokens: i32,
        completion_tokens: i32,
        rag_chunks: Vec<RagChunk>,
    },
    Error(String),
}

/// Stream from Cerebras while checking if RAG results have arrived.
/// Uses tokio::select! to race between stream chunks and RAG completion.
#[allow(clippy::too_many_arguments)]
async fn stream_with_rag_check(
    client: &reqwest::Client,
    api_key: &str,
    model: &str,
    temperature: f64,
    messages: &[serde_json::Value],
    tx: &mpsc::Sender<Result<Event, AppError>>,
    full_text: &mut String,
    rag_rx: &mut oneshot::Receiver<Vec<RagChunk>>,
) -> StreamResult {
    let body = serde_json::json!({
        "model": model,
        "messages": messages,
        "temperature": temperature,
        "stream": true,
        "stream_options": { "include_usage": true },
    });

    let response = match common::cerebras_request_with_retry(client, api_key, &body).await {
        Ok(r) => r,
        Err(e) => return StreamResult::Error(e),
    };

    let mut stream = response.bytes_stream();
    let mut buffer = String::new();
    let mut prompt_tokens = 0i32;
    let mut completion_tokens = 0i32;

    loop {
        tokio::select! {
            rag_result = &mut *rag_rx => {
                if let Ok(chunks) = rag_result {
                    if !chunks.is_empty() {
                        return StreamResult::RagArrived {
                            prompt_tokens, completion_tokens, rag_chunks: chunks,
                        };
                    }
                }
            }
            chunk = stream.next() => {
                let chunk = match chunk {
                    Some(Ok(c)) => c,
                    Some(Err(e)) => return StreamResult::Error(format!("Stream error: {}", e)),
                    None => return StreamResult::Completed { prompt_tokens, completion_tokens },
                };

                buffer.push_str(&String::from_utf8_lossy(&chunk));

                while let Some(line_end) = buffer.find('\n') {
                    let line = buffer[..line_end].trim().to_string();
                    buffer = buffer[line_end + 1..].to_string();

                    if line == "data: [DONE]" {
                        return StreamResult::Completed { prompt_tokens, completion_tokens };
                    }

                    if let Some(data) = line.strip_prefix("data: ") {
                        if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(data) {
                            if let Some(err) = parsed.get("error") {
                                let msg = err["message"].as_str().unwrap_or("unknown").to_string();
                                return StreamResult::Error(msg);
                            }

                            if let Some(delta) = parsed["choices"][0]["delta"]["content"].as_str() {
                                full_text.push_str(delta);
                                if tx.send(Ok(Event::default().data(
                                    serde_json::json!({"type": "token", "token": delta}).to_string()
                                ))).await.is_err() {
                                    return StreamResult::Error("client disconnected".to_string());
                                }
                            }

                            if let Some(usage) = parsed.get("usage") {
                                if !usage.is_null() {
                                    prompt_tokens = usage["prompt_tokens"].as_i64().unwrap_or(0) as i32;
                                    completion_tokens = usage["completion_tokens"].as_i64().unwrap_or(0) as i32;
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
