use sqlx::PgPool;
use uuid::Uuid;

#[derive(Debug)]
pub struct ConversationRow {
    pub id: Uuid,
    pub course_id: Uuid,
    pub user_id: Uuid,
    pub title: Option<String>,
    pub pinned: bool,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug)]
pub struct StudentSidebarConversationRow {
    pub id: Uuid,
    pub course_id: Uuid,
    pub user_id: Uuid,
    pub title: Option<String>,
    pub pinned: bool,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    /// True iff a teacher_note attached to this conversation has
    /// a `created_at` newer than the owner's
    /// `student_last_viewed_at` (or the latter is NULL = never
    /// visited yet). Drives the unread dot on the student-side
    /// chat sidebar.
    pub has_unread_note: bool,
}

#[derive(Debug)]
pub struct ConversationWithUserRow {
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

#[derive(Debug)]
pub struct TeacherNoteRow {
    pub id: Uuid,
    pub conversation_id: Uuid,
    pub message_id: Option<Uuid>,
    pub author_id: Uuid,
    pub content: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    pub author_display_name: Option<String>,
}

#[derive(Debug)]
pub struct MessageRow {
    pub id: Uuid,
    pub conversation_id: Uuid,
    pub role: String,
    pub content: String,
    pub chunks_used: Option<serde_json::Value>,
    pub model_used: Option<String>,
    pub tokens_prompt: Option<i32>,
    pub tokens_completion: Option<i32>,
    pub generation_ms: Option<i32>,
    pub retrieval_count: Option<i32>,
    /// Concatenated `thinking_token` SSE stream emitted during the
    /// research phase (only populated for `tool_use_enabled` courses).
    /// NULL on legacy single-pass messages; the frontend renders no
    /// disclosure in that case.
    pub thinking_transcript: Option<String>,
    /// JSONB array of `{name, args, result_summary}` triples ordered
    /// by tool-call emission. NULL on legacy single-pass messages.
    pub tool_events: Option<serde_json::Value>,
    /// Wall-clock duration of the research phase in milliseconds.
    /// NULL on legacy single-pass messages; persisted alongside the
    /// transcript so the frontend can render "Thought for Ns" on
    /// past messages, not just the in-progress one.
    pub thinking_ms: Option<i32>,
    /// Prompt tokens consumed by the research/agentic phase (the
    /// tool-using step before writeup). NULL on legacy single-pass
    /// messages and on user messages. Writeup prompt share is
    /// `tokens_prompt - research_prompt_tokens`.
    pub research_prompt_tokens: Option<i32>,
    /// Completion tokens consumed by the research/agentic phase.
    /// NULL on legacy single-pass messages and on user messages.
    /// Writeup completion share is `tokens_completion -
    /// research_completion_tokens`.
    pub research_completion_tokens: Option<i32>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

pub async fn create(
    db: &PgPool,
    id: Uuid,
    course_id: Uuid,
    user_id: Uuid,
) -> Result<ConversationRow, sqlx::Error> {
    sqlx::query_as!(
        ConversationRow,
        "INSERT INTO conversations (id, course_id, user_id) VALUES ($1, $2, $3) RETURNING id, course_id, user_id, title, pinned, created_at, updated_at",
        id,
        course_id,
        user_id,
    )
    .fetch_one(db)
    .await
}

pub async fn list_by_course_user(
    db: &PgPool,
    course_id: Uuid,
    user_id: Uuid,
) -> Result<Vec<StudentSidebarConversationRow>, sqlx::Error> {
    // `has_unread_note` is a single correlated EXISTS per row;
    // cheap because `teacher_notes` is indexed by conversation_id
    // and we short-circuit on the first match. Two truth cases:
    //   1. The owner has never visited (NULL last-viewed) AND a
    //      note exists at all → unread.
    //   2. A note's `created_at` is strictly newer than the
    //      stored last-viewed → unread.
    // Acked / pre-existing notes that pre-date the migration
    // backfill don't fire spuriously because the backfill set
    // `student_last_viewed_at = NOW()` on every existing row.
    sqlx::query_as!(
        StudentSidebarConversationRow,
        r#"SELECT c.id, c.course_id, c.user_id, c.title, c.pinned,
            c.created_at, c.updated_at,
            EXISTS (
                SELECT 1 FROM teacher_notes tn
                WHERE tn.conversation_id = c.id
                  AND (
                      c.student_last_viewed_at IS NULL
                      OR tn.created_at > c.student_last_viewed_at
                  )
            ) AS "has_unread_note!: bool"
        FROM conversations c
        WHERE c.course_id = $1 AND c.user_id = $2
        ORDER BY c.updated_at DESC"#,
        course_id,
        user_id,
    )
    .fetch_all(db)
    .await
}

pub async fn find_by_id(db: &PgPool, id: Uuid) -> Result<Option<ConversationRow>, sqlx::Error> {
    sqlx::query_as!(
        ConversationRow,
        "SELECT id, course_id, user_id, title, pinned, created_at, updated_at FROM conversations WHERE id = $1",
        id,
    )
    .fetch_optional(db)
    .await
}

#[derive(Debug)]
pub struct ConversationWithFeedbackRow {
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
    pub feedback_up: i64,
    pub feedback_down: i64,
    /// Down-vote feedback rows for this conversation that have
    /// neither been explicitly acknowledged nor have a teacher
    /// note attached to the same message. Drives the "Needs
    /// Review" badge counter; either resolution path clears it,
    /// the OR of the two rules is what keeps the legacy "add a
    /// note" shortcut working alongside the new explicit ack.
    pub unaddressed_down: i64,
    /// True iff the conversation has activity the teaching team
    /// hasn't seen yet: either no `conversation_reviews` row
    /// exists, or the latest user message arrived after the
    /// stored `reviewed_at`. Re-derived per query rather than
    /// cached on the conversation row so a new student turn
    /// automatically flips this back on without explicit
    /// invalidation. Read by the teacher dashboard's
    /// "Unreviewed" filter.
    pub teacher_unreviewed: bool,
    /// Most-recent teaching-team review timestamp, or NULL if no
    /// teacher has opened this conversation since the migration
    /// backfill ran. Surfaced for the "Reviewed by X · 2d ago"
    /// caption in the dashboard.
    pub last_reviewed_at: Option<chrono::DateTime<chrono::Utc>>,
    /// User who performed the most-recent review; pseudonymised
    /// at the response layer for ext: viewers.
    pub last_reviewed_by: Option<Uuid>,
    pub last_reviewer_display_name: Option<String>,
}

pub async fn list_all_by_course(
    db: &PgPool,
    course_id: Uuid,
) -> Result<Vec<ConversationWithUserRow>, sqlx::Error> {
    sqlx::query_as!(
        ConversationWithUserRow,
        r#"SELECT c.id, c.course_id, c.user_id, c.title, c.pinned, c.created_at, c.updated_at,
            u.eppn AS "user_eppn?", u.display_name AS user_display_name,
            (SELECT COUNT(*) FROM messages m WHERE m.conversation_id = c.id) AS message_count
        FROM conversations c
        JOIN users u ON u.id = c.user_id
        WHERE c.course_id = $1
        ORDER BY c.updated_at DESC"#,
        course_id,
    )
    .fetch_all(db)
    .await
}

pub async fn list_all_by_course_with_feedback(
    db: &PgPool,
    course_id: Uuid,
) -> Result<Vec<ConversationWithFeedbackRow>, sqlx::Error> {
    // `teacher_unreviewed` joins the per-conversation review marker
    // against the latest user-message timestamp. Two truth cases:
    //   1. No review row at all (LEFT JOIN miss) → true.
    //   2. Latest student turn timestamp > reviewed_at → true.
    // The implicit "MAX(NULL)" path (a conversation with zero user
    // messages, which shouldn't happen but is defensive) reads as
    // FALSE because `> NULL` is unknown -> false.
    //
    // `unaddressed_down` is the OR of the two clearing rules:
    // either the feedback row has been explicitly acknowledged, OR
    // a teacher note is attached to the same message. Keeps the
    // legacy "add a correction note" shortcut working alongside
    // the new explicit ack button.
    sqlx::query_as!(
        ConversationWithFeedbackRow,
        r#"SELECT c.id, c.course_id, c.user_id, c.title, c.pinned, c.created_at, c.updated_at,
            u.eppn AS "user_eppn?", u.display_name AS user_display_name,
            (SELECT COUNT(*) FROM messages m WHERE m.conversation_id = c.id) AS message_count,
            COALESCE((
                SELECT COUNT(*) FROM message_feedback f
                JOIN messages m ON m.id = f.message_id
                WHERE m.conversation_id = c.id AND f.rating = 'up'
            ), 0) AS "feedback_up!: i64",
            COALESCE((
                SELECT COUNT(*) FROM message_feedback f
                JOIN messages m ON m.id = f.message_id
                WHERE m.conversation_id = c.id AND f.rating = 'down'
            ), 0) AS "feedback_down!: i64",
            COALESCE((
                SELECT COUNT(*) FROM message_feedback f
                JOIN messages m ON m.id = f.message_id
                WHERE m.conversation_id = c.id AND f.rating = 'down'
                  AND f.acknowledged_at IS NULL
                  AND NOT EXISTS (
                      SELECT 1 FROM teacher_notes tn WHERE tn.message_id = f.message_id
                  )
            ), 0) AS "unaddressed_down!: i64",
            (
                cr.reviewed_at IS NULL
                OR cr.reviewed_at < COALESCE(
                    (SELECT MAX(m.created_at) FROM messages m
                       WHERE m.conversation_id = c.id AND m.role = 'user'),
                    cr.reviewed_at
                )
            ) AS "teacher_unreviewed!: bool",
            cr.reviewed_at AS "last_reviewed_at?",
            cr.reviewed_by AS "last_reviewed_by?",
            ru.display_name AS last_reviewer_display_name
        FROM conversations c
        JOIN users u ON u.id = c.user_id
        LEFT JOIN conversation_reviews cr ON cr.conversation_id = c.id
        LEFT JOIN users ru ON ru.id = cr.reviewed_by
        WHERE c.course_id = $1
        ORDER BY c.updated_at DESC"#,
        course_id,
    )
    .fetch_all(db)
    .await
}

pub async fn list_pinned_by_course(
    db: &PgPool,
    course_id: Uuid,
) -> Result<Vec<ConversationWithUserRow>, sqlx::Error> {
    sqlx::query_as!(
        ConversationWithUserRow,
        r#"SELECT c.id, c.course_id, c.user_id, c.title, c.pinned, c.created_at, c.updated_at,
            u.eppn AS "user_eppn?", u.display_name AS user_display_name,
            (SELECT COUNT(*) FROM messages m WHERE m.conversation_id = c.id) AS message_count
        FROM conversations c
        JOIN users u ON u.id = c.user_id
        WHERE c.course_id = $1 AND c.pinned = true
        ORDER BY c.updated_at DESC"#,
        course_id,
    )
    .fetch_all(db)
    .await
}

pub async fn set_pinned(
    db: &PgPool,
    id: Uuid,
    pinned: bool,
) -> Result<Option<ConversationRow>, sqlx::Error> {
    sqlx::query_as!(
        ConversationRow,
        "UPDATE conversations SET pinned = $2, updated_at = NOW() WHERE id = $1 RETURNING id, course_id, user_id, title, pinned, created_at, updated_at",
        id,
        pinned,
    )
    .fetch_optional(db)
    .await
}

// Teacher notes

pub async fn create_note(
    db: &PgPool,
    id: Uuid,
    conversation_id: Uuid,
    message_id: Option<Uuid>,
    author_id: Uuid,
    content: &str,
) -> Result<TeacherNoteRow, sqlx::Error> {
    sqlx::query_as!(
        TeacherNoteRow,
        r#"WITH inserted AS (
            INSERT INTO teacher_notes (id, conversation_id, message_id, author_id, content)
            VALUES ($1, $2, $3, $4, $5)
            RETURNING id, conversation_id, message_id, author_id, content, created_at, updated_at
        )
        SELECT i.id AS "id!", i.conversation_id AS "conversation_id!", i.message_id, i.author_id AS "author_id!", i.content AS "content!", i.created_at AS "created_at!", i.updated_at AS "updated_at!", u.display_name AS author_display_name
        FROM inserted i
        JOIN users u ON u.id = i.author_id"#,
        id,
        conversation_id,
        message_id,
        author_id,
        content,
    )
    .fetch_one(db)
    .await
}

pub async fn list_notes(
    db: &PgPool,
    conversation_id: Uuid,
) -> Result<Vec<TeacherNoteRow>, sqlx::Error> {
    sqlx::query_as!(
        TeacherNoteRow,
        r#"SELECT tn.id, tn.conversation_id, tn.message_id, tn.author_id, tn.content,
            tn.created_at, tn.updated_at, u.display_name AS author_display_name
        FROM teacher_notes tn
        JOIN users u ON u.id = tn.author_id
        WHERE tn.conversation_id = $1
        ORDER BY tn.created_at ASC"#,
        conversation_id,
    )
    .fetch_all(db)
    .await
}

pub async fn update_note(
    db: &PgPool,
    id: Uuid,
    content: &str,
) -> Result<Option<TeacherNoteRow>, sqlx::Error> {
    sqlx::query_as!(
        TeacherNoteRow,
        r#"WITH updated AS (
            UPDATE teacher_notes SET content = $2, updated_at = NOW() WHERE id = $1
            RETURNING id, conversation_id, message_id, author_id, content, created_at, updated_at
        )
        SELECT u2.id AS "id!", u2.conversation_id AS "conversation_id!", u2.message_id, u2.author_id AS "author_id!", u2.content AS "content!", u2.created_at AS "created_at!", u2.updated_at AS "updated_at!", users.display_name AS author_display_name
        FROM updated u2
        JOIN users ON users.id = u2.author_id"#,
        id,
        content,
    )
    .fetch_optional(db)
    .await
}

