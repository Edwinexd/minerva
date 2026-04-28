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
            get(list_conversations).post(start_conversation),
        )
        .route("/conversations/all", get(list_all_conversations))
        .route("/conversations/pinned", get(list_pinned_conversations))
        .route("/conversations/topics", get(popular_topics))
        .route(
            "/conversations/feedback-stats",
            get(get_course_feedback_stats),
        )
        .route("/conversations/flag-kinds", get(list_flag_kinds))
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
        // Aegis live analyzer. Called from the frontend on debounced
        // input changes; the panel updates BEFORE the user hits Send.
        // No persistence happens here; the verdict the student
        // ultimately accepts gets persisted via the send-message body.
        .route("/aegis/analyze", post(analyze_prompt_route))
        // Aegis rewrite: takes the student's draft + the suggestions
        // already produced for it and asks the model to rewrite the
        // draft incorporating them. Drives the panel's "Some ideas"
        // button; one-click revision with auto-send.
        .route("/aegis/rewrite", post(rewrite_prompt_route))
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
pub(crate) struct ConversationWithUserResponse {
    pub id: Uuid,
    pub course_id: Uuid,
    pub user_id: Uuid,
    pub title: Option<String>,
    pub pinned: bool,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    pub user_eppn: Option<String>,
    pub user_display_name: Option<String>,
    pub message_count: Option<i64>,
}

#[derive(Serialize)]
struct ConversationWithFeedbackResponse {
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
    feedback_up: i64,
    feedback_down: i64,
    unaddressed_down: i64,
}

#[derive(Serialize)]
struct FeedbackCategoryCountItem {
    category: Option<String>,
    count: i64,
}

