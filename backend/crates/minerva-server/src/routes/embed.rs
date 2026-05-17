//! Embeddable chat API for iframe usage (e.g. Moodle plugin).
//!
//! Completely separate from the Shibboleth-authed routes.
//! Authenticated via HMAC-signed embed tokens created by the
//! integration API. Each token is scoped to a (course_id, user_id)
//! pair and has a limited lifetime.
//!
//! Mounted at `/api/embed/`; outside the auth_middleware layer.

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
use crate::ext_obfuscate::{self, Pseudonymizer};
use crate::routes::chat::{
    analyze_prompt_for_user, fetch_conversation_for_view, list_pinned_conversations_for,
    load_prompt_analyses_for_conversation, rewrite_prompt_for_user, AegisAnalysisPayload,
    AegisModeWire, AegisRewriteResponse, AegisSuggestionPayload, ConversationWithUserResponse,
    PromptAnalysisResponse,
};
use crate::routes::courses::{resolve_course_flags, CourseFeatureFlagsView};
use crate::routes::integration::verify_embed_token;
use crate::routes::suggested_questions::{self, SuggestedQuestionsResponse};
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
        // Mirrors the Shibboleth chat router's mark-read. The embed
        // surface is always a student opening their own conversation
        // (embed tokens are owner-scoped), so this purely bumps
        // `student_last_viewed_at`; no teacher branch like the
        // Shibboleth handler has.
        .route(
            "/course/{course_id}/conversations/{cid}/mark-read",
            post(mark_read),
        )
        .route("/course/{course_id}/me", get(get_me))
        .route(
            "/course/{course_id}/acknowledge-privacy",
            post(acknowledge_privacy),
        )
        // Mirrors the Shibboleth chat router's live aegis analyzer
        // endpoint. Same `analyze_prompt_for_user` helper backs
        // both; the only difference here is the embed-token auth
        // flow + token in the body.
        .route("/course/{course_id}/aegis/analyze", post(analyze_prompt))
        // Mirrors the Shibboleth aegis rewrite route. Used by the
        // panel's "Some ideas" button to revise a draft with the
        // suggestions baked in.
        .route("/course/{course_id}/aegis/rewrite", post(rewrite_prompt))
        .route(
            "/course/{course_id}/suggested-questions",
            get(suggested_questions_handler),
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

//; Response types --

#[derive(Serialize)]
struct CourseResponse {
    id: Uuid,
    name: String,
    description: Option<String>,
    /// Per-course feature flags resolved through the same path the
    /// runtime uses. Lets the iframe-side frontend gate the aegis
    /// Feedback panel (and any future flags) without an extra
    /// round-trip. Same shape as the Shibboleth `/courses/{id}`
    /// route via `crate::routes::courses::CourseFeatureFlagsView`.
    feature_flags: CourseFeatureFlagsView,
}

#[derive(Serialize)]
struct ConversationResponse {
    id: Uuid,
    course_id: Uuid,
    title: Option<String>,
    created_at: chrono::DateTime<chrono::Utc>,
    updated_at: chrono::DateTime<chrono::Utc>,
    /// True when a teacher note attached to this conversation
    /// post-dates the owner's last view. Drives the unread dot
    /// in the embed sidebar; symmetrical with the Shibboleth
    /// route's `ConversationResponse`.
    has_unread_note: bool,
}

#[derive(Serialize)]
struct MessageResponse {
    id: Uuid,
    role: String,
    content: String,
    chunks_used: Option<serde_json::Value>,
    model_used: Option<String>,
    /// Research-phase thinking transcript persisted on the message
    /// (populated only for `tool_use_enabled` courses).
    thinking_transcript: Option<String>,
    /// JSONB array of `{name, args, result_summary}` records from
    /// the research phase; mirrors the chat route's shape so the
    /// embed UI can render the same "Thinking" disclosure.
    tool_events: Option<serde_json::Value>,
    /// Research-phase duration in milliseconds; surfaced so the
    /// embed UI can render "Thought for Ns" symmetrically with the
    /// regular chat.
    thinking_ms: Option<i32>,
    /// Research-phase prompt-token share of `tokens_prompt`; mirrors
    /// the chat-route field so the embed bubble can nest research /
    /// writeup under the prompt total.
    research_prompt_tokens: Option<i32>,
    /// Research-phase completion-token share of `tokens_completion`;
    /// mirrors the chat-route field.
    research_completion_tokens: Option<i32>,
    created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Serialize)]
