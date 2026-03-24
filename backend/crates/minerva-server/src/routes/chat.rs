use axum::extract::{Extension, Path, State};
use axum::response::sse::{Event, Sse};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures::{Stream, StreamExt};
use minerva_core::models::User;
use qdrant_client::qdrant::SearchPointsBuilder;
use serde::{Deserialize, Serialize};
use std::pin::Pin;
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot};
use tokio_stream::wrappers::ReceiverStream;
use uuid::Uuid;

use crate::error::AppError;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/conversations", get(list_conversations).post(create_conversation))
        .route("/conversations/{cid}", get(get_conversation))
        .route("/conversations/{cid}/message", post(send_message))
}

#[derive(Serialize)]
struct ConversationResponse {
    id: Uuid,
    course_id: Uuid,
    title: Option<String>,
    created_at: chrono::DateTime<chrono::Utc>,
    updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Serialize)]
struct MessageResponse {
    id: Uuid,
    role: String,
    content: String,
    chunks_used: Option<serde_json::Value>,
    model_used: Option<String>,
    tokens_prompt: Option<i32>,
    tokens_completion: Option<i32>,
    created_at: chrono::DateTime<chrono::Utc>,
}

async fn list_conversations(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(course_id): Path<Uuid>,
) -> Result<Json<Vec<ConversationResponse>>, AppError> {
    verify_course_access(&state, course_id, user.id).await?;

    let rows =
        minerva_db::queries::conversations::list_by_course_user(&state.db, course_id, user.id)
            .await?;

    Ok(Json(
        rows.into_iter()
            .map(|r| ConversationResponse {
                id: r.id,
                course_id: r.course_id,
                title: r.title,
                created_at: r.created_at,
                updated_at: r.updated_at,
            })
            .collect(),
    ))
}

async fn create_conversation(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(course_id): Path<Uuid>,
) -> Result<Json<ConversationResponse>, AppError> {
    verify_course_access(&state, course_id, user.id).await?;

    let id = Uuid::new_v4();
    let row =
        minerva_db::queries::conversations::create(&state.db, id, course_id, user.id).await?;

    Ok(Json(ConversationResponse {
        id: row.id,
        course_id: row.course_id,
        title: row.title,
        created_at: row.created_at,
        updated_at: row.updated_at,
    }))
}

async fn get_conversation(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path((course_id, cid)): Path<(Uuid, Uuid)>,
) -> Result<Json<Vec<MessageResponse>>, AppError> {
    verify_course_access(&state, course_id, user.id).await?;

    let conv = minerva_db::queries::conversations::find_by_id(&state.db, cid)
        .await?
        .ok_or(AppError::NotFound)?;

    if conv.user_id != user.id && !user.role.is_admin() {
        return Err(AppError::Forbidden);
    }

    let messages = minerva_db::queries::conversations::list_messages(&state.db, cid).await?;

    Ok(Json(
        messages
            .into_iter()
            .map(|m| MessageResponse {
                id: m.id,
                role: m.role,
                content: m.content,
                chunks_used: m.chunks_used,
                model_used: m.model_used,
                tokens_prompt: m.tokens_prompt,
                tokens_completion: m.tokens_completion,
                created_at: m.created_at,
            })
            .collect(),
    ))
}

#[derive(Deserialize)]
struct SendMessageRequest {
    content: String,
}