#[derive(Serialize)]
struct CourseFeedbackStatsResponse {
    total_up: i64,
    total_down: i64,
    categories: Vec<FeedbackCategoryCountItem>,
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
    generation_ms: Option<i32>,
    retrieval_count: Option<i32>,
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

/// Wire shape for an aegis verdict that flows in BOTH directions:
///
///   * **Server -> client** as the body of `POST /aegis/analyze`
///     (the live analyzer the frontend hits on debounced input
///     AND on Send to drive the just-in-time intercept).
///   * **Client -> server** as the optional `prompt_analysis`
///     field on `POST /conversations/.../message` (the verdict the
///     student saw at submit; persisted server-side and surfaced
///     as the History row for that turn).
///
/// Round-tripping the same struct keeps the live-vs-persisted
/// payloads byte-identical so the panel's typing-mode and
/// history-mode rendering share one code path.
///
/// We trust the client-supplied verdict on send. Re-running the
/// analyzer at send time would double cost per turn, and the
/// student is the only one who reads their own panel so a
/// manipulated payload only fools themselves; teacher dashboards
/// can re-derive truth from `course_token_usage` (category=aegis).
/// We DO clamp the suggestion array to AEGIS_SUGGESTIONS_MAX items
/// at insert time; the analyzer's schema enforces it, but
/// defending against a hand-crafted body costs nothing.
#[derive(Serialize, Deserialize, Clone)]
pub(crate) struct AegisAnalysisPayload {
    /// 0..=AEGIS_SUGGESTIONS_MAX suggestions. Empty = "looks good,
    /// no changes needed"; the panel renders an affirmation rather
    /// than nothing in that case. Each suggestion is a tagged
    /// single-sentence improvement plus a longer explanation.
    pub suggestions: Vec<AegisSuggestionPayload>,
    /// Calibration the analyzer was running under for this verdict
    /// ("beginner" | "expert"). Persisted on the History row so
    /// future UI can label "this analysis was made when you said
    /// you were a beginner" if useful. Defaults to `Beginner` so a
    /// missing-field client (older frontend) reads as the lenient
    /// rubric.
    #[serde(default)]
    pub mode: AegisModeWire,
}

#[derive(Serialize, Deserialize, Clone)]
pub(crate) struct AegisSuggestionPayload {
    /// Short tag the panel uses for grouping / iconography. The
    /// analyzer's JSON-schema enum is one of:
    /// "clarity" | "rationale" | "audience" | "format" | "tasks"
    /// | "instruction" | "examples" | "constraints". Free-form
    /// `String` in the wire shape so server-side enum extensions
    /// don't force a frontend release.
    pub kind: String,
    /// Importance: "high" | "medium" | "low". Drives the panel's
    /// per-card colour so the student sees which suggestions move
    /// the needle vs which are polish.
    pub severity: String,
    /// Single-sentence actionable improvement, second-person. The
    /// panel's collapsed default; one-liner.
    pub text: String,
    /// One to two sentences expanding on WHY the fix matters and
    /// what the student should consider when applying it. The
    /// panel reveals this on click-to-expand. `#[serde(default)]`
    /// keeps deserialisation forward-compatible with persisted
    /// rows from before the field landed (they decode to "").
    #[serde(default)]
    pub explanation: String,
}

impl AegisAnalysisPayload {
    fn from_verdict(v: crate::classification::aegis::AegisVerdict, mode: AegisModeWire) -> Self {
        Self {
            // Cap here too (the analyzer's system prompt says
            // 0..=AEGIS_SUGGESTIONS_MAX, but Cerebras strict-mode
            // schemas don't accept `maxItems` so we enforce in
            // code). Persistence path truncates again at insert;
            // belt-and-braces.
            suggestions: v
                .suggestions
                .into_iter()
                .take(crate::classification::aegis::AEGIS_SUGGESTIONS_MAX)
                .map(|s| AegisSuggestionPayload {
                    kind: s.kind,
                    severity: s.severity,
                    text: s.text,
                    explanation: s.explanation,
                })
                .collect(),
            mode,
        }
    }
}

/// Aegis prompt analysis for a single user message. Sent over the
/// chat detail endpoint so the right-rail Feedback panel's history
/// list comes from the same payload the messages do. Empty
/// vec when aegis is off for the course or every turn so far had
/// nothing worth suggesting.
///
/// Shared with the embed route via `pub(crate)` so the iframe-side
/// frontend gets the same shape without redefining the wire type.
#[derive(Serialize)]
pub(crate) struct PromptAnalysisResponse {
    pub(crate) id: Uuid,
    pub(crate) message_id: Uuid,
    /// 0..=AEGIS_SUGGESTIONS_MAX suggestions,
    /// oldest-most-relevant-first. Deserialised out of the DB's
    /// JSONB column. Pre-explanation rows decode with empty
    /// `explanation` strings via `#[serde(default)]` on the
    /// payload struct.
    pub(crate) suggestions: Vec<AegisSuggestionPayload>,
    /// "beginner" | "expert"; which calibration the analyzer was
    /// running under for this row.
    pub(crate) mode: String,
    pub(crate) created_at: chrono::DateTime<chrono::Utc>,
}

/// Shared DB-row -> wire-type mapper. JSONB column is opaque in the
/// DB layer; we deserialise here. A malformed row (e.g. someone
/// hand-edited the table) renders with an empty suggestion list
/// rather than 500-ing the whole conversation detail.
fn prompt_analysis_response_from_row(
    row: minerva_db::queries::prompt_analyses::PromptAnalysisRow,
) -> PromptAnalysisResponse {
    let suggestions: Vec<AegisSuggestionPayload> = serde_json::from_value(row.suggestions)
        .unwrap_or_else(|e| {
            tracing::warn!(
                "aegis: prompt_analyses.suggestions JSONB malformed for id={}: {}; rendering empty",
                row.id,
                e,
            );
            Vec::new()
        });
    PromptAnalysisResponse {
        id: row.id,
        message_id: row.message_id,
        suggestions,
        mode: row.mode,
        created_at: row.created_at,
    }
}

/// Load aegis prompt analyses for a conversation and convert them
/// to the shared wire shape. Soft-fails to an empty Vec on DB error
/// (logged at warn); the Feedback panel just renders nothing for
/// that conversation rather than 500-ing the whole detail load.
///
/// Shared between the Shibboleth chat detail route and the embed
/// route so both surface identical payloads to their panels.
pub(crate) async fn load_prompt_analyses_for_conversation(
    db: &sqlx::PgPool,
    cid: Uuid,
) -> Vec<PromptAnalysisResponse> {
    match minerva_db::queries::prompt_analyses::list_for_conversation(db, cid).await {
        Ok(rows) => rows
            .into_iter()
            .map(prompt_analysis_response_from_row)
            .collect(),
        Err(e) => {
            tracing::warn!(
                "aegis: list_for_conversation failed for {}: {}; rendering empty",
                cid,
                e,
            );
            Vec::new()
        }
    }
}

#[derive(Serialize)]
struct ConversationFlagResponse {
    id: Uuid,
    flag: String,
    /// 1-based index into the conversation's user-message stream.
    /// The frontend aligns each flag to the corresponding user
    /// message (and the assistant reply that followed) for the
    /// per-turn UI. Nullable because the schema is generic --
    /// future flag kinds may not be turn-scoped.
    turn_index: Option<i32>,
    /// Short human-readable string from whichever classifier
    /// produced the flag. Surfaced to teachers verbatim so they
    /// can sanity-check the model's judgement.
    rationale: Option<String>,
    /// Full JSON payload with classifier verdicts, matched
    /// assignment ids, etc. Renderable by the dashboard for the
    /// "details" pane.
    metadata: Option<serde_json::Value>,
    created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Serialize)]