struct TeacherNoteResponse {
    id: Uuid,
    conversation_id: Uuid,
    message_id: Option<Uuid>,
    author_id: Uuid,
    author_display_name: Option<String>,
    content: String,
    created_at: chrono::DateTime<chrono::Utc>,
    updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Serialize)]
struct ConversationDetailResponse {
    messages: Vec<MessageResponse>,
    notes: Vec<TeacherNoteResponse>,
    /// Aegis prompt-coaching analyses for this conversation, in the
    /// same shape as the Shibboleth route. Empty when aegis is off
    /// for the course or every turn so far soft-failed. Reuses the
    /// shared `PromptAnalysisResponse` so a schema change touches
    /// one place.
    prompt_analyses: Vec<PromptAnalysisResponse>,
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

//; Handlers --

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

    let feature_flags = resolve_course_flags(&state.db, course.id).await;
    Ok(Json(CourseResponse {
        id: course.id,
        name: course.name,
        description: course.description,
        feature_flags,
    }))
}

async fn suggested_questions_handler(
    State(state): State<AppState>,
    Path(course_id): Path<Uuid>,
    Query(query): Query<TokenQuery>,
) -> Result<Json<SuggestedQuestionsResponse>, AppError> {
    // Token already proves the bearer is a member of `course_id`.
    authenticate(&state, course_id, &query)?;
    let questions = suggested_questions::get_or_refresh(&state, course_id).await?;
    Ok(Json(SuggestedQuestionsResponse { questions }))
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
                has_unread_note: r.has_unread_note,
            })
            .collect(),
    ))
}

/// List pinned conversations for the embed view.
///
/// Embed token attests course membership (verified by `authenticate`),
/// so the only extra step before delegating to the shared helper is
/// promoting the DB row to a `User`; the embed route doesn't pass
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
    // Teacher notes are the whole point of pinning a conversation, so the
    // embed view has to surface them too. Author display name goes through
    // the same `ext:` pseudonymizer as the Shibboleth route.
    let notes = minerva_db::queries::conversations::list_notes(&state.db, cid).await?;
    // Aegis prompt analyses use the shared loader so this stays in
    // lockstep with the Shibboleth route's payload shape.
    let prompt_analyses = load_prompt_analyses_for_conversation(&state.db, cid).await;
    let ps = Pseudonymizer::for_viewer(&state.db, &viewer, &state.config.hmac_secret).await?;

    Ok(Json(ConversationDetailResponse {
        messages: messages
            .into_iter()
            .map(|m| MessageResponse {
                id: m.id,
                role: m.role,
                content: m.content,
                chunks_used: m.chunks_used,
                model_used: m.model_used,
                thinking_transcript: m.thinking_transcript,
                tool_events: m.tool_events,
                thinking_ms: m.thinking_ms,
                research_prompt_tokens: m.research_prompt_tokens,
                research_completion_tokens: m.research_completion_tokens,
                created_at: m.created_at,
            })
            .collect(),
        notes: notes
            .into_iter()
            .map(|n| {
                let (_, author_display_name) =
                    ext_obfuscate::apply(ps.as_ref(), n.author_id, None, n.author_display_name);
                TeacherNoteResponse {
                    id: n.id,
                    conversation_id: n.conversation_id,
                    message_id: n.message_id,
                    author_id: n.author_id,
                    author_display_name,
                    content: n.content,
                    created_at: n.created_at,
                    updated_at: n.updated_at,
                }
            })
            .collect(),
        prompt_analyses,
    }))
}

#[derive(Deserialize)]
struct SendMessageRequest {
    content: String,
    token: String,
    /// Aegis verdict the student had on screen when they pressed
    /// Send. Same field the Shibboleth route accepts; persisted
    /// linked to the new message_id for the History panel. None =
    /// no live analysis (debounce window, or aegis off for course).
    #[serde(default)]
    prompt_analysis: Option<AegisAnalysisPayload>,
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
        body.prompt_analysis,
    )
    .await
}

