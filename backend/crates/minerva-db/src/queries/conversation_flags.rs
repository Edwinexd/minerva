//! Per-conversation flag log + KG state helpers.
//!
//! Two things live here:
//!
//! 1. `conversation_flags` (CRUD); append-only record of judgements
//!    the chat path made about a conversation (currently just
//!    `extraction_attempt`, but the schema is generic). The teacher
//!    dashboard reads this to render badges + per-turn breakdowns.
//!
//! 2. `kg_state` accessors on the `conversations` table; a JSONB
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
    /// NULL until a teacher / TA / owner / admin clicks
    /// "Acknowledge" on the dashboard. Once set, the flag still
    /// renders on the conversation detail page (audit trail) but
    /// stops driving the "Needs Review" badge / tab / list-row
    /// indicator. Replaces the prior "extraction flags are
    /// append-only forever" behaviour.
    pub acknowledged_at: Option<chrono::DateTime<chrono::Utc>>,
    /// User who clicked Acknowledge; surfaced as "acknowledged by
    /// Edwin · 2d ago" in the UI. Pseudonymised at the response
    /// layer for ext: viewers.
    pub acknowledged_by: Option<Uuid>,
    /// Display name of the acknowledger. Populated by list/query
    /// helpers below via LEFT JOIN; None when no ack yet or the
    /// user has no display name set.
    pub acknowledger_display_name: Option<String>,
}

/// Append a flag to a conversation. Always inserts a new row; we
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
        RETURNING id, conversation_id, flag, turn_index, rationale, metadata,
            created_at, acknowledged_at, acknowledged_by,
            NULL::TEXT AS acknowledger_display_name"#,
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
/// `turn_index` to align flags to messages. Acked flags stay in the
/// list (audit trail) with their ack metadata populated.
pub async fn list_for_conversation(
    db: &PgPool,
    conversation_id: Uuid,
) -> Result<Vec<ConversationFlagRow>, sqlx::Error> {
    sqlx::query_as!(
        ConversationFlagRow,
        r#"SELECT cf.id, cf.conversation_id, cf.flag, cf.turn_index,
            cf.rationale, cf.metadata, cf.created_at,
            cf.acknowledged_at, cf.acknowledged_by,
            au.display_name AS acknowledger_display_name
         FROM conversation_flags cf
         LEFT JOIN users au ON au.id = cf.acknowledged_by
         WHERE cf.conversation_id = $1
         ORDER BY cf.created_at ASC"#,
        conversation_id,
    )
    .fetch_all(db)
    .await
}

/// Distinct **unacknowledged** flag-name set for a course's
/// conversations; powers the "this conversation has been flagged"
/// badge + "Needs Review" tab in the conversation list. Acked flags
/// are intentionally filtered here; the dashboard treats them as
/// resolved for triage purposes, even though they're still visible
/// in the conversation's detail panel for audit. Returned as a
/// HashMap so the route handler can do O(1) lookups when rendering
/// each list row.
pub async fn flag_kinds_by_conversation(
    db: &PgPool,
    course_id: Uuid,
) -> Result<std::collections::HashMap<Uuid, Vec<String>>, sqlx::Error> {
    let rows = sqlx::query!(
        r#"SELECT DISTINCT cf.conversation_id, cf.flag
           FROM conversation_flags cf
           JOIN conversations c ON c.id = cf.conversation_id
           WHERE c.course_id = $1 AND cf.acknowledged_at IS NULL"#,
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

/// Look up a single flag by id, used by the route layer to validate
/// the `(course, conversation, flag)` triple matches the URL before
/// acking. Returns None for unknown ids.
pub async fn find_by_id(db: &PgPool, id: Uuid) -> Result<Option<ConversationFlagRow>, sqlx::Error> {
    sqlx::query_as!(
        ConversationFlagRow,
        r#"SELECT cf.id, cf.conversation_id, cf.flag, cf.turn_index,
            cf.rationale, cf.metadata, cf.created_at,
            cf.acknowledged_at, cf.acknowledged_by,
            au.display_name AS acknowledger_display_name
         FROM conversation_flags cf
         LEFT JOIN users au ON au.id = cf.acknowledged_by
         WHERE cf.id = $1"#,
        id,
    )
    .fetch_optional(db)
    .await
}

/// Stamp `acknowledged_at = NOW()` / `acknowledged_by = user_id` on a
/// single flag. Re-acking an already-acked row overwrites the
/// timestamp and attribution (acks are last-writer-wins by design --
/// if a senior teacher re-reviews after a TA acked it, their name is
/// what the dashboard shows).
pub async fn acknowledge(
    db: &PgPool,
    id: Uuid,
    user_id: Uuid,
) -> Result<Option<ConversationFlagRow>, sqlx::Error> {
    sqlx::query_as!(
        ConversationFlagRow,
        r#"WITH updated AS (
            UPDATE conversation_flags
            SET acknowledged_at = NOW(), acknowledged_by = $2
            WHERE id = $1
            RETURNING id, conversation_id, flag, turn_index, rationale,
                metadata, created_at, acknowledged_at, acknowledged_by
        )
        SELECT u2.id AS "id!", u2.conversation_id AS "conversation_id!",
            u2.flag AS "flag!", u2.turn_index, u2.rationale, u2.metadata,
            u2.created_at AS "created_at!", u2.acknowledged_at, u2.acknowledged_by,
            au.display_name AS acknowledger_display_name
        FROM updated u2
        LEFT JOIN users au ON au.id = u2.acknowledged_by"#,
        id,
        user_id,
    )
    .fetch_optional(db)
    .await
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
/// (read, mutate, write); the chat path holds the conversation
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
