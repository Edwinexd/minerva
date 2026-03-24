use axum::extract::{Extension, Path, State};
use axum::response::sse::{Event, Sse};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures::{Stream, StreamExt};
use minerva_core::models::User;
use qdrant_client::qdrant::SearchPointsBuilder;
use serde::{Deserialize, Serialize};
use std::pin::Pin;
use tokio::sync::mpsc;
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

    // Get conversation history
    let history = minerva_db::queries::conversations::list_messages(&state.db, cid).await?;

    // Embed the query
    let http_client = reqwest::Client::new();
    let query_embedding = minerva_ingest::embedder::embed_texts(
        &http_client, &state.config.openai_api_key, std::slice::from_ref(&body.content),
    )
    .await
    .map_err(|e| AppError::Internal(format!("embedding failed: {}", e)))?;

    let query_vector = query_embedding.embeddings.into_iter().next()
        .ok_or_else(|| AppError::Internal("no embedding returned".to_string()))?;

    // Search Qdrant
    let collection_name = format!("course_{}", course_id);
    let chunks: Vec<String> = match state.qdrant
        .search_points(
            SearchPointsBuilder::new(&collection_name, query_vector, course.max_chunks as u64)
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
            tracing::warn!("qdrant search failed: {}, proceeding without context", e);
            Vec::new()
        }
    };

    let chunks_json = serde_json::to_value(&chunks).ok();

    // Build prompt
    let system_prompt = build_system_prompt(&course, &chunks);
    let chat_messages = build_chat_messages(&system_prompt, &history, &course);

    // Set up channel for streaming
    let (tx, rx) = mpsc::channel::<Result<Event, AppError>>(32);

    let db = state.db.clone();
    let api_key = state.config.cerebras_api_key.clone();
    let model = course.model.clone();
    let temperature = course.temperature;
    let user_id = conv.user_id;
    let is_first_message = history.len() <= 1; // Only the user message we just inserted
    let user_content = body.content.clone();

    tokio::spawn(async move {
        let mut full_text = String::new();
        let mut prompt_tokens = 0i32;
        let mut completion_tokens = 0i32;

        let body = serde_json::json!({
            "model": model,
            "messages": chat_messages,
            "temperature": temperature,
            "stream": true,
        });

        let response = http_client
            .post("https://api.cerebras.ai/v1/chat/completions")
            .header("Authorization", format!("Bearer {}", api_key))
            .json(&body)
            .send()
            .await;

        let response = match response {
            Ok(r) if r.status().is_success() => r,
            Ok(r) => {
                let status = r.status();
                let body = r.text().await.unwrap_or_default();
                let _ = tx.send(Ok(Event::default().data(
                    serde_json::json!({"type": "error", "error": format!("Cerebras API error {}: {}", status, body)}).to_string()
                ))).await;
                return;
            }
            Err(e) => {
                let _ = tx.send(Ok(Event::default().data(
                    serde_json::json!({"type": "error", "error": format!("Request failed: {}", e)}).to_string()
                ))).await;
                return;
            }
        };

        // Stream SSE from Cerebras
        let mut stream = response.bytes_stream();
        let mut buffer = String::new();

        while let Some(chunk) = stream.next().await {
            let chunk = match chunk {
                Ok(c) => c,
                Err(_) => break,
            };
            buffer.push_str(&String::from_utf8_lossy(&chunk));

            // Process complete SSE lines
            while let Some(line_end) = buffer.find('\n') {
                let line = buffer[..line_end].trim().to_string();
                buffer = buffer[line_end + 1..].to_string();

                if line == "data: [DONE]" {
                    break;
                }

                if let Some(data) = line.strip_prefix("data: ") {
                    if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(data) {
                        // Extract token
                        if let Some(delta) = parsed["choices"][0]["delta"]["content"].as_str() {
                            full_text.push_str(delta);
                            let _ = tx.send(Ok(Event::default().data(
                                serde_json::json!({"type": "token", "token": delta}).to_string()
                            ))).await;
                        }

                        // Extract usage from final chunk
                        if let Some(usage) = parsed.get("usage") {
                            prompt_tokens = usage["prompt_tokens"].as_i64().unwrap_or(0) as i32;
                            completion_tokens = usage["completion_tokens"].as_i64().unwrap_or(0) as i32;
                        }
                    }
                }
            }
        }

        // Save assistant message
        let assistant_msg_id = Uuid::new_v4();
        let _ = minerva_db::queries::conversations::insert_message(
            &db, assistant_msg_id, cid, "assistant", &full_text,
            chunks_json.as_ref(), Some(&model),
            Some(prompt_tokens), Some(completion_tokens),
        ).await;

        // Auto-generate conversation title from first user message
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
            prompt_tokens as i64, completion_tokens as i64, 0,
        ).await;

        // Send done event
        let _ = tx.send(Ok(Event::default().data(
            serde_json::json!({
                "type": "done",
                "tokens_prompt": prompt_tokens,
                "tokens_completion": completion_tokens,
            }).to_string()
        ))).await;
    });

    let stream = ReceiverStream::new(rx);
    Ok(Sse::new(Box::pin(stream)))
}

fn build_system_prompt(
    course: &minerva_db::queries::courses::CourseRow,
    chunks: &[String],
) -> String {
    let mut prompt = format!(
        "You are a helpful teaching assistant for the course '{}'. Answer questions based on the provided course materials. If you cannot answer from the materials, say so clearly.",
        course.name
    );

    if let Some(ref custom) = course.system_prompt {
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
    _course: &minerva_db::queries::courses::CourseRow,
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
