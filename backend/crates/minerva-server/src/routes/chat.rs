use axum::extract::{Extension, Path, State};
use axum::response::sse::{Event, Sse};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use futures::Stream;
use minerva_core::models::User;
use serde::{Deserialize, Serialize};
use std::pin::Pin;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use uuid::Uuid;

use crate::error::AppError;
use crate::state::AppState;
use crate::strategy;

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/conversations",
            get(list_conversations).post(create_conversation),
        )
        .route("/conversations/all", get(list_all_conversations))
        .route("/conversations/pinned", get(list_pinned_conversations))
        .route("/conversations/{cid}", get(get_conversation))
        .route("/conversations/{cid}/message", post(send_message))
        .route("/conversations/{cid}/pin", put(set_pin))
        .route(
            "/conversations/{cid}/notes",
            get(list_notes).post(create_note),
        )
        .route(
            "/conversations/{cid}/notes/{note_id}",
            put(update_note).delete(delete_note),
        )
}

#[derive(Serialize)]
struct ConversationResponse {
    id: Uuid,
    course_id: Uuid,
    title: Option<String>,
    pinned: bool,
    created_at: chrono::DateTime<chrono::Utc>,
    updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Serialize)]
struct ConversationWithUserResponse {
    id: Uuid,
    course_id: Uuid,
    user_id: Uuid,
    title: Option<String>,
    pinned: bool,
    created_at: chrono::DateTime<chrono::Utc>,
    updated_at: chrono::DateTime<chrono::Utc>,
    user_eppn: Option<String>,
    user_display_name: Option<String>,
    message_count: Option<i64>,
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
                pinned: r.pinned,
                created_at: r.created_at,
                updated_at: r.updated_at,
            })
            .collect(),
    ))
}

/// List all conversations in a course (teacher/admin only)
async fn list_all_conversations(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(course_id): Path<Uuid>,
) -> Result<Json<Vec<ConversationWithUserResponse>>, AppError> {
    verify_course_teacher_access(&state, course_id, &user).await?;

    let rows = minerva_db::queries::conversations::list_all_by_course(&state.db, course_id).await?;

    Ok(Json(
        rows.into_iter()
            .map(|r| ConversationWithUserResponse {
                id: r.id,
                course_id: r.course_id,
                user_id: r.user_id,
                title: r.title,
                pinned: r.pinned,
                created_at: r.created_at,
                updated_at: r.updated_at,
                user_eppn: r.user_eppn,
                user_display_name: r.user_display_name,
                message_count: r.message_count,
            })
            .collect(),
    ))
}

