use axum::response::sse::Event;
use futures::StreamExt;
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot};

use super::common;
use super::GenerationContext;
use crate::error::AppError;

/// Parallel strategy: start streaming immediately without RAG, inject context mid-stream.
/// 1. Start Cerebras immediately (no context) for instant first token
/// 2. In parallel, embed query + search Qdrant
/// 3. When RAG arrives: abort stream, inject context + partial output, restart to continue
pub async fn run(ctx: GenerationContext, tx: mpsc::Sender<Result<Event, AppError>>) {
    let http_client = reqwest::Client::new();
    let collection_name = format!("course_{}", ctx.course_id);

    // Build initial prompt WITHOUT RAG
    let system_no_rag = common::build_system_prompt(&ctx.course_name, &ctx.custom_prompt, &[]);
    let initial_messages = common::build_chat_messages(&system_no_rag, &ctx.history);

    // Spawn RAG lookup in parallel
    let (rag_tx, mut rag_rx) = oneshot::channel::<Vec<String>>();
    {
        let client = http_client.clone();
        let key = ctx.openai_api_key.clone();
        let qdrant = Arc::clone(&ctx.qdrant);
        let query = ctx.user_content.clone();
        let max_chunks = ctx.max_chunks;
        let coll = collection_name.clone();

        tokio::spawn(async move {
            let chunks =
                common::rag_lookup(&client, &key, &qdrant, &coll, &query, max_chunks).await;
            let _ = rag_tx.send(chunks);
        });
    }

    let mut full_text = String::new();
    let mut total_prompt = 0i32;
    let mut total_completion = 0i32;
    let mut chunks_json: Option<serde_json::Value> = None;
    let mut rag_injected = false;

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
            chunks_json = serde_json::to_value(&rag_chunks).ok();

            tracing::info!(
                "parallel: RAG arrived after {} chars, continuing with {} chunks",
                full_text.len(),
                rag_chunks.len()
            );

            // Build continued prompt with RAG + partial assistant output
            let system_with_rag =
                common::build_system_prompt(&ctx.course_name, &ctx.custom_prompt, &rag_chunks);
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

    common::finalize(
        &ctx,
        &tx,
        &full_text,
        chunks_json.as_ref(),
        total_prompt,
        total_completion,
        rag_injected,
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
        rag_chunks: Vec<String>,
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
    rag_rx: &mut oneshot::Receiver<Vec<String>>,
) -> StreamResult {
    let body = serde_json::json!({
        "model": model,
        "messages": messages,
        "temperature": temperature,
        "stream": true,
        "stream_options": { "include_usage": true },
    });

    let response = match client
        .post("https://api.cerebras.ai/v1/chat/completions")
        .header("Authorization", format!("Bearer {}", api_key))
        .json(&body)
        .send()
        .await
    {
        Ok(r) if r.status().is_success() => r,
        Ok(r) => {
            let status = r.status();
            let body = r.text().await.unwrap_or_default();
            return StreamResult::Error(format!("Cerebras API error {}: {}", status, body));
        }
        Err(e) => return StreamResult::Error(format!("Request failed: {}", e)),
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
