use axum::response::sse::Event;
use futures::StreamExt;
use qdrant_client::qdrant::SearchPointsBuilder;
use std::sync::Arc;
use tokio::sync::mpsc;

use super::common;
use super::GenerationContext;
use crate::error::AppError;

/// Similarity threshold for FLARE retrieval-triggered regeneration.
const SIMILARITY_THRESHOLD: f32 = 0.35;

/// Maximum number of FLARE retrieval iterations to prevent infinite loops.
const MAX_FLARE_ITERATIONS: usize = 10;

/// FLARE strategy: Forward-Looking Active REtrieval augmented generation.
///
/// Generates text by streaming from Cerebras, buffering until sentence boundaries.
/// After each sentence, the generated sentence is used as a retrieval query against
/// Qdrant. If high-similarity results are found (above SIMILARITY_THRESHOLD), the
/// prompt is rebuilt with retrieved context and generation continues from that point.
///
/// This differs from simple RAG (which retrieves once using the user question) by
/// using the model's own generated text as retrieval queries, catching cases where
/// the model needs information it didn't know to ask for.
pub async fn run(ctx: GenerationContext, tx: mpsc::Sender<Result<Event, AppError>>) {
    let http_client = reqwest::Client::new();
    let collection_name = format!("course_{}", ctx.course_id);

    // Per the FLARE paper: start with an initial retrieval using the user's question
    let initial_chunks = common::rag_lookup(
        &http_client,
        &ctx.openai_api_key,
        &ctx.qdrant,
        &collection_name,
        &ctx.user_content,
        ctx.max_chunks,
    )
    .await;

    let mut all_chunks: Vec<String> = initial_chunks;
    let mut full_text = String::new();
    let mut total_prompt_tokens = 0i32;
    let mut total_completion_tokens = 0i32;
    let mut flare_iterations = 0usize;

    tracing::info!(
        "flare: starting generation for conversation {} with {} initial chunks",
        ctx.conversation_id,
        all_chunks.len()
    );

    loop {
        if flare_iterations >= MAX_FLARE_ITERATIONS {
            tracing::info!(
                "flare: reached max iterations ({}), finishing",
                MAX_FLARE_ITERATIONS
            );
            break;
        }
        flare_iterations += 1;

        // Build the prompt with whatever chunks we have so far
        let system = common::build_system_prompt(&ctx.course_name, &ctx.custom_prompt, &all_chunks);
        let mut messages = common::build_chat_messages(&system, &ctx.history);

        // If we already have partial output, append it to the last user message
        // as context so the model continues naturally. We frame it as:
        // "You already started answering with: <partial>. Continue from where you left off."
        if !full_text.is_empty() {
            messages.push(serde_json::json!({
                "role": "assistant",
                "content": full_text,
            }));
            messages.push(serde_json::json!({
                "role": "user",
                "content": "Continue your response from exactly where you left off. Do not repeat what you already said.",
            }));
        }

        tracing::debug!(
            "flare: iteration {} - streaming with {} chunks, {} chars so far",
            flare_iterations,
            all_chunks.len(),
            full_text.len()
        );

        // Stream from Cerebras, buffering tokens and detecting sentence boundaries
        let result = stream_until_sentence(
            &http_client,
            &ctx.cerebras_api_key,
            &ctx.model,
            ctx.temperature,
            &messages,
            &tx,
            &mut full_text,
        )
        .await;

        let (sentence, prompt_tokens, completion_tokens, completed) = match result {
            Ok(r) => r,
            Err(e) => {
                tracing::error!("flare: streaming error: {}", e);
                let _ = tx
                    .send(Ok(Event::default().data(
                        serde_json::json!({"type": "error", "error": e}).to_string(),
                    )))
                    .await;
                return;
            }
        };

        total_prompt_tokens += prompt_tokens;
        total_completion_tokens += completion_tokens;

        // If the stream completed (model finished generating), we are done
        if completed {
            tracing::info!(
                "flare: generation completed after {} iterations, {} total chars",
                flare_iterations,
                full_text.len()
            );
            break;
        }

        // Use the generated sentence as a retrieval query
        let trimmed_sentence = sentence.trim();
        if trimmed_sentence.is_empty() {
            continue;
        }

        tracing::debug!(
            "flare: checking retrieval for sentence: {:?}",
            truncate_for_log(trimmed_sentence, 100)
        );

        let new_chunks = flare_retrieve(
            &http_client,
            &ctx.openai_api_key,
            &ctx.qdrant,
            &collection_name,
            trimmed_sentence,
            ctx.max_chunks,
        )
        .await;

        if new_chunks.is_empty() {
            tracing::debug!("flare: no relevant chunks found, continuing generation");
            continue;
        }

        // We found relevant chunks -- add any new ones to context
        let mut added_new = false;
        for chunk in &new_chunks {
            if !all_chunks.contains(chunk) {
                all_chunks.push(chunk.clone());
                added_new = true;
            }
        }

        if added_new {
            tracing::info!(
                "flare: found {} new relevant chunks, total context now {} chunks",
                new_chunks.len(),
                all_chunks.len()
            );
            // Loop back and continue generation with the new context.
            // The full_text already includes everything streamed so far, and on the
            // next iteration we pass it as assistant prefix, so the model continues
            // from where it was but now has the retrieved context available.
        } else {
            tracing::debug!("flare: retrieved chunks already in context, continuing");
        }
    }

    let chunks_json = if all_chunks.is_empty() {
        None
    } else {
        serde_json::to_value(&all_chunks).ok()
    };

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

/// Stream from Cerebras and buffer until a sentence boundary is detected.
///
/// Returns `(sentence, prompt_tokens, completion_tokens, completed)` where:
/// - `sentence` is the text of the last buffered segment (up to the sentence boundary)
/// - `completed` is true if the model finished generating (stream ended)
///
/// All tokens are forwarded to the client via `tx` as they arrive.
/// `full_text` is appended with all generated tokens.
async fn stream_until_sentence(
    client: &reqwest::Client,
    api_key: &str,
    model: &str,
    temperature: f64,
    messages: &[serde_json::Value],
    tx: &mpsc::Sender<Result<Event, AppError>>,
    full_text: &mut String,
) -> Result<(String, i32, i32, bool), String> {
    let body = serde_json::json!({
        "model": model,
        "messages": messages,
        "temperature": temperature,
        "stream": true,
        "stream_options": { "include_usage": true },
    });

    let response = client
        .post("https://api.cerebras.ai/v1/chat/completions")
        .header("Authorization", format!("Bearer {}", api_key))
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("Request failed: {}", e))?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(format!("Cerebras API error {}: {}", status, body));
    }

    let mut stream = response.bytes_stream();
    let mut sse_buffer = String::new();
    let mut sentence_buffer = String::new();
    let mut prompt_tokens = 0i32;
    let mut completion_tokens = 0i32;

    while let Some(chunk) = stream.next().await {
        let chunk = match chunk {
            Ok(c) => c,
            Err(e) => {
                tracing::error!("flare: stream error: {}", e);
                return Err(format!("Stream interrupted: {}", e));
            }
        };
        sse_buffer.push_str(&String::from_utf8_lossy(&chunk));

        while let Some(line_end) = sse_buffer.find('\n') {
            let line = sse_buffer[..line_end].trim().to_string();
            sse_buffer = sse_buffer[line_end + 1..].to_string();

            if line == "data: [DONE]" {
                return Ok((sentence_buffer, prompt_tokens, completion_tokens, true));
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

                    if let Some(delta) = parsed["choices"][0]["delta"]["content"].as_str() {
                        full_text.push_str(delta);
                        sentence_buffer.push_str(delta);

                        // Stream token to client immediately
                        if tx
                            .send(Ok(Event::default().data(
                                serde_json::json!({"type": "token", "token": delta}).to_string(),
                            )))
                            .await
                            .is_err()
                        {
                            return Err("client disconnected".to_string());
                        }

                        // Check if we hit a sentence boundary
                        if has_sentence_boundary(&sentence_buffer) {
                            return Ok((sentence_buffer, prompt_tokens, completion_tokens, false));
                        }
                    }

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

    // Stream ended without [DONE] marker
    Ok((sentence_buffer, prompt_tokens, completion_tokens, true))
}

/// Check if the buffer contains a complete paragraph/section boundary.
///
/// We use paragraph breaks (\n\n) as the primary boundary, not sentence-ending
/// punctuation like ". ", because that triggers too aggressively inside markdown
/// tables, lists, and other structured content. Paragraph breaks are more
/// reliable semantic boundaries for retrieval decisions.
///
/// Also requires a minimum buffer size to avoid triggering on very short fragments.
fn has_sentence_boundary(text: &str) -> bool {
    // Minimum chars before we even consider checking -- avoids micro-fragments
    if text.len() < 100 {
        return false;
    }

    // Primary: paragraph break (double newline)
    if text.contains("\n\n") {
        return true;
    }

    // Secondary: if we have accumulated a lot of text without a paragraph break,
    // check for sentence-ending punctuation at the very end of the buffer.
    // This handles cases where the model generates long single paragraphs.
    if text.len() > 300 {
        let trimmed = text.trim_end();
        if trimmed.ends_with(". ")
            || trimmed.ends_with(".\n")
            || trimmed.ends_with('.')
            || trimmed.ends_with("? ")
            || trimmed.ends_with("?\n")
            || trimmed.ends_with('?')
            || trimmed.ends_with("! ")
            || trimmed.ends_with("!\n")
            || trimmed.ends_with('!')
        {
            return true;
        }
    }

    false
}

/// Embed a sentence and search Qdrant for similar chunks above the similarity threshold.
/// Returns chunk texts only if results meet the threshold.
async fn flare_retrieve(
    client: &reqwest::Client,
    openai_key: &str,
    qdrant: &Arc<qdrant_client::Qdrant>,
    collection_name: &str,
    query: &str,
    max_chunks: i32,
) -> Vec<String> {
    let embedding = match minerva_ingest::embedder::embed_texts(
        client,
        openai_key,
        std::slice::from_ref(&query.to_string()),
    )
    .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!("flare: embedding failed: {}, skipping retrieval", e);
            return Vec::new();
        }
    };

    let vector = match embedding.embeddings.into_iter().next() {
        Some(v) => v,
        None => return Vec::new(),
    };

    // Search with score_threshold so we only get high-confidence results
    let search_result = qdrant
        .search_points(
            SearchPointsBuilder::new(collection_name, vector, max_chunks as u64)
                .with_payload(true)
                .score_threshold(SIMILARITY_THRESHOLD),
        )
        .await;

    let result = match search_result {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!("flare: qdrant search failed: {}, skipping retrieval", e);
            return Vec::new();
        }
    };

    result
        .result
        .iter()
        .filter_map(|point| {
            let payload = &point.payload;
            let text = match payload.get("text").and_then(|v| v.kind.as_ref()) {
                Some(qdrant_client::qdrant::value::Kind::StringValue(s)) => s.clone(),
                _ => return None,
            };
            let filename = match payload.get("filename").and_then(|v| v.kind.as_ref()) {
                Some(qdrant_client::qdrant::value::Kind::StringValue(s)) => s.clone(),
                _ => String::new(),
            };
            tracing::debug!(
                "flare: retrieved chunk from '{}' with score {:.3}",
                filename,
                point.score
            );
            Some(format!("[Source: {}]\n{}", filename, text))
        })
        .collect()
}

/// Truncate a string for logging purposes.
fn truncate_for_log(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}...", &s[..max_len])
    }
}
