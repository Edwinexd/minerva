use axum::response::sse::Event;
use futures::StreamExt;
use qdrant_client::qdrant::SearchPointsBuilder;
use reqwest::Response;
use std::collections::HashSet;
use tokio::sync::mpsc;

use crate::error::AppError;

/// Maximum number of retries for transient Cerebras API errors (5XX, timeouts).
const MAX_RETRIES: u32 = 3;

/// Initial backoff delay between retries.
const INITIAL_BACKOFF: std::time::Duration = std::time::Duration::from_millis(500);

/// A chunk returned by RAG lookup, carrying metadata for display filtering.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RagChunk {
    pub document_id: String,
    pub filename: String,
    pub text: String,
}

impl RagChunk {
    /// Format for inclusion in the LLM system prompt (always full text).
    pub fn formatted(&self) -> String {
        format!("[Source: {}]\n{}", self.filename, self.text)
    }
}

/// Build the list of chunk strings to send to the client/store in DB.
/// Non-displayable sources have their text stripped.
pub fn chunks_for_client(chunks: &[RagChunk], hidden_doc_ids: &HashSet<String>) -> Vec<String> {
    chunks
        .iter()
        .map(|c| {
            if hidden_doc_ids.contains(&c.document_id) {
                format!("[Source: {}]", c.filename)
            } else {
                c.formatted()
            }
        })
        .collect()
}

/// Send a request to the Cerebras API with retry on 5XX / network errors.
/// Returns the successful response or the last error as a formatted string.
pub async fn cerebras_request_with_retry(
    client: &reqwest::Client,
    api_key: &str,
    body: &serde_json::Value,
) -> Result<Response, String> {
    let mut last_err = String::new();

    for attempt in 0..=MAX_RETRIES {
        if attempt > 0 {
            let backoff = INITIAL_BACKOFF * 2u32.pow(attempt - 1);
            tracing::warn!(
                "cerebras: retry {}/{} after {:?}",
                attempt,
                MAX_RETRIES,
                backoff
            );
            tokio::time::sleep(backoff).await;
        }

        let result = client
            .post("https://api.cerebras.ai/v1/chat/completions")
            .header("Authorization", format!("Bearer {}", api_key))
            .json(body)
            .send()
            .await;

        match result {
            Ok(response) => {
                let status = response.status();
                if status.is_success() {
                    return Ok(response);
                }
                if status.is_server_error() {
                    let body_text = response.text().await.unwrap_or_default();
                    last_err = format!("Cerebras API error {}: {}", status, body_text);
                    tracing::warn!("cerebras: {}", last_err);
                    continue;
                }
                // Client errors (4XX) are not retryable
                let body_text = response.text().await.unwrap_or_default();
                return Err(format!("Cerebras API error {}: {}", status, body_text));
            }
            Err(e) if e.is_timeout() || e.is_connect() => {
                last_err = format!("Request failed: {}", e);
                tracing::warn!("cerebras: {}", last_err);
                continue;
            }
            Err(e) => {
                return Err(format!("Request failed: {}", e));
            }
        }
    }

    Err(last_err)
}

/// Build the system prompt with optional RAG chunks.
/// When chunks are empty (e.g. parallel phase 1), uses a generic prompt
/// that doesn't tell the model to refuse -- since context may arrive later.
pub fn build_system_prompt(
    course_name: &str,
    custom_prompt: &Option<String>,
    chunks: &[RagChunk],
) -> String {
    let mut prompt = if chunks.is_empty() {
        format!(
            "You are a helpful teaching assistant for the course '{}'. Answer the student's question to the best of your ability.",
            course_name
        )
    } else {
        format!(
            "You are a helpful teaching assistant for the course '{}'. Answer questions based on the provided course materials. If the answer is not in the materials, say so clearly.",
            course_name
        )
    };

    if let Some(ref custom) = custom_prompt {
        prompt.push_str("\n\nAdditional instructions: ");
        prompt.push_str(custom);
    }

    if !chunks.is_empty() {
        prompt.push_str("\n\nRelevant course materials:\n---\n");
        let formatted: Vec<String> = chunks.iter().map(|c| c.formatted()).collect();
        prompt.push_str(&formatted.join("\n---\n"));
        prompt.push_str("\n---");
    }

    prompt
}

/// Build the chat messages array for the Cerebras API.
pub fn build_chat_messages(
    system_prompt: &str,
    history: &[minerva_db::queries::conversations::MessageRow],
) -> Vec<serde_json::Value> {
    let mut messages = vec![serde_json::json!({
        "role": "system",
        "content": system_prompt,
    })];

    for msg in history.iter() {
        messages.push(serde_json::json!({
            "role": msg.role,
            "content": msg.content,
        }));
    }

    messages
}

