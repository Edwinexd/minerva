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
use uuid::Uuid;

use crate::auth::user_from_row;
use crate::error::AppError;
use crate::routes::chat::{
    fetch_conversation_for_view, list_pinned_conversations_for, ConversationWithUserResponse,
};
use crate::routes::integration::verify_embed_token;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/course/{course_id}", get(get_course))
        .route(
            "/course/{course_id}/conversations",
            get(list_conversations).post(start_conversation),
        )
        .route(
            "/course/{course_id}/conversations/pinned",
            get(list_pinned_conversations),
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
        .route(
            "/course/{course_id}/acknowledge-privacy",
            post(acknowledge_privacy),
        )
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
    role: String,
    privacy_acknowledged_at: Option<chrono::DateTime<chrono::Utc>>,
    lti_client_id: Option<String>,
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

    // Check if this course has an LTI registration.
    let lti_regs =
        minerva_db::queries::lti::list_registrations_for_course(&state.db, course_id).await?;
    let lti_client_id = lti_regs.first().map(|r| r.client_id.clone());

    Ok(Json(MeResponse {
        id: user.id,
        eppn: user.eppn,
        display_name: user.display_name,
        role: user.role,
        privacy_acknowledged_at: user.privacy_acknowledged_at,
        lti_client_id,
    }))
}

async fn acknowledge_privacy(
    State(state): State<AppState>,
    Path(course_id): Path<Uuid>,
    Query(query): Query<TokenQuery>,
) -> Result<Json<serde_json::Value>, AppError> {
    let (_, user_id) = authenticate(&state, course_id, &query)?;
    minerva_db::queries::users::acknowledge_privacy(&state.db, user_id).await?;
    Ok(Json(serde_json::json!({ "ok": true })))
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

/// List pinned conversations for the embed view.
///
/// Embed token attests course membership (verified by `authenticate`),
/// so the only extra step before delegating to the shared helper is
/// promoting the DB row to a `User` -- the embed route doesn't pass
/// through `auth_middleware` and therefore has no `Extension<User>`.
async fn list_pinned_conversations(
    State(state): State<AppState>,
    Path(course_id): Path<Uuid>,
    Query(query): Query<TokenQuery>,
) -> Result<Json<Vec<ConversationWithUserResponse>>, AppError> {
    let (_, user_id) = authenticate(&state, course_id, &query)?;
    let viewer = user_from_row(
        minerva_db::queries::users::find_by_id(&state.db, user_id)
            .await?
            .ok_or(AppError::Unauthorized)?,
    );
    Ok(Json(
        list_pinned_conversations_for(&state, course_id, &viewer).await?,
    ))
}

async fn get_conversation(
    State(state): State<AppState>,
    Path((course_id, cid)): Path<(Uuid, Uuid)>,
    Query(query): Query<TokenQuery>,
) -> Result<Json<ConversationDetailResponse>, AppError> {
    let (_, user_id) = authenticate(&state, course_id, &query)?;
    // Reuse the shared access guard so teacher-pinned conversations are
    // readable from the embed sidebar (previously 403'd because the
    // owner-only check rejected non-owner viewers).
    let viewer = user_from_row(
        minerva_db::queries::users::find_by_id(&state.db, user_id)
            .await?
            .ok_or(AppError::Unauthorized)?,
    );
    let (_conv, _is_teacher) = fetch_conversation_for_view(&state, course_id, cid, &viewer).await?;

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
    let (course, user_id, privacy_acked_at) =
        authenticate_for_message(&state, course_id, &body.token).await?;
    crate::routes::chat::run_chat_message(
        &state,
        course,
        user_id,
        privacy_acked_at,
        Some(cid),
        body.content,
    )
    .await
}

async fn start_conversation(
    State(state): State<AppState>,
    Path(course_id): Path<Uuid>,
    Json(body): Json<SendMessageRequest>,
) -> Result<Sse<Pin<Box<dyn Stream<Item = Result<Event, AppError>> + Send>>>, AppError> {
    let (course, user_id, privacy_acked_at) =
        authenticate_for_message(&state, course_id, &body.token).await?;
    crate::routes::chat::run_chat_message(
        &state,
        course,
        user_id,
        privacy_acked_at,
        None,
        body.content,
    )
    .await
}

/// Token-auth path used by both message-streaming endpoints. Token lives
/// in the JSON body because EventSource POSTs can't carry query params.
async fn authenticate_for_message(
    state: &AppState,
    path_course_id: Uuid,
    token: &str,
) -> Result<
    (
        minerva_db::queries::courses::CourseRow,
        Uuid,
        Option<chrono::DateTime<chrono::Utc>>,
    ),
    AppError,
> {
    let (token_course_id, user_id) =
        verify_embed_token(&state.config.hmac_secret, token).map_err(|_| AppError::Unauthorized)?;
    if token_course_id != path_course_id {
        return Err(AppError::Forbidden);
    }

    let course = minerva_db::queries::courses::find_by_id(&state.db, path_course_id)
        .await?
        .ok_or(AppError::NotFound)?;
    let user = minerva_db::queries::users::find_by_id(&state.db, user_id)
        .await?
        .ok_or(AppError::Unauthorized)?;

    Ok((course, user_id, user.privacy_acknowledged_at))
}
