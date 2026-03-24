use axum::response::sse::Event;
use futures::StreamExt;
use qdrant_client::qdrant::SearchPointsBuilder;
use tokio::sync::mpsc;

use crate::error::AppError;

/// Build the system prompt with optional RAG chunks.
pub fn build_system_prompt(
    course_name: &str,
    custom_prompt: &Option<String>,
    chunks: &[String],
) -> String {
    let mut prompt = format!(
        "You are a helpful teaching assistant for the course '{}'. Answer questions based on the provided course materials. If you cannot answer from the materials, say so clearly.",
        course_name
    );

    if let Some(ref custom) = custom_prompt {
        prompt.push_str("\n\nAdditional instructions: ");
        prompt.push_str(custom);
    }

    if !chunks.is_empty() {
        prompt.push_str("\n\nRelevant course materials:\n---\n");
        prompt.push_str(&chunks.join("\n---\n"));
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

/// Perform RAG lookup: embed query, search Qdrant, return chunk texts.
pub async fn rag_lookup(
    client: &reqwest::Client,
    openai_key: &str,
    qdrant: &qdrant_client::Qdrant,
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
            SearchPointsBuilder::new(collection_name, vector, max_chunks as u64).with_payload(true),
        )
        .await
    {
        Ok(result) => result
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
                Some(format!("[Source: {}]\n{}", filename, text))
            })
            .collect(),
        Err(e) => {
            tracing::warn!("qdrant search failed: {}, skipping RAG", e);
            Vec::new()
        }
    }
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