pub async fn delete_note(db: &PgPool, id: Uuid) -> Result<bool, sqlx::Error> {
    let result = sqlx::query!("DELETE FROM teacher_notes WHERE id = $1", id)
        .execute(db)
        .await?;
    Ok(result.rows_affected() > 0)
}

pub async fn find_note_by_id(db: &PgPool, id: Uuid) -> Result<Option<TeacherNoteRow>, sqlx::Error> {
    sqlx::query_as!(
        TeacherNoteRow,
        r#"SELECT tn.id, tn.conversation_id, tn.message_id, tn.author_id, tn.content,
            tn.created_at, tn.updated_at, u.display_name AS author_display_name
        FROM teacher_notes tn
        JOIN users u ON u.id = tn.author_id
        WHERE tn.id = $1"#,
        id,
    )
    .fetch_optional(db)
    .await
}

pub async fn list_messages(
    db: &PgPool,
    conversation_id: Uuid,
) -> Result<Vec<MessageRow>, sqlx::Error> {
    sqlx::query_as!(
        MessageRow,
        "SELECT id, conversation_id, role, content, chunks_used, model_used, tokens_prompt, tokens_completion, generation_ms, retrieval_count, thinking_transcript, tool_events, thinking_ms, research_prompt_tokens, research_completion_tokens, created_at FROM messages WHERE conversation_id = $1 ORDER BY created_at ASC",
        conversation_id,
    )
    .fetch_all(db)
    .await
}