/// FLARE-style streaming:
/// 1. Start Cerebras immediately WITHOUT RAG context (instant first token)
/// 2. In parallel, embed query + search Qdrant for RAG chunks
/// 3. When RAG results arrive: abort current stream, inject context + partial output,
///    restart Cerebras to continue seamlessly
/// 4. Client sees uninterrupted token stream
async fn send_message(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path((course_id, cid)): Path<(Uuid, Uuid)>,
    Json(body): Json<SendMessageRequest>,
) -> Result<Sse<Pin<Box<dyn Stream<Item = Result<Event, AppError>> + Send>>>, AppError> {
    let course = verify_course_access(&state, course_id, user.id).await?;

    let conv = minerva_db::queries::conversations::find_by_id(&state.db, cid)
        .await?
        .ok_or(AppError::NotFound)?;

    if conv.user_id != user.id {
        return Err(AppError::Forbidden);
    }

    // Save user message
    let user_msg_id = Uuid::new_v4();
    minerva_db::queries::conversations::insert_message(
        &state.db, user_msg_id, cid, "user", &body.content,
        None, None, None, None,
    )
    .await?;

    let history = minerva_db::queries::conversations::list_messages(&state.db, cid).await?;

    let (tx, rx) = mpsc::channel::<Result<Event, AppError>>(32);

    let db = state.db.clone();
    let qdrant = Arc::clone(&state.qdrant);
    let api_key = state.config.cerebras_api_key.clone();
    let openai_key = state.config.openai_api_key.clone();
    let model = course.model.clone();
    let temperature = course.temperature;
    let max_chunks = course.max_chunks;
    let user_id = conv.user_id;
    let is_first_message = history.len() <= 1;
    let user_content = body.content.clone();
    let course_name = course.name.clone();
    let custom_prompt = course.system_prompt.clone();

    tokio::spawn(async move {
        let http_client = reqwest::Client::new();

        // Build initial prompt WITHOUT RAG context
        let system_no_rag = build_system_prompt_text(&course_name, &custom_prompt, &[]);
        let initial_messages = build_chat_messages(&system_no_rag, &history);

        // Spawn RAG lookup in parallel
        let (rag_tx, mut rag_rx) = oneshot::channel::<Vec<String>>();
        let rag_client = http_client.clone();
        let rag_openai_key = openai_key.clone();
        let rag_qdrant = Arc::clone(&qdrant);
        let rag_content = user_content.clone();
        let collection_name = format!("course_{}", course_id);

        tokio::spawn(async move {
            let chunks = rag_lookup(
                &rag_client, &rag_openai_key, &rag_qdrant,
                &collection_name, &rag_content, max_chunks,
            ).await;
            let _ = rag_tx.send(chunks);
        });

        // Phase 1: Start streaming from Cerebras immediately (no RAG)
        let mut full_text = String::new();
        let mut total_prompt_tokens = 0i32;
        let mut total_completion_tokens = 0i32;
        let mut chunks_json: Option<serde_json::Value> = None;
        let mut rag_injected = false;

        let phase1_result = stream_cerebras(
            &http_client, &api_key, &model, temperature,
            &initial_messages, &tx, &mut full_text, &mut rag_rx,
        ).await;

        match phase1_result {
            StreamResult::Completed { prompt_tokens, completion_tokens } => {
                // RAG didn't arrive in time, response is done without context
                total_prompt_tokens += prompt_tokens;
                total_completion_tokens += completion_tokens;
            }
            StreamResult::Interrupted { prompt_tokens, completion_tokens, rag_chunks } => {
                // RAG arrived! Continue with context
                total_prompt_tokens += prompt_tokens;
                total_completion_tokens += completion_tokens;
                rag_injected = true;
                chunks_json = serde_json::to_value(&rag_chunks).ok();

                tracing::info!(
                    "FLARE: RAG arrived after {} chars, restarting with {} chunks",
                    full_text.len(), rag_chunks.len()
                );

                // Build new prompt with RAG context + partial output as assistant prefix
                let system_with_rag = build_system_prompt_text(&course_name, &custom_prompt, &rag_chunks);
                let mut continued_messages = build_chat_messages(&system_with_rag, &history);

                // Add partial assistant response so Cerebras continues from where we left off
                if !full_text.is_empty() {
                    continued_messages.push(serde_json::json!({
                        "role": "assistant",
                        "prefix": true,
                        "content": full_text,
                    }));
                }

                // Phase 2: Continue streaming with RAG context
                let mut dummy_rag_rx = oneshot::channel::<Vec<String>>().1;
                let phase2_result = stream_cerebras(
                    &http_client, &api_key, &model, temperature,
                    &continued_messages, &tx, &mut full_text, &mut dummy_rag_rx,
                ).await;

                match phase2_result {
                    StreamResult::Completed { prompt_tokens, completion_tokens } => {
                        total_prompt_tokens += prompt_tokens;
                        total_completion_tokens += completion_tokens;
                    }
                    StreamResult::Interrupted { prompt_tokens, completion_tokens, .. } => {
                        // Shouldn't happen in phase 2, but handle gracefully
                        total_prompt_tokens += prompt_tokens;
                        total_completion_tokens += completion_tokens;
                    }
                    StreamResult::Error(_) => {}
                }
            }
            StreamResult::Error(e) => {
                tracing::error!("cerebras stream failed: {}", e);
            }
        }

        // If RAG wasn't injected during streaming, check if results are available now
        if !rag_injected {
            // rag_rx was moved into stream_cerebras, but if it completed without interruption
            // the chunks are lost. That's fine - the answer was generated without context.
            tracing::debug!("response completed without RAG context injection");
        }

        // Save assistant message
        let assistant_msg_id = Uuid::new_v4();
        let _ = minerva_db::queries::conversations::insert_message(
            &db, assistant_msg_id, cid, "assistant", &full_text,
            chunks_json.as_ref(), Some(&model),
            Some(total_prompt_tokens), Some(total_completion_tokens),
        ).await;

        // Auto-generate conversation title
        if is_first_message {
            let title: String = user_content.chars().take(60).collect();
            let title = if user_content.chars().count() > 60 {
                format!("{}...", title.trim())
            } else {
                title
            };
            let _ = minerva_db::queries::conversations::update_title(&db, cid, &title).await;
        }

        // Record usage
        let _ = minerva_db::queries::usage::record_usage(
            &db, user_id, course_id,
            total_prompt_tokens as i64, total_completion_tokens as i64, 0,
        ).await;

        // Send done event
        let _ = tx.send(Ok(Event::default().data(
            serde_json::json!({
                "type": "done",
                "tokens_prompt": total_prompt_tokens,
                "tokens_completion": total_completion_tokens,
                "rag_injected": rag_injected,
            }).to_string()
        ))).await;
    });

    let stream = ReceiverStream::new(rx);
    Ok(Sse::new(Box::pin(stream)))
}

