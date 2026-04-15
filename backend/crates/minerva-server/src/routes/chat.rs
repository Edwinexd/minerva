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

use std::collections::{HashMap, HashSet};

use crate::error::AppError;
use crate::ext_obfuscate::{self, Pseudonymizer};
use crate::routes::enforce_owner_cap;
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
        .route("/conversations/topics", get(popular_topics))
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
        .route(
            "/conversations/{cid}/messages/{message_id}/feedback",
            put(set_feedback),
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
struct MessageFeedbackResponse {
    id: Uuid,
    message_id: Uuid,
    user_id: Uuid,
    rating: String,
    category: Option<String>,
    comment: Option<String>,
    created_at: chrono::DateTime<chrono::Utc>,
    updated_at: chrono::DateTime<chrono::Utc>,
    user_eppn: Option<String>,
    user_display_name: Option<String>,
}

#[derive(Serialize)]
struct ConversationDetailResponse {
    messages: Vec<MessageResponse>,
    notes: Vec<TeacherNoteResponse>,
    feedback: Vec<MessageFeedbackResponse>,
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
    let ps = Pseudonymizer::for_viewer(&state.db, &user, &state.config.hmac_secret).await?;

    Ok(Json(
        rows.into_iter()
            .map(|r| {
                let (user_eppn, user_display_name) =
                    ext_obfuscate::apply(ps.as_ref(), r.user_id, r.user_eppn, r.user_display_name);
                ConversationWithUserResponse {
                    id: r.id,
                    course_id: r.course_id,
                    user_id: r.user_id,
                    title: r.title,
                    pinned: r.pinned,
                    created_at: r.created_at,
                    updated_at: r.updated_at,
                    user_eppn,
                    user_display_name,
                    message_count: r.message_count,
                }
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

    let is_teacher = is_course_teacher_or_admin(&state, course_id, &user).await?;
    let rows =
        minerva_db::queries::conversations::list_pinned_by_course(&state.db, course_id).await?;
    let ps = Pseudonymizer::for_viewer(&state.db, &user, &state.config.hmac_secret).await?;

    Ok(Json(
        rows.into_iter()
            .map(|r| {
                let is_own = r.user_id == user.id;
                let (raw_eppn, raw_display) = if is_teacher || is_own {
                    (r.user_eppn, r.user_display_name)
                } else {
                    (None, None)
                };
                let (user_eppn, user_display_name) =
                    ext_obfuscate::apply(ps.as_ref(), r.user_id, raw_eppn, raw_display);
                ConversationWithUserResponse {
                    id: r.id,
                    course_id: r.course_id,
                    user_id: r.user_id,
                    title: r.title,
                    pinned: r.pinned,
                    created_at: r.created_at,
                    updated_at: r.updated_at,
                    user_eppn,
                    user_display_name,
                    message_count: r.message_count,
                }
            })
            .collect(),
    ))
}

// ── Popular topics ──────────────────────────────────────────────────────

const STOP_WORDS: &[&str] = &[
    "a",
    "an",
    "the",
    "and",
    "or",
    "but",
    "in",
    "on",
    "at",
    "to",
    "for",
    "of",
    "with",
    "by",
    "from",
    "is",
    "are",
    "was",
    "were",
    "be",
    "been",
    "being",
    "have",
    "has",
    "had",
    "do",
    "does",
    "did",
    "will",
    "would",
    "could",
    "should",
    "may",
    "might",
    "shall",
    "can",
    "cannot",
    "not",
    "no",
    "it",
    "its",
    "i",
    "me",
    "my",
    "we",
    "our",
    "you",
    "your",
    "he",
    "she",
    "they",
    "them",
    "their",
    "this",
    "that",
    "these",
    "those",
    "what",
    "which",
    "who",
    "whom",
    "how",
    "when",
    "where",
    "why",
    "if",
    "then",
    "than",
    "so",
    "as",
    "about",
    "up",
    "out",
    "just",
    "also",
    "some",
    "any",
    "all",
    "each",
    "every",
    "more",
    "much",
    "many",
    "very",
    "too",
    "other",
    "into",
    "over",
    "after",
    "before",
    "between",
    "through",
    "during",
    "there",
    "here",
    "help",
    "question",
    "explain",
    "understand",
    "need",
    "want",
    "know",
    "tell",
    "please",
    "thanks",
    "hi",
    "hello",
    "hey",
    "like",
    "get",
    "make",
    "use",
    "using",
    "used",
    "way",
    "don",
    "doesn",
    "didn",
    "won",
    "wouldn",
    "couldn",
    "shouldn",
    "isn",
    "aren",
    "wasn",
    "weren",
    "hasn",
    "haven",
    "hadn",
    "can",
    "im",
    "ive",
    "youre",
    "thing",
    "things",
    "something",
    "anything",
    "nothing",
    "everything",
    "really",
    "actually",
    "basically",
    "think",
    "going",
    "try",
    "trying",
    "work",
    "working",
    "works",
    "example",
    "different",
    "same",
    "new",
];

fn tokenize(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric() && c != '\'')
        .filter(|w| w.len() > 2)
        .map(|w| w.trim_matches('\'').to_string())
        .filter(|w| w.len() > 2)
        .collect()
}

#[derive(Serialize)]
struct TopicResponse {
    topic: String,
    conversation_count: usize,
    unique_users: usize,
    total_messages: usize,
    conversation_ids: Vec<Uuid>,
}

async fn popular_topics(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(course_id): Path<Uuid>,
) -> Result<Json<Vec<TopicResponse>>, AppError> {
    verify_course_teacher_access(&state, course_id, &user).await?;

    // Fetch all user messages and conversation metadata
    let messages =
        minerva_db::queries::conversations::list_user_messages_by_course(&state.db, course_id)
            .await?;
    let all_convs =
        minerva_db::queries::conversations::list_all_by_course(&state.db, course_id).await?;

    // Build lookup: conversation_id -> (user_id, message_count)
    let conv_meta: HashMap<Uuid, (Uuid, i64)> = all_convs
        .iter()
        .map(|c| (c.id, (c.user_id, c.message_count.unwrap_or(0))))
        .collect();

    let stop: HashSet<&str> = STOP_WORDS.iter().copied().collect();

    // Group messages by conversation and extract tokens per conversation
    let mut conv_tokens: HashMap<Uuid, HashSet<String>> = HashMap::new();
    for msg in &messages {
        let tokens = tokenize(&msg.content);
        let entry = conv_tokens.entry(msg.conversation_id).or_default();
        for token in &tokens {
            if !stop.contains(token.as_str()) {
                entry.insert(token.clone());
            }
        }
        // Also extract bigrams
        for pair in tokens.windows(2) {
            let a = &pair[0];
            let b = &pair[1];
            if !stop.contains(a.as_str()) && !stop.contains(b.as_str()) {
                entry.insert(format!("{} {}", a, b));
            }
        }
    }

    // Count in how many conversations each term appears
    let mut term_convs: HashMap<String, HashSet<Uuid>> = HashMap::new();
    for (cid, tokens) in &conv_tokens {
        for token in tokens {
            term_convs.entry(token.clone()).or_default().insert(*cid);
        }
    }

    // Sort candidates: prefer bigrams, then by conversation count
    let mut candidates: Vec<(String, HashSet<Uuid>)> = term_convs
        .into_iter()
        .filter(|(_, convs)| convs.len() >= 2)
        .collect();
    candidates.sort_by(|a, b| {
        let a_bigram = a.0.contains(' ');
        let b_bigram = b.0.contains(' ');
        // Bigrams first, then by count desc
        b_bigram.cmp(&a_bigram).then(b.1.len().cmp(&a.1.len()))
    });

    // Greedily pick topics avoiding too much overlap
    let mut assigned: HashSet<Uuid> = HashSet::new();
    let mut topics: Vec<TopicResponse> = Vec::new();

    for (term, conv_ids) in &candidates {
        let unassigned: Vec<&Uuid> = conv_ids
            .iter()
            .filter(|id| !assigned.contains(id))
            .collect();
        if unassigned.len() < 2 {
            continue;
        }

        let mut unique_users = HashSet::new();
        let mut total_messages: usize = 0;
        let mut cids: Vec<Uuid> = Vec::new();

        for cid in conv_ids {
            if let Some((user_id, msg_count)) = conv_meta.get(cid) {
                unique_users.insert(*user_id);
                total_messages += *msg_count as usize;
            }
            cids.push(*cid);
        }

        topics.push(TopicResponse {
            topic: term.clone(),
            conversation_count: conv_ids.len(),
            unique_users: unique_users.len(),
            total_messages,
            conversation_ids: cids,
        });

        for cid in conv_ids {
            assigned.insert(*cid);
        }

        if topics.len() >= 15 {
            break;
        }
    }

    Ok(Json(topics))
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
    let feedback_rows =
        minerva_db::queries::message_feedback::list_for_conversation(&state.db, cid).await?;
    let ps = Pseudonymizer::for_viewer(&state.db, &user, &state.config.hmac_secret).await?;

    // Hide eppn/display_name from non-teachers viewing other students' feedback
    // on a pinned conversation; the conversation owner sees their own anyway.
    let feedback = feedback_rows
        .into_iter()
        .map(|f| {
            let is_own = f.user_id == user.id;
            let (raw_eppn, raw_display) = if is_teacher || is_own {
                (f.user_eppn, f.user_display_name)
            } else {
                (None, None)
            };
            let (user_eppn, user_display_name) =
                ext_obfuscate::apply(ps.as_ref(), f.user_id, raw_eppn, raw_display);
            MessageFeedbackResponse {
                id: f.id,
                message_id: f.message_id,
                user_id: f.user_id,
                rating: f.rating,
                category: f.category,
                comment: f.comment,
                created_at: f.created_at,
                updated_at: f.updated_at,
                user_eppn,
                user_display_name,
            }
        })
        .collect();

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
        feedback,
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

    // Every user must acknowledge the in-app data-handling disclosure before
    // sending their first message, regardless of role.
    if user.privacy_acknowledged_at.is_none() {
        return Err(AppError::PrivacyNotAcknowledged);
    }

    // Enforce per-student-per-course daily cap (0 = unlimited)
    if course.daily_token_limit > 0 {
        let used = minerva_db::queries::usage::get_user_daily_tokens(&state.db, user.id, course_id)
            .await?;
        if used >= course.daily_token_limit {
            return Err(AppError::QuotaExceeded);
        }
    }

    // Enforce the course owner's aggregate daily cap (sum across every
    // course they own). Acts as a sanity ceiling on a single teacher's
    // total AI spend regardless of per-course settings.
    enforce_owner_cap(&state, course.owner_id).await?;

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
        min_score: course.min_score,
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
        fastembed: Arc::clone(&state.fastembed),
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
    let ps = Pseudonymizer::for_viewer(&state.db, &user, &state.config.hmac_secret).await?;

    Ok(Json(
        notes
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
    let ps = Pseudonymizer::for_viewer(&state.db, &user, &state.config.hmac_secret).await?;
    let (_, author_display_name) =
        ext_obfuscate::apply(ps.as_ref(), note.author_id, None, note.author_display_name);

    Ok(Json(TeacherNoteResponse {
        id: note.id,
        conversation_id: note.conversation_id,
        message_id: note.message_id,
        author_id: note.author_id,
        author_display_name,
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

// Message feedback (thumbs up / thumbs down)

#[derive(Deserialize)]
struct SetFeedbackRequest {
    rating: String,
    category: Option<String>,
    comment: Option<String>,
}

async fn set_feedback(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path((course_id, cid, message_id)): Path<(Uuid, Uuid, Uuid)>,
    Json(body): Json<SetFeedbackRequest>,
) -> Result<Json<MessageFeedbackResponse>, AppError> {
    verify_course_access(&state, course_id, user.id).await?;

    let conv = minerva_db::queries::conversations::find_by_id(&state.db, cid)
        .await?
        .ok_or(AppError::NotFound)?;
    if conv.course_id != course_id || conv.user_id != user.id {
        return Err(AppError::Forbidden);
    }

    if body.rating != "up" && body.rating != "down" {
        return Err(AppError::BadRequest("rating must be 'up' or 'down'".into()));
    }

    // Ensure message belongs to this conversation and is an assistant message.
    let messages = minerva_db::queries::conversations::list_messages(&state.db, cid).await?;
    let msg = messages
        .iter()
        .find(|m| m.id == message_id)
        .ok_or(AppError::NotFound)?;
    if msg.role != "assistant" {
        return Err(AppError::BadRequest(
            "feedback only applies to assistant messages".into(),
        ));
    }

    let category = body.category.as_deref().filter(|s| !s.is_empty());
    let comment = body.comment.as_deref().filter(|s| !s.is_empty());

    let row = minerva_db::queries::message_feedback::upsert(
        &state.db,
        message_id,
        user.id,
        &body.rating,
        category,
        comment,
    )
    .await?;

    Ok(Json(MessageFeedbackResponse {
        id: row.id,
        message_id: row.message_id,
        user_id: row.user_id,
        rating: row.rating,
        category: row.category,
        comment: row.comment,
        created_at: row.created_at,
        updated_at: row.updated_at,
        user_eppn: Some(user.eppn.clone()),
        user_display_name: user.display_name.clone(),
    }))
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