/// Perform RAG lookup: embed query, search Qdrant, return structured chunks.
/// Supports both OpenAI client-side embedding and Qdrant server-side inference.
pub async fn rag_lookup(
    client: &reqwest::Client,
    openai_key: &str,
    qdrant: &qdrant_client::Qdrant,
    collection_name: &str,
    query: &str,
    max_chunks: i32,
    embedding_provider: &str,
    embedding_model: &str,
) -> Vec<RagChunk> {
    let scored_points = if embedding_provider == "qdrant" {
        // Qdrant server-side inference
        use qdrant_client::qdrant::{Document, Query, QueryPointsBuilder};

        match qdrant
            .query(
                QueryPointsBuilder::new(collection_name)
                    .query(Query::new_nearest(Document::new(query, embedding_model)))
                    .with_payload(true)
                    .limit(max_chunks as u64),
            )
            .await
        {
            Ok(result) => result.result,
            Err(e) => {
                tracing::warn!("qdrant query failed: {}, skipping RAG", e);
                return Vec::new();
            }
        }
    } else {
        // OpenAI client-side embedding
        let embedding = match minerva_ingest::embedder::embed_texts(
            client,
            openai_key,
            std::slice::from_ref(&query.to_string()),
        )
        .await
        {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!("embedding failed: {}, skipping RAG", e);
                return Vec::new();
            }
        };

        let vector = match embedding.embeddings.into_iter().next() {
            Some(v) => v,
            None => return Vec::new(),
        };

        match qdrant
            .search_points(
                SearchPointsBuilder::new(collection_name, vector, max_chunks as u64)
                    .with_payload(true),
            )
            .await
        {
            Ok(result) => result.result,
            Err(e) => {
                tracing::warn!("qdrant search failed: {}, skipping RAG", e);
                return Vec::new();
            }
        }
    };

    scored_points
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
            let document_id = match payload.get("document_id").and_then(|v| v.kind.as_ref()) {
                Some(qdrant_client::qdrant::value::Kind::StringValue(s)) => s.clone(),
                _ => String::new(),
            };
            Some(RagChunk {
                document_id,
                filename,
                text,
            })
        })
        .collect()
}

/// Stream a Cerebras completion to the client via tx, appending tokens to full_text.
/// Returns (prompt_tokens, completion_tokens).
pub async fn stream_cerebras_to_client(
    client: &reqwest::Client,
    api_key: &str,
    model: &str,
    temperature: f64,
    messages: &[serde_json::Value],
    tx: &mpsc::Sender<Result<Event, AppError>>,
    full_text: &mut String,
) -> Result<(i32, i32), String> {
    let body = serde_json::json!({
        "model": model,
        "messages": messages,
        "temperature": temperature,
        "stream": true,
        "stream_options": { "include_usage": true },
    });

    let response = cerebras_request_with_retry(client, api_key, &body).await?;

    let mut stream = response.bytes_stream();
    let mut buffer = String::new();
    let mut prompt_tokens = 0i32;
    let mut completion_tokens = 0i32;

    'outer: while let Some(chunk) = stream.next().await {
        let chunk = match chunk {
            Ok(c) => c,
            Err(e) => {
                tracing::error!("cerebras stream error: {}", e);
                return Err(format!("Stream interrupted: {}", e));
            }
        };
        buffer.push_str(&String::from_utf8_lossy(&chunk));

        while let Some(line_end) = buffer.find('\n') {
            let line = buffer[..line_end].trim().to_string();
            buffer = buffer[line_end + 1..].to_string();

            if line == "data: [DONE]" {
                break 'outer;
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
                        if tx
                            .send(Ok(Event::default().data(
                                serde_json::json!({"type": "token", "token": delta}).to_string(),
                            )))
                            .await
                            .is_err()
                        {
                            return Err("client disconnected".to_string());
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

    Ok((prompt_tokens, completion_tokens))
}

/// Finalize: save message, set title, record usage, send done event.
pub async fn finalize(
    ctx: &super::GenerationContext,
    tx: &mpsc::Sender<Result<Event, AppError>>,
    full_text: &str,
    chunks_json: Option<&serde_json::Value>,
    prompt_tokens: i32,
    completion_tokens: i32,
    rag_injected: bool,
) {
    let assistant_msg_id = uuid::Uuid::new_v4();
    let _ = minerva_db::queries::conversations::insert_message(
        &ctx.db,
        assistant_msg_id,
        ctx.conversation_id,
        "assistant",
        full_text,
        chunks_json,
        Some(&ctx.model),
        Some(prompt_tokens),
        Some(completion_tokens),
    )
    .await;

    if ctx.is_first_message {
        let title: String = ctx.user_content.chars().take(60).collect();
        let title = if ctx.user_content.chars().count() > 60 {
            format!("{}...", title.trim())
        } else {
            title
        };
        let _ =
            minerva_db::queries::conversations::update_title(&ctx.db, ctx.conversation_id, &title)
                .await;
    }

    let _ = minerva_db::queries::usage::record_usage(
        &ctx.db,
        ctx.user_id,
        ctx.course_id,
        prompt_tokens as i64,
        completion_tokens as i64,
        0,
    )
    .await;

    let _ = tx
        .send(Ok(Event::default().data(
            serde_json::json!({
                "type": "done",
                "tokens_prompt": prompt_tokens,
                "tokens_completion": completion_tokens,
                "rag_injected": rag_injected,
                "chunks_used": chunks_json,
            })
            .to_string(),
        )))
        .await;
}
