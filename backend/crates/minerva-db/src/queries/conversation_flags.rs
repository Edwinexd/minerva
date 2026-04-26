//! Per-conversation flag log + KG state helpers.
//!
//! Two things live here:
//!
//! 1. `conversation_flags` (CRUD) -- append-only record of judgements
//!    the chat path made about a conversation (currently just
//!    `extraction_attempt`, but the schema is generic). The teacher
//!    dashboard reads this to render badges + per-turn breakdowns.
//!
//! 2. `kg_state` accessors on the `conversations` table -- a JSONB
//!    blob the chat path mutates per turn (sliding window of which
//!    assignments have been near, whether the extraction guard's
//!    hard constraint is currently active, etc.). Kept here rather
//!    than in `conversations.rs` because every reader of kg_state
//!    is also a flag reader/writer; pulling them together keeps
//!    related concerns next to each other.
//!
//! See `20260426000006_conversation_flags.sql` for the schema and
//! the documented JSON shape of `kg_state`.

use sqlx::PgPool;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct ConversationFlagRow {
    pub id: Uuid,
    pub conversation_id: Uuid,
    pub flag: String,
    pub turn_index: Option<i32>,
    pub rationale: Option<String>,
    pub metadata: Option<serde_json::Value>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// Append a flag to a conversation. Always inserts a new row -- we
/// keep history rather than upsert, so the dashboard can show the
/// trail of judgements over time. Idempotency, when needed, is the
/// caller's responsibility (e.g. don't fire on every duplicate
/// retry of the same turn).
pub async fn insert(
    db: &PgPool,
    conversation_id: Uuid,
    flag: &str,
    turn_index: Option<i32>,
    rationale: Option<&str>,
    metadata: Option<&serde_json::Value>,
) -> Result<ConversationFlagRow, sqlx::Error> {
    sqlx::query_as!(
        ConversationFlagRow,
        r#"INSERT INTO conversation_flags
            (conversation_id, flag, turn_index, rationale, metadata)
        VALUES ($1, $2, $3, $4, $5)
        RETURNING id, conversation_id, flag, turn_index, rationale, metadata, created_at"#,
        conversation_id,
        flag,
        turn_index,
        rationale,
        metadata,
    )
    .fetch_one(db)
    .await
}

/// Every flag attached to a conversation, oldest first. The dashboard
/// shows these on the conversation detail page; the per-turn UI uses
/// `turn_index` to align flags to messages.
pub async fn list_for_conversation(
    db: &PgPool,
    conversation_id: Uuid,
) -> Result<Vec<ConversationFlagRow>, sqlx::Error> {
    sqlx::query_as!(
        ConversationFlagRow,
        "SELECT id, conversation_id, flag, turn_index, rationale, metadata, created_at
         FROM conversation_flags
         WHERE conversation_id = $1
         ORDER BY created_at ASC",
        conversation_id,
    )
    .fetch_all(db)
    .await
}

/// Distinct flag-name set for a course's conversations -- powers the
/// "this conversation has been flagged" badge in the conversation
/// list. Returned as a HashMap so the route handler can do O(1)
/// lookups when rendering each list row.
pub async fn flag_kinds_by_conversation(
    db: &PgPool,
    course_id: Uuid,
) -> Result<std::collections::HashMap<Uuid, Vec<String>>, sqlx::Error> {
    let rows = sqlx::query!(
        r#"SELECT DISTINCT cf.conversation_id, cf.flag
           FROM conversation_flags cf
           JOIN conversations c ON c.id = cf.conversation_id
           WHERE c.course_id = $1"#,
        course_id,
    )
    .fetch_all(db)
    .await?;
    let mut out: std::collections::HashMap<Uuid, Vec<String>> = std::collections::HashMap::new();
    for r in rows {
        out.entry(r.conversation_id).or_default().push(r.flag);
    }
    Ok(out)
}

// ── kg_state on conversations ──────────────────────────────────────

/// Read the kg_state blob. Returns an empty Value when the row
/// hasn't been written yet (new conversations have the column's
/// default `'{}'`). Caller deserialises into its own typed shape.
pub async fn get_kg_state(
    db: &PgPool,
    conversation_id: Uuid,
) -> Result<serde_json::Value, sqlx::Error> {
    let row: Option<serde_json::Value> = sqlx::query_scalar!(
        "SELECT kg_state FROM conversations WHERE id = $1",
        conversation_id,
    )
    .fetch_optional(db)
    .await?;
    Ok(row.unwrap_or(serde_json::json!({})))
}

/// Replace the kg_state blob. Caller is responsible for round-tripping
/// (read, mutate, write) -- the chat path holds the conversation
/// effectively single-writer per turn so we don't need optimistic-
/// concurrency machinery here.
pub async fn set_kg_state(
    db: &PgPool,
    conversation_id: Uuid,
    state: &serde_json::Value,
) -> Result<(), sqlx::Error> {
    sqlx::query!(
        "UPDATE conversations SET kg_state = $1 WHERE id = $2",
        state,
        conversation_id,
    )
    .execute(db)
    .await?;
    Ok(())
}
