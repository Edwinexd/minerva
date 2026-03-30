//! Embeddable chat API for iframe usage (e.g. Moodle plugin).
//!
//! Completely separate from the Shibboleth-authed routes.
//! Authenticated via HMAC-signed embed tokens created by the
//! integration API. Each token is scoped to a (course_id, user_id)
//! pair and has a limited lifetime.
//!
//! Mounted at `/api/embed/` -- outside the auth_middleware layer.

use axum::extract::{Path, Query, State};
use axum::response::sse::{Event, Sse};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures::Stream;
use serde::{Deserialize, Serialize};
use std::pin::Pin;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use uuid::Uuid;

use crate::error::AppError;
use crate::routes::integration::verify_embed_token;
use crate::state::AppState;
use crate::strategy;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/course/{course_id}", get(get_course))
        .route(
            "/course/{course_id}/conversations",
            get(list_conversations).post(create_conversation),
        )
        .route(
            "/course/{course_id}/conversations/{cid}",
            get(get_conversation),
        )
        .route(
            "/course/{course_id}/conversations/{cid}/message",
            post(send_message),
        )
        .route("/course/{course_id}/me", get(get_me))
}

/// All embed endpoints require `?token=...` query param.
#[derive(Deserialize)]
struct TokenQuery {
    token: String,
}

/// Validate the embed token and return (course_id from token, user_id).
/// Also verifies the path course_id matches the token's course_id.
fn authenticate(
    state: &AppState,
    path_course_id: Uuid,
    query: &TokenQuery,
) -> Result<(Uuid, Uuid), AppError> {
    let (course_id, user_id) = verify_embed_token(&state.config.hmac_secret, &query.token)?;
    if course_id != path_course_id {
        return Err(AppError::Forbidden);
    }
    Ok((course_id, user_id))
}

// -- Response types --

#[derive(Serialize)]
struct CourseResponse {
    id: Uuid,
    name: String,
    description: Option<String>,
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
    created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Serialize)]
struct ConversationDetailResponse {
    messages: Vec<MessageResponse>,
}

#[derive(Serialize)]
struct MeResponse {
    id: Uuid,
    eppn: String,
    display_name: Option<String>,
}

// -- Handlers --

async fn get_me(
    State(state): State<AppState>,
    Path(course_id): Path<Uuid>,
    Query(query): Query<TokenQuery>,
) -> Result<Json<MeResponse>, AppError> {
    let (_, user_id) = authenticate(&state, course_id, &query)?;

    let user = minerva_db::queries::users::find_by_id(&state.db, user_id)
        .await?
        .ok_or(AppError::NotFound)?;

    Ok(Json(MeResponse {
        id: user.id,
        eppn: user.eppn,
        display_name: user.display_name,
    }))
}

async fn get_course(
    State(state): State<AppState>,
    Path(course_id): Path<Uuid>,
    Query(query): Query<TokenQuery>,
) -> Result<Json<CourseResponse>, AppError> {
    authenticate(&state, course_id, &query)?;

    let course = minerva_db::queries::courses::find_by_id(&state.db, course_id)
        .await?
        .ok_or(AppError::NotFound)?;

    Ok(Json(CourseResponse {
        id: course.id,
        name: course.name,
        description: course.description,
    }))
}