/// Stamp `student_last_viewed_at = NOW()` on the conversation for
/// the embed-token holder. Required steps: token must be valid,
/// must match the URL course_id, AND the user_id encoded in the
/// token must be the owner of the conversation. The last check is
/// what stops a (legitimate) embed user from clearing someone
/// else's unread state by guessing a conversation id.
async fn mark_read(
    State(state): State<AppState>,
    Path((course_id, cid)): Path<(Uuid, Uuid)>,
    Query(query): Query<TokenQuery>,
) -> Result<Json<serde_json::Value>, AppError> {
    let (_, user_id) = authenticate(&state, course_id, &query)?;
    let conv = minerva_db::queries::conversations::find_by_id(&state.db, cid)
        .await?
        .ok_or(AppError::NotFound)?;
    if conv.course_id != course_id {
        return Err(AppError::NotFound);
    }
    if conv.user_id != user_id {
        return Err(AppError::Forbidden);
    }
    minerva_db::queries::conversations::mark_student_viewed(&state.db, cid).await?;
    Ok(Json(serde_json::json!({ "ok": true })))
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
        body.prompt_analysis,
    )
    .await
}

#[derive(Deserialize)]
struct AnalyzePromptRequest {
    content: String,
    token: String,
    #[serde(default)]
    conversation_id: Option<Uuid>,
    /// Mirrors `chat::AnalyzePromptRequest::mode`. Defaults to
    /// Beginner so missing-field requests get the lenient grade.
    #[serde(default)]
    mode: AegisModeWire,
    /// Mirrors `chat::AnalyzePromptRequest::previous_suggestions`.
    /// The verdict the analyzer returned for the previous debounced
    /// fire on (a near-identical earlier version of) this same draft;
    /// the embed frontend ships it back so the model can avoid
    /// re-circling on dimensions it just coached. Defaults to empty
    /// for older clients.
    #[serde(default)]
    previous_suggestions: Vec<AegisSuggestionPayload>,
}

/// Embed-side wrapper around `chat::analyze_prompt_for_user`.
/// Auth flow differs (embed token in body); the analysis logic
/// itself is the shared helper, so the verdict shape and behaviour
/// stay in lockstep with the Shibboleth route.
async fn analyze_prompt(
    State(state): State<AppState>,
    Path(course_id): Path<Uuid>,
    Json(body): Json<AnalyzePromptRequest>,
) -> Result<Json<Option<AegisAnalysisPayload>>, AppError> {
    let (resolved_course_id, user_id) = verify_embed_token(&state.config.hmac_secret, &body.token)?;
    if resolved_course_id != course_id {
        return Err(AppError::Forbidden);
    }
    let verdict = analyze_prompt_for_user(
        &state,
        course_id,
        user_id,
        body.content,
        body.conversation_id,
        body.mode,
        body.previous_suggestions,
    )
    .await?;
    Ok(Json(verdict))
}

#[derive(Deserialize)]
struct RewritePromptRequest {
    content: String,
    token: String,
    #[serde(default)]
    suggestions: Vec<AegisSuggestionPayload>,
    #[serde(default)]
    mode: AegisModeWire,
    /// Optional conversation context (mirrors the Shibboleth rewrite
    /// route). Embed contexts almost never overlap with study mode,
    /// so this stays None for the common path and the umbrella gate
    /// decides; passed through for symmetry so a future embedded
    /// study can use it without another signature change.
    #[serde(default)]
    conversation_id: Option<Uuid>,
}

/// Embed-side wrapper around `chat::rewrite_prompt_for_user`.
/// Same shared helper, embed-token auth via the body.
async fn rewrite_prompt(
    State(state): State<AppState>,
    Path(course_id): Path<Uuid>,
    Json(body): Json<RewritePromptRequest>,
) -> Result<Json<AegisRewriteResponse>, AppError> {
    let (resolved_course_id, _user_id) =
        verify_embed_token(&state.config.hmac_secret, &body.token)?;
    if resolved_course_id != course_id {
        return Err(AppError::Forbidden);
    }
    let resp = rewrite_prompt_for_user(
        &state,
        course_id,
        body.content,
        body.suggestions,
        body.mode,
        body.conversation_id,
    )
    .await?;
    Ok(Json(resp))
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
