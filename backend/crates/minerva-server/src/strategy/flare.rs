use axum::response::sse::Event;
use futures::StreamExt;
use std::sync::Arc;
use tokio::sync::mpsc;

use super::common;
use super::common::RagChunk;
use super::GenerationContext;
use crate::error::AppError;

/// Log-probability threshold below which a token is considered "low confidence".
/// -2.0 corresponds to roughly <13% probability. The FLARE paper uses token
/// probability thresholds; we use logprobs since that's what Cerebras returns.
const LOGPROB_THRESHOLD: f64 = -2.0;

/// Qdrant similarity threshold for FLARE retrieval results.
const SIMILARITY_THRESHOLD: f32 = 0.35;

/// Maximum number of retrieval-triggered restarts to prevent infinite loops.
const MAX_FLARE_RESTARTS: usize = 5;

/// FLARE strategy: Forward-Looking Active REtrieval augmented generation.
///
/// Uses Cerebras logprobs to detect low-confidence tokens. When the model
/// generates a sentence containing uncertain tokens, that sentence is used
/// as a retrieval query. If relevant chunks are found, generation restarts
/// from that point with enriched context.
pub async fn run(ctx: GenerationContext, tx: mpsc::Sender<Result<Event, AppError>>) {
    let http_client = reqwest::Client::new();
    let collection_name = format!("course_{}", ctx.course_id);

    // Initial retrieval using user's question (per the paper)
    let initial_chunks = common::rag_lookup(
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

    let mut all_chunks: Vec<RagChunk> = initial_chunks;
    let mut full_text = String::new();
    let mut total_prompt_tokens = 0i32;
    let mut total_completion_tokens = 0i32;
    let mut restarts = 0usize;

    tracing::info!(
        "flare: starting with {} initial chunks for conversation {}",
        all_chunks.len(),
        ctx.conversation_id
    );

    loop {
        let system = common::build_system_prompt(&ctx.course_name, &ctx.custom_prompt, &all_chunks);
        let mut messages = common::build_chat_messages(&system, &ctx.history);

        if !full_text.is_empty() {
            // Continuation: inject instruction in system prompt, partial text as assistant msg
            if let Some(sys_msg) = messages.first_mut() {
                if let Some(content) = sys_msg.get("content").and_then(|c| c.as_str()) {
                    let new_content = format!(
                        "{}\n\nYou are continuing a response. Additional relevant context has been retrieved. Continue seamlessly from where the response left off. Do not repeat anything already written.",
                        content,
                    );
                    sys_msg["content"] = serde_json::Value::String(new_content);
                }
            }
            messages.push(serde_json::json!({
                "role": "assistant",
                "content": full_text,
            }));
        }

        // Stream with logprobs, checking confidence per sentence
        let result = stream_with_logprobs(
            &http_client,
            &ctx.cerebras_api_key,
            &ctx.model,
            ctx.temperature,
            &messages,
            &tx,
            &mut full_text,
        )
        .await;

        let outcome = match result {
            Ok(o) => o,
            Err(e) => {
                tracing::error!("flare: stream error: {}", e);
                let _ = tx
                    .send(Ok(Event::default().data(
                        serde_json::json!({"type": "error", "error": e}).to_string(),
                    )))
                    .await;
                break;
            }
        };

        total_prompt_tokens += outcome.prompt_tokens;
        total_completion_tokens += outcome.completion_tokens;

        if let StreamOutcome::LowConfidenceSentence { sentence } = &outcome.kind {
            if restarts >= MAX_FLARE_RESTARTS {
                tracing::info!(
                    "flare: max restarts ({}) reached, finishing",
                    MAX_FLARE_RESTARTS
                );
                // Let the stream finish naturally on the next iteration
                // by running one more time without checking confidence
                let final_result = common::stream_cerebras_to_client(
                    &http_client,
                    &ctx.cerebras_api_key,
                    &ctx.model,
                    ctx.temperature,
                    &messages,
                    &tx,
                    &mut full_text,
                )
                .await;
                if let Ok((pt, ct)) = final_result {
                    total_prompt_tokens += pt;
                    total_completion_tokens += ct;
                }
                break;
            }

            tracing::info!(
                "flare: low-confidence sentence detected, retrieving (restart {}/{})",
                restarts + 1,
                MAX_FLARE_RESTARTS
            );

            // Use the low-confidence sentence as a retrieval query
            // Mid-stream FLARE retrieval keeps its own paper-derived floor
            // (SIMILARITY_THRESHOLD) but tightens to the course's configured
            // min_score when the teacher has set a stricter value.
            let flare_threshold = SIMILARITY_THRESHOLD.max(ctx.min_score);
            let new_chunks = flare_retrieve(
                &http_client,
                &ctx.openai_api_key,
                &ctx.fastembed,
                &ctx.qdrant,
                &collection_name,
                sentence,
                ctx.max_chunks,
                flare_threshold,
                &ctx.embedding_provider,
                &ctx.embedding_model,
            )
            .await;

            let mut added = false;
            for chunk in &new_chunks {
                if all_chunks.len() >= ctx.max_chunks as usize {
                    break;
                }
                if !all_chunks.contains(chunk) {
                    all_chunks.push(chunk.clone());
                    added = true;
                }
            }

            if added {
                restarts += 1;
                tracing::info!(
                    "flare: added new chunks, total now {}. Restarting from {} chars.",
                    all_chunks.len(),
                    full_text.len()
                );
                continue; // Restart generation with new context
            }

            tracing::debug!("flare: no new chunks found, continuing without restart");
            continue; // Keep generating - just no new context to add
        }

        // StreamOutcome::Completed - model finished naturally
        break;
    }

    let hidden = minerva_db::queries::documents::hidden_document_ids(&ctx.db, ctx.course_id)
        .await
        .unwrap_or_default();
    let chunks_json = if all_chunks.is_empty() {
        None
    } else {
        let client_chunks = common::chunks_for_client(&all_chunks, &hidden);
        serde_json::to_value(&client_chunks).ok()
    };

    // Estimate tokens for interrupted streams
    if total_prompt_tokens == 0 && !full_text.is_empty() {
        total_completion_tokens = (full_text.len() / 4) as i32;
    }

    common::finalize(
        &ctx,
        &tx,
        &full_text,
        chunks_json.as_ref(),
        total_prompt_tokens,
        total_completion_tokens,
        !all_chunks.is_empty(),
    )
    .await;
}

struct StreamWithLogprobsResult {
    prompt_tokens: i32,
    completion_tokens: i32,
    kind: StreamOutcome,
}

enum StreamOutcome {
    /// Model finished generating naturally
    Completed,
    /// A sentence with low-confidence tokens was detected
    LowConfidenceSentence { sentence: String },
}

/// Stream from Cerebras with logprobs enabled.
/// Buffers tokens into sentences. When a sentence boundary is hit, checks
/// if any token in that sentence had logprob below the threshold.
/// If so, returns the sentence for retrieval. Otherwise keeps streaming.
async fn stream_with_logprobs(
    client: &reqwest::Client,
    api_key: &str,
    model: &str,
    temperature: f64,
    messages: &[serde_json::Value],
    tx: &mpsc::Sender<Result<Event, AppError>>,
    full_text: &mut String,
) -> Result<StreamWithLogprobsResult, String> {
    let body = serde_json::json!({
        "model": model,
        "messages": messages,
        "temperature": temperature,
        "stream": true,
        "logprobs": true,
        "top_logprobs": 1,
        "stream_options": { "include_usage": true },
    });

    let response = common::cerebras_request_with_retry(client, api_key, &body).await?;

    let mut stream = response.bytes_stream();
    let mut sse_buffer = String::new();
    let mut sentence_buffer = String::new();
    let mut sentence_has_low_confidence = false;
    let mut prompt_tokens = 0i32;
    let mut completion_tokens = 0i32;

    while let Some(chunk) = stream.next().await {
        let chunk = match chunk {
            Ok(c) => c,
            Err(e) => return Err(format!("Stream error: {}", e)),
        };
        sse_buffer.push_str(&String::from_utf8_lossy(&chunk));

        while let Some(line_end) = sse_buffer.find('\n') {
            let line = sse_buffer[..line_end].trim().to_string();
            sse_buffer = sse_buffer[line_end + 1..].to_string();

            if line == "data: [DONE]" {
                return Ok(StreamWithLogprobsResult {
                    prompt_tokens,
                    completion_tokens,
                    kind: StreamOutcome::Completed,
                });
            }

            if let Some(data) = line.strip_prefix("data: ") {
                if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(data) {
                    if let Some(err) = parsed.get("error") {
                        let msg = err["message"]
                            .as_str()
                            .unwrap_or("unknown error")
                            .to_string();
                        return Err(msg);
                    }

                    // Extract token content and logprob
                    if let Some(choice) = parsed["choices"].get(0) {
                        if let Some(delta_content) = choice["delta"]["content"].as_str() {
                            full_text.push_str(delta_content);
                            sentence_buffer.push_str(delta_content);

                            // Stream to client immediately
                            if tx
                                .send(Ok(Event::default().data(
                                    serde_json::json!({"type": "token", "token": delta_content})
                                        .to_string(),
                                )))
                                .await
                                .is_err()
                            {
                                return Err("client disconnected".to_string());
                            }

                            // Check logprob for this token
                            if let Some(logprobs) = choice.get("logprobs") {
                                if let Some(content_logprobs) = logprobs["content"].as_array() {
                                    for lp in content_logprobs {
                                        if let Some(logprob) = lp["logprob"].as_f64() {
                                            if logprob < LOGPROB_THRESHOLD {
                                                sentence_has_low_confidence = true;
                                            }
                                        }
                                    }
                                }
                            }

                            // Check for sentence boundary
                            if is_sentence_boundary(&sentence_buffer) {
                                if sentence_has_low_confidence {
                                    tracing::debug!(
                                        "flare: low-confidence sentence: {:?}",
                                        truncate_for_log(&sentence_buffer, 80)
                                    );
                                    return Ok(StreamWithLogprobsResult {
                                        prompt_tokens,
                                        completion_tokens,
                                        kind: StreamOutcome::LowConfidenceSentence {
                                            sentence: sentence_buffer,
                                        },
                                    });
                                }
                                // Sentence was confident, reset buffer for next sentence
                                sentence_buffer.clear();
                                sentence_has_low_confidence = false;
                            }
                        }
                    }

                    // Extract usage from final chunk
                    if let Some(usage) = parsed.get("usage") {
                        if !usage.is_null() {
                            prompt_tokens = usage["prompt_tokens"].as_i64().unwrap_or(0) as i32;
                            completion_tokens =
                                usage["completion_tokens"].as_i64().unwrap_or(0) as i32;
                        }
                    }
                }
            }
        }
    }

    Ok(StreamWithLogprobsResult {
        prompt_tokens,
        completion_tokens,
        kind: StreamOutcome::Completed,
    })
}

/// Check for a sentence/paragraph boundary in the buffer.
/// Avoids triggering inside markdown tables, code blocks, or lists.
fn is_sentence_boundary(text: &str) -> bool {
    if text.len() < 100 {
        return false;
    }

    // Don't trigger inside markdown tables (lines starting with |)
    if let Some(last_line) = text.lines().last() {
        let trimmed_line = last_line.trim();
        if trimmed_line.starts_with('|') || trimmed_line.starts_with("|-") {
            return false;
        }
    }

    // Don't trigger inside code blocks
    let fence_count = text.matches("```").count();
    if !fence_count.is_multiple_of(2) {
        return false; // Inside an unclosed code block
    }

    // Don't trigger inside markdown lists (last line starts with - or *)
    if let Some(last_line) = text.lines().last() {
        let trimmed_line = last_line.trim();
        if trimmed_line.starts_with("- ") || trimmed_line.starts_with("* ") {
            return false;
        }
    }

    // Paragraph break is always a good boundary
    if text.ends_with("\n\n") {
        return true;
    }

    // For long buffers, check if we ended a sentence cleanly
    if text.len() > 300 {
        let trimmed = text.trim_end();
        let last_char = trimmed.chars().last().unwrap_or(' ');
        if last_char == '.' || last_char == '?' || last_char == '!' {
            // Make sure we're not inside a table row or list
            if let Some(last_line) = trimmed.lines().last() {
                let ll = last_line.trim();
                if !ll.starts_with('|') && !ll.starts_with("- ") && !ll.starts_with("* ") {
                    return true;
                }
            }
        }
    }

    false
}

/// Search Qdrant with a similarity threshold for FLARE retrieval.
#[allow(clippy::too_many_arguments)]
async fn flare_retrieve(
    client: &reqwest::Client,
    openai_key: &str,
    fastembed: &Arc<minerva_ingest::fastembed_embedder::FastEmbedder>,
    qdrant: &Arc<qdrant_client::Qdrant>,
    collection_name: &str,
    query: &str,
    max_chunks: i32,
    score_threshold: f32,
    embedding_provider: &str,
    embedding_model: &str,
) -> Vec<RagChunk> {
    match common::embedding_search(
        client,
        openai_key,
        fastembed,
        qdrant,
        collection_name,
        query,
        max_chunks as u64,
        Some(score_threshold),
        embedding_provider,
        embedding_model,
    )
    .await
    {
        Ok(points) => points
            .iter()
            .filter_map(|point| {
                let chunk = common::scored_point_to_rag_chunk(point)?;
                tracing::debug!(
                    "flare: retrieved chunk from '{}' score {:.3}",
                    chunk.filename,
                    point.score
                );
                Some(chunk)
            })
            .collect(),
        Err(e) => {
            tracing::warn!("flare: {}", e);
            Vec::new()
        }
    }
}

fn truncate_for_log(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}...", &s[..max_len])
    }
}