#[allow(clippy::too_many_arguments)]
pub async fn insert_message(
    db: &PgPool,
    id: Uuid,
    conversation_id: Uuid,
    role: &str,
    content: &str,
    chunks_used: Option<&serde_json::Value>,
    model_used: Option<&str>,
    tokens_prompt: Option<i32>,
    tokens_completion: Option<i32>,
    generation_ms: Option<i32>,
    retrieval_count: Option<i32>,
    thinking_transcript: Option<&str>,
    tool_events: Option<&serde_json::Value>,
    thinking_ms: Option<i32>,
    research_prompt_tokens: Option<i32>,
    research_completion_tokens: Option<i32>,
) -> Result<MessageRow, sqlx::Error> {
    // Also update conversation timestamp
    let _ = sqlx::query!(
        "UPDATE conversations SET updated_at = NOW() WHERE id = $1",
        conversation_id,
    )
    .execute(db)
    .await;

    sqlx::query_as!(
        MessageRow,
        r#"INSERT INTO messages (id, conversation_id, role, content, chunks_used, model_used, tokens_prompt, tokens_completion, generation_ms, retrieval_count, thinking_transcript, tool_events, thinking_ms, research_prompt_tokens, research_completion_tokens)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15)
        RETURNING id, conversation_id, role, content, chunks_used, model_used, tokens_prompt, tokens_completion, generation_ms, retrieval_count, thinking_transcript, tool_events, thinking_ms, research_prompt_tokens, research_completion_tokens, created_at"#,
        id,
        conversation_id,
        role,
        content,
        chunks_used,
        model_used,
        tokens_prompt,
        tokens_completion,
        generation_ms,
        retrieval_count,
        thinking_transcript,
        tool_events,
        thinking_ms,
        research_prompt_tokens,
        research_completion_tokens,
    )
    .fetch_one(db)
    .await
}