struct ConversationDetailResponse {
    messages: Vec<MessageResponse>,
    notes: Vec<TeacherNoteResponse>,
    feedback: Vec<MessageFeedbackResponse>,
    /// Empty for non-teacher viewers (the conversation owner). The
    /// teacher dashboard reads this to render guard-trip badges
    /// and per-turn detail. Ordered oldest-first to match message
    /// order; same shape as `list_for_conversation` returns.
    flags: Vec<ConversationFlagResponse>,
    /// Aegis prompt-coaching scores, one per user message that the
    /// analyzer successfully scored. Empty when the `aegis` flag
    /// is off for the course (no rows ever get written) or every
    /// turn so far soft-failed. Ordered oldest-first.
    prompt_analyses: Vec<PromptAnalysisResponse>,
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

/// List all conversations in a course (teacher/admin only), with per-conversation feedback counts.
async fn list_all_conversations(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(course_id): Path<Uuid>,
) -> Result<Json<Vec<ConversationWithFeedbackResponse>>, AppError> {
    verify_course_teacher_access(&state, course_id, &user).await?;

    let rows =
        minerva_db::queries::conversations::list_all_by_course_with_feedback(&state.db, course_id)
            .await?;
    let ps = Pseudonymizer::for_viewer(&state.db, &user, &state.config.hmac_secret).await?;

    Ok(Json(
        rows.into_iter()
            .map(|r| {
                let (user_eppn, user_display_name) =
                    ext_obfuscate::apply(ps.as_ref(), r.user_id, r.user_eppn, r.user_display_name);
                ConversationWithFeedbackResponse {
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
                    feedback_up: r.feedback_up,
                    feedback_down: r.feedback_down,
                    unaddressed_down: r.unaddressed_down,
                }
            })
            .collect(),
    ))
}

/// Per-conversation flag-kind map for the teacher conversation
/// list page. Returns only the distinct flag *kinds* (e.g.
/// "extraction_attempt") attached to each conversation, not the
/// full flag rows; the list view only needs to know which
/// badges to render. Detailed per-turn flag data is fetched on
/// demand via `get_conversation`. Teacher/admin only.
async fn list_flag_kinds(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(course_id): Path<Uuid>,
) -> Result<Json<HashMap<Uuid, Vec<String>>>, AppError> {
    verify_course_teacher_access(&state, course_id, &user).await?;
    let map =
        minerva_db::queries::conversation_flags::flag_kinds_by_conversation(&state.db, course_id)
            .await?;
    Ok(Json(map))
}

/// Course-level feedback stats (teacher/admin only).
async fn get_course_feedback_stats(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(course_id): Path<Uuid>,
) -> Result<Json<CourseFeedbackStatsResponse>, AppError> {
    verify_course_teacher_access(&state, course_id, &user).await?;

    let summary =
        minerva_db::queries::message_feedback::total_ratings_for_course(&state.db, course_id)
            .await?;
    let categories =
        minerva_db::queries::message_feedback::category_counts_for_course(&state.db, course_id)
            .await?;

    Ok(Json(CourseFeedbackStatsResponse {
        total_up: summary.total_up,
        total_down: summary.total_down,
        categories: categories
            .into_iter()
            .map(|r| FeedbackCategoryCountItem {
                category: r.category,
                count: r.count,
            })
            .collect(),
    }))
}

/// Build the pinned-conversations payload for `viewer` on `course_id`.
///
/// Shared between the Shibboleth and embed routes so the
/// teacher-vs-non-teacher attribution rule, the `ext:`-viewer
/// pseudonymization, and the response shape can't drift apart.
/// Callers are responsible for verifying the viewer is allowed to see
/// the course (membership, embed token, etc.) before invoking this.
pub(crate) async fn list_pinned_conversations_for(
    state: &AppState,
    course_id: Uuid,
    viewer: &User,
) -> Result<Vec<ConversationWithUserResponse>, AppError> {
    let is_teacher = is_course_teacher_or_admin(state, course_id, viewer).await?;
    let rows =
        minerva_db::queries::conversations::list_pinned_by_course(&state.db, course_id).await?;
    let ps = Pseudonymizer::for_viewer(&state.db, viewer, &state.config.hmac_secret).await?;

    Ok(rows
        .into_iter()
        .map(|r| {
            let is_own = r.user_id == viewer.id;
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
        .collect())
}

/// List pinned conversations (any course member)
async fn list_pinned_conversations(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(course_id): Path<Uuid>,
) -> Result<Json<Vec<ConversationWithUserResponse>>, AppError> {
    verify_course_access(&state, course_id, user.id).await?;
    Ok(Json(
        list_pinned_conversations_for(&state, course_id, &user).await?,
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

/// Access guard for "show me this conversation" routes.
///
/// Returns the conversation row plus the viewer's teacher/admin status
/// (the latter is computed here anyway for the access check, and the
/// Shibboleth route reuses it for feedback pseudonymization downstream).
/// The rule is: owner of the conv, teacher/admin on the course, or the
/// conv is pinned by a teacher (which makes it readable by every course
/// member). Mismatched `course_id` is reported as 404; not 403 --
/// so a teacher of course A can't probe for cids in course B.
///
/// Shared between the Shibboleth and embed routes; the embed
/// `get_conversation` previously only allowed the owner, which 403'd
/// when a student opened a teacher-pinned chat from the sidebar.
pub(crate) async fn fetch_conversation_for_view(
    state: &AppState,
    course_id: Uuid,
    cid: Uuid,
    viewer: &User,
) -> Result<(minerva_db::queries::conversations::ConversationRow, bool), AppError> {
    let conv = minerva_db::queries::conversations::find_by_id(&state.db, cid)
        .await?
        .ok_or(AppError::NotFound)?;

    if conv.course_id != course_id {
        return Err(AppError::NotFound);
    }

    let is_teacher = is_course_teacher_or_admin(state, course_id, viewer).await?;
    if conv.user_id != viewer.id && !is_teacher && !conv.pinned {
        return Err(AppError::Forbidden);
    }

    Ok((conv, is_teacher))
}

async fn get_conversation(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path((course_id, cid)): Path<(Uuid, Uuid)>,
) -> Result<Json<ConversationDetailResponse>, AppError> {
    verify_course_access(&state, course_id, user.id).await?;

    let (_conv, is_teacher) = fetch_conversation_for_view(&state, course_id, cid, &user).await?;

    let messages = minerva_db::queries::conversations::list_messages(&state.db, cid).await?;
    let notes = minerva_db::queries::conversations::list_notes(&state.db, cid).await?;
    let feedback_rows =
        minerva_db::queries::message_feedback::list_for_conversation(&state.db, cid).await?;
    // Conversation flags are teacher-only by policy: a student
    // shouldn't see "you tripped the extraction guard at turn 3"
    // metadata about themselves; the rewrite already surfaces
    // the visible policy note to them. Empty Vec for non-teacher
    // viewers so the response shape stays stable for the typed
    // frontend client.
    let flag_rows = if is_teacher {
        minerva_db::queries::conversation_flags::list_for_conversation(&state.db, cid)
            .await
            .unwrap_or_default()
    } else {
        Vec::new()
    };

    // Aegis prompt analyses. Visible to whoever can see the
    // conversation (owner + teacher); the shared loader handles
    // soft-fail-to-empty so a DB hiccup doesn't 500 the detail.
    let prompt_analyses = load_prompt_analyses_for_conversation(&state.db, cid).await;
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
                generation_ms: m.generation_ms,
                retrieval_count: m.retrieval_count,
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
        flags: flag_rows
            .into_iter()
            .map(|f| ConversationFlagResponse {
                id: f.id,
                flag: f.flag,
                turn_index: f.turn_index,
                rationale: f.rationale,
                metadata: f.metadata,
                created_at: f.created_at,
            })
            .collect(),
        prompt_analyses,
    }))
}

#[derive(Deserialize)]
struct SendMessageRequest {
    content: String,
    /// Aegis verdict the student had on screen when they pressed
    /// Send. The frontend hits `POST /aegis/analyze` on debounced
    /// input and caches the latest verdict here, then ships it
    /// alongside the message so the History panel persists what
    /// the student actually saw. None when aegis is off for the
    /// course OR the user typed and sent inside the debounce window
    /// (no analysis ever produced); both are valid; the History
    /// row simply doesn't appear for that turn.
    #[serde(default)]
    prompt_analysis: Option<AegisAnalysisPayload>,
}

/// Request body for the live aegis analyzer.
#[derive(Deserialize)]
pub(crate) struct AnalyzePromptRequest {
    /// The prompt the student is currently typing. Empty / very
    /// short bodies are filtered server-side (the analyzer needs
    /// at least a few words to say anything useful).
    pub content: String,
    /// Optional conversation context. When provided, the analyzer
    /// gets the prior user turns of that conversation as context
    /// so a short follow-up like "explain that further" reads as
    /// well-grounded rather than missing-context.
    #[serde(default)]
    pub conversation_id: Option<Uuid>,
    /// Student's self-declared subject expertise. Calibrates the
    /// rubric server-side: a beginner gets graded leniently on
    /// terminology / pre-loaded context, an expert gets held to
    /// a higher bar for the same prompt. Passed verbatim from the
    /// frontend's panel toggle. Defaults to `Beginner` so a request
    /// from an older client (no field) gets the more lenient grade.
    #[serde(default)]
    pub mode: AegisModeWire,
}

/// Wire shape for `AegisMode`. `serde` reads this as the lower-cased
/// strings the frontend ships; `"beginner"` / `"expert"`; and
/// rejects anything else at deserialise time. Default is Beginner
/// so missing-field / older-client requests stay on the lenient
/// rubric (see `AnalyzePromptRequest::mode`). Serialize is needed
/// because the field round-trips through `AegisAnalysisPayload`
/// (server-to-client on /aegis/analyze, client-to-server on send).
#[derive(Serialize, Deserialize, Default, Clone, Copy)]
#[serde(rename_all = "lowercase")]
pub(crate) enum AegisModeWire {
    #[default]
    Beginner,
    Expert,
}

impl AegisModeWire {
    pub(crate) fn into_internal(self) -> crate::classification::aegis::AegisMode {
        match self {
            AegisModeWire::Beginner => crate::classification::aegis::AegisMode::Beginner,
            AegisModeWire::Expert => crate::classification::aegis::AegisMode::Expert,
        }
    }
}

/// Shared aegis-analyze pipeline used by both the Shibboleth and
/// embed routes. Caller is responsible for proving the user has
/// access to `course_id`; this helper trusts that and only handles
/// the flag check, the conversation scoping for context, and the
/// analyzer call.
///
/// Returns `Ok(None)` when:
///   * aegis is disabled for the course (the panel hides anyway)
///   * the analyzer soft-failed (transport / parse error)
///   * the prompt is too short for the analyzer to act on
///
/// `Err(AppError::NotFound)` / `Err(AppError::Forbidden)` only when
/// the supplied conversation_id is cross-course or non-owner --
/// mirrors `run_chat_message`'s IDOR guard.
pub(crate) async fn analyze_prompt_for_user(
    state: &AppState,
    course_id: Uuid,
    user_id: Uuid,
    content: String,
    conversation_id: Option<Uuid>,
    mode: AegisModeWire,
) -> Result<Option<AegisAnalysisPayload>, AppError> {
    if !crate::feature_flags::aegis_enabled(&state.db, course_id).await {
        return Ok(None);
    }

    // Build the trail oldest-first, current-turn-LAST. When a
    // conversation_id is given, scope-check it before pulling
    // history so a cross-course / cross-user id can't leak prior
    // turns through the analyzer's prompt.
    let mut trail: Vec<String> = Vec::new();
    if let Some(cid) = conversation_id {
        let conv = minerva_db::queries::conversations::find_by_id(&state.db, cid)
            .await?
            .ok_or(AppError::NotFound)?;
        if conv.course_id != course_id {
            return Err(AppError::NotFound);
        }
        if conv.user_id != user_id {
            return Err(AppError::Forbidden);
        }
        let history = minerva_db::queries::conversations::list_messages(&state.db, cid).await?;
        for m in history {
            if m.role == "user" {
                trail.push(m.content);
            }
        }
    }
    trail.push(content);

    let verdict = match crate::classification::aegis::analyze_prompt(
        &state.http_client,
        &state.config.cerebras_api_key,
        &state.db,
        course_id,
        &trail,
        mode.into_internal(),
    )
    .await
    {
        Ok(v) => v,
        Err(reason) => {
            // Upstream failure (Cerebras 4xx/5xx, malformed
            // response, etc.); bubble up as 500 so the frontend
            // and observability layer see a real failure rather
            // than the previous misleading 200+null. The detailed
            // reason rides into the log line via `Internal`'s
            // logging path; the client gets a generic 500 body.
            return Err(AppError::Internal(format!("aegis analyze: {reason}")));
        }
    };

    // Stamp the mode the analyzer ran under so the client (and
    // any persisted History row downstream) carries the calibration
    // label. We trust the request's `mode` here since it's the
    // value the analyzer just used.
    Ok(verdict.map(|v| AegisAnalysisPayload::from_verdict(v, mode)))
}

/// Live aegis analyzer endpoint (Shibboleth flow). Returns the
/// verdict synchronously so the panel can render before the user
/// clicks Send. The shape (`Option<AegisAnalysisPayload>`) matches
/// what the History list carries so panel rendering is uniform
/// between live-typing and persisted-history modes.
async fn analyze_prompt_route(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(course_id): Path<Uuid>,
    Json(body): Json<AnalyzePromptRequest>,
) -> Result<Json<Option<AegisAnalysisPayload>>, AppError> {
    verify_course_access(&state, course_id, user.id).await?;
    let verdict = analyze_prompt_for_user(
        &state,
        course_id,
        user.id,
        body.content,
        body.conversation_id,
        body.mode,
    )
    .await?;
    Ok(Json(verdict))
}

#[derive(Serialize)]
pub(crate) struct AegisRewriteResponse {
    /// The rewritten draft, ready to drop straight into the chat input.
    pub content: String,
}

/// Request body for the rewrite route.
#[derive(Deserialize)]
pub(crate) struct RewritePromptRequest {
    /// The student's current draft.
    pub content: String,
    /// The suggestions Aegis already produced for this draft. The
    /// frontend ships them back so the rewrite call doesn't have to
    /// re-analyze (saves an LLM call); the rewrite then incorporates
    /// these specific suggestions verbatim.
    #[serde(default)]
    pub suggestions: Vec<AegisSuggestionPayload>,
    /// Subject-expertise mode. Calibrates the rewrite's register
    /// (beginner stays casual; expert stays terse).
    #[serde(default)]
    pub mode: AegisModeWire,
}

/// Shared rewrite pipeline. Caller is responsible for proving the
/// user has access to `course_id`. Returns a 500 (`AppError::Internal`)
/// on upstream failure so the frontend can surface a real error
/// rather than silently failing back to the original draft.
pub(crate) async fn rewrite_prompt_for_user(
    state: &AppState,
    course_id: Uuid,
    content: String,
    suggestions: Vec<AegisSuggestionPayload>,
    mode: AegisModeWire,
) -> Result<AegisRewriteResponse, AppError> {
    if !crate::feature_flags::aegis_enabled(&state.db, course_id).await {
        // Aegis off -> rewrite makes no sense to expose. 404 reads
        // cleaner than 400 here since the route conceptually
        // doesn't exist for this course.
        return Err(AppError::NotFound);
    }
    // Map wire-shape suggestions to the analyzer's internal struct
    // for the LLM call. The two structs are field-identical; the
    // explicit map keeps the layers decoupled in case one shape
    // grows fields the other shouldn't see.
    let analyzer_suggestions: Vec<crate::classification::aegis::AegisSuggestion> = suggestions
        .into_iter()
        .map(|s| crate::classification::aegis::AegisSuggestion {
            kind: s.kind,
            severity: s.severity,
            text: s.text,
            explanation: s.explanation,
        })
        .collect();

    match crate::classification::aegis::rewrite_prompt(
        &state.http_client,
        &state.config.cerebras_api_key,
        &state.db,
        course_id,
        &content,
        &analyzer_suggestions,
        mode.into_internal(),
    )
    .await
    {
        Ok(rewritten) => Ok(AegisRewriteResponse { content: rewritten }),
        Err(reason) => Err(AppError::Internal(format!("aegis rewrite: {reason}"))),
    }
}

async fn rewrite_prompt_route(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(course_id): Path<Uuid>,
    Json(body): Json<RewritePromptRequest>,
) -> Result<Json<AegisRewriteResponse>, AppError> {
    verify_course_access(&state, course_id, user.id).await?;
    let resp =
        rewrite_prompt_for_user(&state, course_id, body.content, body.suggestions, body.mode)
            .await?;
    Ok(Json(resp))
}

async fn send_message(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path((course_id, cid)): Path<(Uuid, Uuid)>,
    Json(body): Json<SendMessageRequest>,
) -> Result<Sse<Pin<Box<dyn Stream<Item = Result<Event, AppError>> + Send>>>, AppError> {
    let course = verify_course_access(&state, course_id, user.id).await?;
    run_chat_message(
        &state,
        course,
        user.id,
        user.privacy_acknowledged_at,
        Some(cid),
        body.content,
        body.prompt_analysis,
    )
    .await
}

async fn start_conversation(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(course_id): Path<Uuid>,
    Json(body): Json<SendMessageRequest>,
) -> Result<Sse<Pin<Box<dyn Stream<Item = Result<Event, AppError>> + Send>>>, AppError> {
    let course = verify_course_access(&state, course_id, user.id).await?;
    run_chat_message(
        &state,
        course,
        user.id,
        user.privacy_acknowledged_at,
        None,
        body.content,
        body.prompt_analysis,
    )
    .await
}

/// Shared post-auth message pipeline used by both the Shibboleth and embed
/// routes. Caller is responsible for proving the user has access to
/// `course`; this helper trusts that and only enforces conv scoping,
/// privacy ack, daily caps, and message persistence.
///
/// `cid = Some(_)` appends to that conversation; `None` lazily creates a
/// new one *after* all gates pass and signals the freshly-minted id to
/// the client as the stream's first SSE event:
/// `data: {"type":"conversation_created","id":"<uuid>"}`. Server-side id
/// generation keeps clients out of the trust boundary for resource
/// identifiers and means a rejected first message leaves no orphan
/// "Untitled, 0 msgs" row.
/// `prompt_analysis`: aegis verdict the student had on screen when
/// they pressed Send (cached client-side from the live
/// `/aegis/analyze` endpoint). None = no live analysis to persist;
/// History row for this turn simply doesn't appear, which is correct
/// behaviour for "user typed-and-sent inside the debounce window".
pub(super) async fn run_chat_message(
    state: &AppState,
    course: minerva_db::queries::courses::CourseRow,
    user_id: Uuid,
    user_privacy_acknowledged_at: Option<chrono::DateTime<chrono::Utc>>,
    cid: Option<Uuid>,
    user_content: String,
    prompt_analysis: Option<AegisAnalysisPayload>,
) -> Result<Sse<Pin<Box<dyn Stream<Item = Result<Event, AppError>> + Send>>>, AppError> {
    let course_id = course.id;

    // For an existing-conv send, scope-check up front (mirrors the IDOR
    // fix in get_conversation): cross-course id -> NotFound, same-course
    // but other-user -> Forbidden. The new-conv path skips this since the
    // id doesn't exist yet.
    let existing = if let Some(cid) = cid {
        let row = minerva_db::queries::conversations::find_by_id(&state.db, cid)
            .await?
            .ok_or(AppError::NotFound)?;
        if row.course_id != course_id {
            return Err(AppError::NotFound);
        }
        if row.user_id != user_id {
            return Err(AppError::Forbidden);
        }
        Some(row)
    } else {
        None
    };

    if user_privacy_acknowledged_at.is_none() {
        return Err(AppError::PrivacyNotAcknowledged);
    }

    if course.daily_token_limit > 0 {
        let used = minerva_db::queries::usage::get_user_daily_tokens(&state.db, user_id, course_id)
            .await?;
        if used >= course.daily_token_limit {
            return Err(AppError::QuotaExceeded);
        }
    }

    enforce_owner_cap(state, course.owner_id).await?;

    // Existing conv: reuse the row already loaded. New conv: server picks
    // the id and inserts. Either way, by this point the conv definitely
    // exists in the DB before we save the user message.
    let (conv, was_created) = match existing {
        Some(c) => (c, false),
        None => {
            let new_id = Uuid::new_v4();
            let row =
                minerva_db::queries::conversations::create(&state.db, new_id, course_id, user_id)
                    .await?;
            (row, true)
        }
    };
    let conv_id = conv.id;

    let user_msg_id = Uuid::new_v4();
    minerva_db::queries::conversations::insert_message(
        &state.db,
        user_msg_id,
        conv_id,
        "user",
        &user_content,
        None,
        None,
        None,
        None,
        None,
        None,
    )
    .await?;

    let history = minerva_db::queries::conversations::list_messages(&state.db, conv_id).await?;
    let is_first_message = history.len() <= 1;

    let (tx, rx) = mpsc::channel::<Result<Event, AppError>>(32);

    // Front-load the new-conv id so the client learns it before any tokens
    // arrive. The 32-slot channel buffer fits this without blocking the
    // strategy spawn.
    if was_created {
        let payload = serde_json::json!({
            "type": "conversation_created",
            "id": conv_id,
        });
        let _ = tx
            .send(Ok(Event::default().data(payload.to_string())))
            .await;
    }

    let strategy_name = course.strategy.clone();

    // Resolve the KG feature flag once per chat request and pin it
    // into the strategy context. This both saves a DB lookup per
    // partition call and guarantees a stable view across the run --
    // an admin flipping the flag mid-conversation won't half-apply.
    let kg_enabled = crate::feature_flags::course_kg_enabled(&state.db, course_id).await;

    // Aegis: persist the verdict the student had on screen when
    // they pressed Send so it appears in the History panel for
    // this conversation. The analysis itself was produced earlier
    // by `POST /aegis/analyze` (live, no persist); the frontend
    // caches it and ships it here, atomic with the message body.
    //
    // We persist iff the flag is on AND the client supplied a
    // verdict. Skipping the flag check would let a client write
    // rows for courses where aegis is meant to be invisible; the
    // CHECK constraints + the read-side flag gate make this
    // double-belt-and-braces.
    //
    // Soft-fail: a DB hiccup logs at warn and drops the row.
    // The student already saw the panel during typing; missing
    // a History entry is the right failure mode.
    if let Some(analysis) = prompt_analysis {
        if crate::feature_flags::aegis_enabled(&state.db, course_id).await {
            // Trim to the same ceiling the analyzer schema
            // enforces, in case a hand-crafted body exceeds.
            let mut suggestions = analysis.suggestions;
            suggestions.truncate(crate::classification::aegis::AEGIS_SUGGESTIONS_MAX);
            // Serialise once for the JSONB column. Failure here is
            // theoretical (the struct derives Serialize) but we
            // log+drop rather than panic to keep the message-send
            // hot path bulletproof.
            match serde_json::to_value(&suggestions) {
                Ok(suggestions_json) => {
                    let mode_str = analysis.mode.into_internal().as_str();
                    if let Err(e) = minerva_db::queries::prompt_analyses::insert(
                        &state.db,
                        minerva_db::queries::prompt_analyses::PromptAnalysisInsert {
                            message_id: user_msg_id,
                            suggestions: &suggestions_json,
                            mode: mode_str,
                            model_used: crate::classification::aegis::AEGIS_MODEL,
                        },
                    )
                    .await
                    {
                        tracing::warn!(
                            "aegis: prompt_analyses insert failed (conv={}, msg={}): {}",
                            conv_id,
                            user_msg_id,
                            e,
                        );
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        "aegis: prompt_analyses serialise failed (conv={}, msg={}): {}",
                        conv_id,
                        user_msg_id,
                        e,
                    );
                }
            }
        }
    }

    let ctx = strategy::GenerationContext {
        course_name: course.name,
        custom_prompt: course.system_prompt,
        model: course.model,
        temperature: course.temperature,
        max_chunks: course.max_chunks,
        min_score: course.min_score,
        course_id,
        conversation_id: conv_id,
        user_id: conv.user_id,
        cerebras_api_key: state.config.cerebras_api_key.clone(),
        cerebras_base_url: strategy::common::CEREBRAS_CHAT_COMPLETIONS_URL.to_string(),
        openai_api_key: state.config.openai_api_key.clone(),
        embedding_provider: course.embedding_provider,
        embedding_model: course.embedding_model,
        embedding_version: course.embedding_version,
        history,
        user_content,
        is_first_message,
        daily_token_limit: course.daily_token_limit,
        db: state.db.clone(),
        qdrant: Arc::clone(&state.qdrant),
        fastembed: Arc::clone(&state.fastembed),
        kg_enabled,
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

    // Scope the conversation to the URL's course so a teacher/admin of one
    // course cannot list notes from conversations in another course.
    if conv.course_id != course_id {
        return Err(AppError::NotFound);
    }

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
    Path((course_id, cid, note_id)): Path<(Uuid, Uuid, Uuid)>,
    Json(body): Json<UpdateNoteRequest>,
) -> Result<Json<TeacherNoteResponse>, AppError> {
    verify_course_teacher_access(&state, course_id, &user).await?;

    // Verify the note lives in the conversation from the URL, and the
    // conversation lives in the URL's course. Without this, a teacher of
    // course A could overwrite a note in course B by putting B's note_id in
    // the path.
    let conv = minerva_db::queries::conversations::find_by_id(&state.db, cid)
        .await?
        .ok_or(AppError::NotFound)?;
    if conv.course_id != course_id {
        return Err(AppError::NotFound);
    }
    let existing = minerva_db::queries::conversations::find_note_by_id(&state.db, note_id)
        .await?
        .ok_or(AppError::NotFound)?;
    if existing.conversation_id != cid {
        return Err(AppError::NotFound);
    }

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
    Path((course_id, cid, note_id)): Path<(Uuid, Uuid, Uuid)>,
) -> Result<Json<serde_json::Value>, AppError> {
    verify_course_teacher_access(&state, course_id, &user).await?;

    // Verify the note lives in the conversation from the URL, and the
    // conversation lives in the URL's course. Without this, a teacher of
    // course A could delete a note in course B by putting B's note_id in
    // the path.
    let conv = minerva_db::queries::conversations::find_by_id(&state.db, cid)
        .await?
        .ok_or(AppError::NotFound)?;
    if conv.course_id != course_id {
        return Err(AppError::NotFound);
    }
    let existing = minerva_db::queries::conversations::find_note_by_id(&state.db, note_id)
        .await?
        .ok_or(AppError::NotFound)?;
    if existing.conversation_id != cid {
        return Err(AppError::NotFound);
    }

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
        return Err(AppError::bad_request("chat.rating_invalid"));
    }

    // Ensure message belongs to this conversation and is an assistant message.
    let messages = minerva_db::queries::conversations::list_messages(&state.db, cid).await?;
    let msg = messages
        .iter()
        .find(|m| m.id == message_id)
        .ok_or(AppError::NotFound)?;
    if msg.role != "assistant" {
        return Err(AppError::bad_request("chat.feedback_only_assistant"));
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