async fn list_conversations(
    State(state): State<AppState>,
    Path(course_id): Path<Uuid>,
    Query(query): Query<TokenQuery>,
) -> Result<Json<Vec<ConversationResponse>>, AppError> {
    let (_, user_id) = authenticate(&state, course_id, &query)?;

    let rows =
        minerva_db::queries::conversations::list_by_course_user(&state.db, course_id, user_id)
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
    Path(course_id): Path<Uuid>,
    Query(query): Query<TokenQuery>,
) -> Result<Json<ConversationResponse>, AppError> {
    let (_, user_id) = authenticate(&state, course_id, &query)?;

    let id = Uuid::new_v4();
    let row = minerva_db::queries::conversations::create(&state.db, id, course_id, user_id).await?;

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
    Path((course_id, cid)): Path<(Uuid, Uuid)>,
    Query(query): Query<TokenQuery>,
) -> Result<Json<ConversationDetailResponse>, AppError> {
    let (_, user_id) = authenticate(&state, course_id, &query)?;

    let conv = minerva_db::queries::conversations::find_by_id(&state.db, cid)
        .await?
        .ok_or(AppError::NotFound)?;

    if conv.user_id != user_id || conv.course_id != course_id {
        return Err(AppError::Forbidden);
    }

    let messages = minerva_db::queries::conversations::list_messages(&state.db, cid).await?;

    Ok(Json(ConversationDetailResponse {
        messages: messages
            .into_iter()
            .map(|m| MessageResponse {
                id: m.id,
                role: m.role,
                content: m.content,
                chunks_used: m.chunks_used,
                model_used: m.model_used,
                created_at: m.created_at,
            })
            .collect(),
    }))
}

#[derive(Deserialize)]
struct SendMessageRequest {
    content: String,
    token: String,
}

async fn send_message(
    State(state): State<AppState>,
    Path((course_id, cid)): Path<(Uuid, Uuid)>,
    Json(body): Json<SendMessageRequest>,
) -> Result<Sse<Pin<Box<dyn Stream<Item = Result<Event, AppError>> + Send>>>, AppError> {
    // Token is in the JSON body for SSE (can't use query params with EventSource POST).
    let (_, user_id) = verify_embed_token(&state.config.hmac_secret, &body.token)
        .map_err(|_| AppError::Unauthorized)?;

    if verify_embed_token(&state.config.hmac_secret, &body.token)
        .map(|(cid, _)| cid != course_id)
        .unwrap_or(true)
    {
        return Err(AppError::Forbidden);
    }

    let course = minerva_db::queries::courses::find_by_id(&state.db, course_id)
        .await?
        .ok_or(AppError::NotFound)?;

    let conv = minerva_db::queries::conversations::find_by_id(&state.db, cid)
        .await?
        .ok_or(AppError::NotFound)?;

    if conv.user_id != user_id || conv.course_id != course_id {
        return Err(AppError::Forbidden);
    }

    // Enforce daily token limit.
    if course.daily_token_limit > 0 {
        let used = minerva_db::queries::usage::get_user_daily_tokens(&state.db, user_id, course_id)
            .await?;
        if used >= course.daily_token_limit {
            return Err(AppError::QuotaExceeded);
        }
    }

    // Save user message.
    let user_msg_id = Uuid::new_v4();
    minerva_db::queries::conversations::insert_message(
        &state.db,
        user_msg_id,
        cid,
        "user",
        &body.content,
        None,
        None,
        None,
        None,
    )
    .await?;

    let history = minerva_db::queries::conversations::list_messages(&state.db, cid).await?;
    let is_first_message = history.len() <= 1;

    let (tx, rx) = mpsc::channel::<Result<Event, AppError>>(32);

    let strategy_name = course.strategy.clone();

    let ctx = strategy::GenerationContext {
        course_name: course.name,
        custom_prompt: course.system_prompt,
        model: course.model,
        temperature: course.temperature,
        max_chunks: course.max_chunks,
        course_id,
        conversation_id: cid,
        user_id: conv.user_id,
        cerebras_api_key: state.config.cerebras_api_key.clone(),
        openai_api_key: state.config.openai_api_key.clone(),
        embedding_provider: course.embedding_provider,
        embedding_model: course.embedding_model,
        history,
        user_content: body.content,
        is_first_message,
        db: state.db.clone(),
        qdrant: Arc::clone(&state.qdrant),
    };

    tokio::spawn(async move {
        strategy::run_strategy(&strategy_name, ctx, tx).await;
    });

    let stream = ReceiverStream::new(rx);
    Ok(Sse::new(Box::pin(stream)))
}