/// List pinned conversations (any course member)
async fn list_pinned_conversations(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(course_id): Path<Uuid>,
) -> Result<Json<Vec<ConversationWithUserResponse>>, AppError> {
    verify_course_access(&state, course_id, user.id).await?;

    let rows =
        minerva_db::queries::conversations::list_pinned_by_course(&state.db, course_id).await?;

    Ok(Json(
        rows.into_iter()
            .map(|r| ConversationWithUserResponse {
                id: r.id,
                course_id: r.course_id,
                user_id: r.user_id,
                title: r.title,
                pinned: r.pinned,
                created_at: r.created_at,
                updated_at: r.updated_at,
                user_eppn: r.user_eppn,
                user_display_name: r.user_display_name,
                message_count: r.message_count,
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
    let row = minerva_db::queries::conversations::create(&state.db, id, course_id, user.id).await?;

    Ok(Json(ConversationResponse {
        id: row.id,
        course_id: row.course_id,
        title: row.title,
        pinned: row.pinned,
        created_at: row.created_at,
        updated_at: row.updated_at,
    }))
}

async fn get_conversation(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path((course_id, cid)): Path<(Uuid, Uuid)>,
) -> Result<Json<ConversationDetailResponse>, AppError> {
    verify_course_access(&state, course_id, user.id).await?;

    let conv = minerva_db::queries::conversations::find_by_id(&state.db, cid)
        .await?
        .ok_or(AppError::NotFound)?;

    // Owner, admin, course teacher, or pinned conversation
    let is_teacher = is_course_teacher_or_admin(&state, course_id, &user).await?;
    if conv.user_id != user.id && !is_teacher && !conv.pinned {
        return Err(AppError::Forbidden);
    }

    let messages = minerva_db::queries::conversations::list_messages(&state.db, cid).await?;
    let notes = minerva_db::queries::conversations::list_notes(&state.db, cid).await?;

    Ok(Json(ConversationDetailResponse {
        messages: messages
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
        notes: notes
            .into_iter()
            .map(|n| TeacherNoteResponse {
                id: n.id,
                conversation_id: n.conversation_id,
                message_id: n.message_id,
                author_id: n.author_id,
                author_display_name: n.author_display_name,
                content: n.content,
                created_at: n.created_at,
                updated_at: n.updated_at,
            })
            .collect(),
    }))
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

// Pin/unpin

#[derive(Deserialize)]
struct SetPinRequest {
    pinned: bool,
}

async fn set_pin(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path((course_id, cid)): Path<(Uuid, Uuid)>,
    Json(body): Json<SetPinRequest>,
) -> Result<Json<ConversationResponse>, AppError> {
    verify_course_teacher_access(&state, course_id, &user).await?;

    let conv = minerva_db::queries::conversations::find_by_id(&state.db, cid)
        .await?
        .ok_or(AppError::NotFound)?;

    if conv.course_id != course_id {
        return Err(AppError::NotFound);
    }

    let row = minerva_db::queries::conversations::set_pinned(&state.db, cid, body.pinned)
        .await?
        .ok_or(AppError::NotFound)?;

    Ok(Json(ConversationResponse {
        id: row.id,
        course_id: row.course_id,
        title: row.title,
        pinned: row.pinned,
        created_at: row.created_at,
        updated_at: row.updated_at,
    }))
}

// Teacher notes

#[derive(Deserialize)]
struct CreateNoteRequest {
    message_id: Option<Uuid>,
    content: String,
}

#[derive(Deserialize)]
struct UpdateNoteRequest {
    content: String,
}

async fn list_notes(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path((course_id, cid)): Path<(Uuid, Uuid)>,
) -> Result<Json<Vec<TeacherNoteResponse>>, AppError> {
    verify_course_access(&state, course_id, user.id).await?;

    let conv = minerva_db::queries::conversations::find_by_id(&state.db, cid)
        .await?
        .ok_or(AppError::NotFound)?;

    // Anyone can see notes on pinned conversations, or own conversations, or teachers
    let is_teacher = is_course_teacher_or_admin(&state, course_id, &user).await?;
    if conv.user_id != user.id && !is_teacher && !conv.pinned {
        return Err(AppError::Forbidden);
    }

    let notes = minerva_db::queries::conversations::list_notes(&state.db, cid).await?;

    Ok(Json(
        notes
            .into_iter()
            .map(|n| TeacherNoteResponse {
                id: n.id,
                conversation_id: n.conversation_id,
                message_id: n.message_id,
                author_id: n.author_id,
                author_display_name: n.author_display_name,
                content: n.content,
                created_at: n.created_at,
                updated_at: n.updated_at,
            })
            .collect(),
    ))
}

async fn create_note(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path((course_id, cid)): Path<(Uuid, Uuid)>,
    Json(body): Json<CreateNoteRequest>,
) -> Result<Json<TeacherNoteResponse>, AppError> {
    verify_course_teacher_access(&state, course_id, &user).await?;

    let conv = minerva_db::queries::conversations::find_by_id(&state.db, cid)
        .await?
        .ok_or(AppError::NotFound)?;

    if conv.course_id != course_id {
        return Err(AppError::NotFound);
    }

    let id = Uuid::new_v4();
    let note = minerva_db::queries::conversations::create_note(
        &state.db,
        id,
        cid,
        body.message_id,
        user.id,
        &body.content,
    )
    .await?;

    Ok(Json(TeacherNoteResponse {
        id: note.id,
        conversation_id: note.conversation_id,
        message_id: note.message_id,
        author_id: note.author_id,
        author_display_name: note.author_display_name,
        content: note.content,
        created_at: note.created_at,
        updated_at: note.updated_at,
    }))
}

async fn update_note(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path((course_id, _cid, note_id)): Path<(Uuid, Uuid, Uuid)>,
    Json(body): Json<UpdateNoteRequest>,
) -> Result<Json<TeacherNoteResponse>, AppError> {
    verify_course_teacher_access(&state, course_id, &user).await?;

    let note = minerva_db::queries::conversations::update_note(&state.db, note_id, &body.content)
        .await?
        .ok_or(AppError::NotFound)?;

    Ok(Json(TeacherNoteResponse {
        id: note.id,
        conversation_id: note.conversation_id,
        message_id: note.message_id,
        author_id: note.author_id,
        author_display_name: note.author_display_name,
        content: note.content,
        created_at: note.created_at,
        updated_at: note.updated_at,
    }))
}

async fn delete_note(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path((course_id, _cid, note_id)): Path<(Uuid, Uuid, Uuid)>,
) -> Result<Json<serde_json::Value>, AppError> {
    verify_course_teacher_access(&state, course_id, &user).await?;

    let deleted = minerva_db::queries::conversations::delete_note(&state.db, note_id).await?;
    Ok(Json(serde_json::json!({ "deleted": deleted })))
}

// Access helpers

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

async fn is_course_teacher_or_admin(
    state: &AppState,
    course_id: Uuid,
    user: &User,
) -> Result<bool, AppError> {
    if user.role.is_admin() {
        return Ok(true);
    }
    let course = minerva_db::queries::courses::find_by_id(&state.db, course_id)
        .await?
        .ok_or(AppError::NotFound)?;
    if course.owner_id == user.id {
        return Ok(true);
    }
    Ok(minerva_db::queries::courses::is_course_teacher(&state.db, course_id, user.id).await?)
}

async fn verify_course_teacher_access(
    state: &AppState,
    course_id: Uuid,
    user: &User,
) -> Result<minerva_db::queries::courses::CourseRow, AppError> {
    let course = minerva_db::queries::courses::find_by_id(&state.db, course_id)
        .await?
        .ok_or(AppError::NotFound)?;

    if !user.role.is_admin()
        && course.owner_id != user.id
        && !minerva_db::queries::courses::is_course_teacher(&state.db, course_id, user.id).await?
    {
        return Err(AppError::Forbidden);
    }

    Ok(course)
}