#[derive(Debug)]
pub struct ConversationMessageTextRow {
    pub conversation_id: Uuid,
    pub content: String,
}

/// Fetch all user messages for a course (for topic analysis).
pub async fn list_user_messages_by_course(
    db: &PgPool,
    course_id: Uuid,
) -> Result<Vec<ConversationMessageTextRow>, sqlx::Error> {
    sqlx::query_as!(
        ConversationMessageTextRow,
        r#"SELECT m.conversation_id, m.content
        FROM messages m
        JOIN conversations c ON c.id = m.conversation_id
        WHERE c.course_id = $1 AND m.role = 'user'
        ORDER BY m.created_at ASC"#,
        course_id,
    )
    .fetch_all(db)
    .await
}

pub async fn update_title(db: &PgPool, id: Uuid, title: &str) -> Result<(), sqlx::Error> {
    sqlx::query!(
        "UPDATE conversations SET title = $1, updated_at = NOW() WHERE id = $2",
        title,
        id,
    )
    .execute(db)
    .await?;
    Ok(())
}

// ── Unread / reviewed markers ────────────────────────────────────────

/// Stamp `student_last_viewed_at = NOW()` for the conversation owner.
/// Idempotent; fired on every chat-page open. The conversation list
/// query compares this against `MAX(teacher_notes.created_at)` to
/// decide whether to render an unread dot on the row.
///
/// Does NOT verify the caller IS the owner; the route layer enforces
/// that (otherwise a teacher opening a student's chat from the
/// dashboard would silently clear the student's own unread state).
pub async fn mark_student_viewed(db: &PgPool, conversation_id: Uuid) -> Result<(), sqlx::Error> {
    sqlx::query!(
        "UPDATE conversations SET student_last_viewed_at = NOW() WHERE id = $1",
        conversation_id,
    )
    .execute(db)
    .await?;
    Ok(())
}