enum StreamResult {
    /// Stream completed normally
    Completed { prompt_tokens: i32, completion_tokens: i32 },
    /// Stream was interrupted because RAG results arrived
    Interrupted { prompt_tokens: i32, completion_tokens: i32, rag_chunks: Vec<String> },
    /// Stream failed with error
    Error(String),
}

/// Stream from Cerebras, checking for RAG results between chunks.
/// If rag_rx resolves, interrupts the stream and returns the chunks.
#[allow(clippy::too_many_arguments)]
async fn stream_cerebras(
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
            let msg = format!("Cerebras API error {}: {}", status, body);
            let _ = tx.send(Ok(Event::default().data(
                serde_json::json!({"type": "error", "error": msg}).to_string()
            ))).await;
            return StreamResult::Error(msg);
        }
        Err(e) => {
            let msg = format!("Request failed: {}", e);
            let _ = tx.send(Ok(Event::default().data(
                serde_json::json!({"type": "error", "error": msg}).to_string()
            ))).await;
            return StreamResult::Error(msg);
        }
    };

    let mut stream = response.bytes_stream();
    let mut buffer = String::new();
    let mut prompt_tokens = 0i32;
    let mut completion_tokens = 0i32;

    loop {
        tokio::select! {
            // Check if RAG results arrived
            rag_result = &mut *rag_rx => {
                if let Ok(chunks) = rag_result {
                    if !chunks.is_empty() {
                        return StreamResult::Interrupted {
                            prompt_tokens, completion_tokens, rag_chunks: chunks
                        };
                    }
                    // Empty RAG results - continue without interruption
                }
            }
            // Read next chunk from Cerebras stream
            chunk = stream.next() => {
                let chunk = match chunk {
                    Some(Ok(c)) => c,
                    Some(Err(e)) => {
                        tracing::error!("cerebras stream error: {}", e);
                        let _ = tx.send(Ok(Event::default().data(
                            serde_json::json!({"type": "error", "error": format!("Stream interrupted: {}", e)}).to_string()
                        ))).await;
                        return StreamResult::Error(e.to_string());
                    }
                    None => {
                        // Stream ended
                        return StreamResult::Completed { prompt_tokens, completion_tokens };
                    }
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
                                let msg = err["message"].as_str().unwrap_or("unknown error").to_string();
                                tracing::error!("cerebras error: {}", msg);
                                let _ = tx.send(Ok(Event::default().data(
                                    serde_json::json!({"type": "error", "error": msg}).to_string()
                                ))).await;
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

                            if let Some(reason) = parsed["choices"][0]["finish_reason"].as_str() {
                                if reason == "length" {
                                    tracing::warn!("cerebras hit max tokens");
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

/// Perform RAG lookup: embed query, search Qdrant, return chunk texts.
async fn rag_lookup(
    client: &reqwest::Client,
    openai_key: &str,
    qdrant: &qdrant_client::Qdrant,
    collection_name: &str,
    query: &str,
    max_chunks: i32,
) -> Vec<String> {
    // Embed
    let embedding = match minerva_ingest::embedder::embed_texts(
        client, openai_key, std::slice::from_ref(&query.to_string()),
    ).await {
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

    // Search
    match qdrant
        .search_points(
            SearchPointsBuilder::new(collection_name, vector, max_chunks as u64)
                .with_payload(true),
        )
        .await
    {
        Ok(result) => result.result.iter().filter_map(|point| {
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
        }).collect(),
        Err(e) => {
            tracing::warn!("qdrant search failed: {}, skipping RAG", e);
            Vec::new()
        }
    }
}

fn build_system_prompt_text(course_name: &str, custom_prompt: &Option<String>, chunks: &[String]) -> String {
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

fn build_chat_messages(
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

async fn verify_course_access(
    state: &AppState,
    course_id: Uuid,
    user_id: Uuid,
) -> Result<minerva_db::queries::courses::CourseRow, AppError> {
    let course = minerva_db::queries::courses::find_by_id(&state.db, course_id)
        .await?
        .ok_or(AppError::NotFound)?;

    if course.owner_id != user_id
        && !minerva_db::queries::courses::is_member(&state.db, course_id, user_id).await?
    {
        return Err(AppError::Forbidden);
    }

    Ok(course)
}