/// Upsert the per-conversation review marker. Course-shared (any
/// teacher / TA / owner / admin clears it for the whole team);
/// `read == reviewed` per the product call, so this fires on every
/// teacher dashboard expand.
///
/// Route layer is responsible for verifying `reviewer_id` is in
/// fact a teacher / TA / owner / admin on the course; bypassing
/// that check here would let a student mark a conversation
/// "reviewed by teaching team" by hitting the wrong endpoint.
pub async fn mark_teacher_reviewed(
    db: &PgPool,
    conversation_id: Uuid,
    reviewer_id: Uuid,
) -> Result<(), sqlx::Error> {
    sqlx::query!(
        r#"INSERT INTO conversation_reviews (conversation_id, reviewed_at, reviewed_by)
        VALUES ($1, NOW(), $2)
        ON CONFLICT (conversation_id) DO UPDATE
            SET reviewed_at = NOW(),
                reviewed_by = EXCLUDED.reviewed_by"#,
        conversation_id,
        reviewer_id,
    )
    .execute(db)
    .await?;
    Ok(())
}

#[derive(Debug)]
pub struct StudentUnreadRow {
    pub conversation_id: Uuid,
    pub course_id: Uuid,
}

/// All conversations owned by `user_id` that have a teacher_note
/// created after the student's `student_last_viewed_at`. Used to
/// (a) flag rows in the chat sidebar's conversation list, and (b)
/// roll up per-course unread counts for the "My Courses" tile.
///
/// "Never visited" (student_last_viewed_at IS NULL) counts as
/// unread iff a note exists at all; the migration backfilled
/// the column to NOW() so existing day-one rows don't fire
/// spuriously, but new conversations created post-migration
/// start out NULL and we want a fresh teacher note on them to
/// still surface a dot.
pub async fn student_unread_conversations(
    db: &PgPool,
    user_id: Uuid,
) -> Result<Vec<StudentUnreadRow>, sqlx::Error> {
    sqlx::query_as!(
        StudentUnreadRow,
        r#"SELECT c.id AS conversation_id, c.course_id
        FROM conversations c
        WHERE c.user_id = $1
          AND EXISTS (
              SELECT 1 FROM teacher_notes tn
              WHERE tn.conversation_id = c.id
                AND (
                    c.student_last_viewed_at IS NULL
                    OR tn.created_at > c.student_last_viewed_at
                )
          )"#,
        user_id,
    )
    .fetch_all(db)
    .await
}
