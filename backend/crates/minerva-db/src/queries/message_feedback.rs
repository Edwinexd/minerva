use sqlx::PgPool;
use uuid::Uuid;

#[derive(Debug)]
pub struct MessageFeedbackRow {
    pub id: Uuid,
    pub message_id: Uuid,
    pub user_id: Uuid,
    pub rating: String,
    pub category: Option<String>,
    pub comment: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    /// NULL until a teacher clicks "Mark as reviewed" on the
    /// dashboard. The legacy "leaving a note on the same message
    /// addresses the downvote" rule still applies in parallel --
    /// the `unaddressed_down` aggregation ORs the two clearing
    /// paths so existing flows keep working.
    pub acknowledged_at: Option<chrono::DateTime<chrono::Utc>>,
    pub acknowledged_by: Option<Uuid>,
}

#[derive(Debug)]
pub struct MessageFeedbackWithUserRow {
    pub id: Uuid,
    pub message_id: Uuid,
    pub user_id: Uuid,
    pub rating: String,
    pub category: Option<String>,
    pub comment: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    pub user_eppn: Option<String>,
    pub user_display_name: Option<String>,
    pub acknowledged_at: Option<chrono::DateTime<chrono::Utc>>,
    pub acknowledged_by: Option<Uuid>,
    pub acknowledger_display_name: Option<String>,
}

/// Insert or update the current user's feedback for a single
/// message. Each (message, user) pair has at most one feedback
/// row. Re-upserting a feedback row clears any prior ack (a
/// student changing their thumbs-down → thumbs-up resets the
/// teacher review status, since the signal itself changed); the
/// `ON CONFLICT` branch nulls out the ack columns explicitly so
/// stale acknowledgments don't quietly cling to a row that now
/// means something different.
pub async fn upsert(
    db: &PgPool,
    message_id: Uuid,
    user_id: Uuid,
    rating: &str,
    category: Option<&str>,
    comment: Option<&str>,
) -> Result<MessageFeedbackRow, sqlx::Error> {
    sqlx::query_as!(
        MessageFeedbackRow,
        r#"INSERT INTO message_feedback (id, message_id, user_id, rating, category, comment)
        VALUES ($1, $2, $3, $4, $5, $6)
        ON CONFLICT (message_id, user_id) DO UPDATE
            SET rating = EXCLUDED.rating,
                category = EXCLUDED.category,
                comment = EXCLUDED.comment,
                updated_at = NOW(),
                acknowledged_at = NULL,
                acknowledged_by = NULL
        RETURNING id, message_id, user_id, rating, category, comment,
            created_at, updated_at, acknowledged_at, acknowledged_by"#,
        Uuid::new_v4(),
        message_id,
        user_id,
        rating,
        category,
        comment,
    )
    .fetch_one(db)
    .await
}

#[derive(Debug)]
pub struct FeedbackCategoryCountRow {
    pub category: Option<String>,
    pub count: i64,
}

#[derive(Debug)]
pub struct CourseFeedbackSummaryRow {
    pub total_up: i64,
    pub total_down: i64,
}

/// All feedback rows for messages in a conversation, ordered
/// oldest first. Ack columns are populated when set, and the
/// acknowledger's display name is joined in so the teacher
/// dashboard can render "acknowledged by Edwin · 2d ago"
/// without a second query.
pub async fn list_for_conversation(
    db: &PgPool,
    conversation_id: Uuid,
) -> Result<Vec<MessageFeedbackWithUserRow>, sqlx::Error> {
    sqlx::query_as!(
        MessageFeedbackWithUserRow,
        r#"SELECT f.id, f.message_id, f.user_id, f.rating, f.category, f.comment,
            f.created_at, f.updated_at,
            u.eppn AS "user_eppn?", u.display_name AS user_display_name,
            f.acknowledged_at, f.acknowledged_by,
            au.display_name AS acknowledger_display_name
        FROM message_feedback f
        JOIN messages m ON m.id = f.message_id
        JOIN users u ON u.id = f.user_id
        LEFT JOIN users au ON au.id = f.acknowledged_by
        WHERE m.conversation_id = $1
        ORDER BY f.created_at ASC"#,
        conversation_id,
    )
    .fetch_all(db)
    .await
}

/// Look up a single feedback row by id; route layer uses this to
/// validate the `(course, conversation, feedback)` triple matches
/// the URL before acking.
pub async fn find_by_id(
    db: &PgPool,
    id: Uuid,
) -> Result<Option<MessageFeedbackWithUserRow>, sqlx::Error> {
    sqlx::query_as!(
        MessageFeedbackWithUserRow,
        r#"SELECT f.id, f.message_id, f.user_id, f.rating, f.category, f.comment,
            f.created_at, f.updated_at,
            u.eppn AS "user_eppn?", u.display_name AS user_display_name,
            f.acknowledged_at, f.acknowledged_by,
            au.display_name AS acknowledger_display_name
        FROM message_feedback f
        JOIN users u ON u.id = f.user_id
        LEFT JOIN users au ON au.id = f.acknowledged_by
        WHERE f.id = $1"#,
        id,
    )
    .fetch_optional(db)
    .await
}

/// Stamp `acknowledged_at = NOW()` / `acknowledged_by = user_id` on
/// a single feedback row. Idempotent (re-acking overwrites with the
/// latest reviewer), and orthogonal to the legacy "leaving a note
/// on the same message addresses the downvote" path; both clearing
/// rules are ORed by `unaddressed_down` so either resolves it.
pub async fn acknowledge(
    db: &PgPool,
    id: Uuid,
    user_id: Uuid,
) -> Result<Option<MessageFeedbackWithUserRow>, sqlx::Error> {
    sqlx::query_as!(
        MessageFeedbackWithUserRow,
        r#"WITH updated AS (
            UPDATE message_feedback
            SET acknowledged_at = NOW(), acknowledged_by = $2
            WHERE id = $1
            RETURNING id, message_id, user_id, rating, category, comment,
                created_at, updated_at, acknowledged_at, acknowledged_by
        )
        SELECT u2.id AS "id!", u2.message_id AS "message_id!",
            u2.user_id AS "user_id!", u2.rating AS "rating!", u2.category,
            u2.comment, u2.created_at AS "created_at!", u2.updated_at AS "updated_at!",
            u.eppn AS "user_eppn?", u.display_name AS user_display_name,
            u2.acknowledged_at, u2.acknowledged_by,
            au.display_name AS acknowledger_display_name
        FROM updated u2
        JOIN users u ON u.id = u2.user_id
        LEFT JOIN users au ON au.id = u2.acknowledged_by"#,
        id,
        user_id,
    )
    .fetch_optional(db)
    .await
}

/// Per-category thumbs-down counts for all conversations in a course.
pub async fn category_counts_for_course(
    db: &PgPool,
    course_id: Uuid,
) -> Result<Vec<FeedbackCategoryCountRow>, sqlx::Error> {
    sqlx::query_as!(
        FeedbackCategoryCountRow,
        r#"SELECT f.category, COUNT(*) AS "count!: i64"
        FROM message_feedback f
        JOIN messages m ON m.id = f.message_id
        JOIN conversations c ON c.id = m.conversation_id
        WHERE c.course_id = $1 AND f.rating = 'down'
        GROUP BY f.category
        ORDER BY COUNT(*) DESC"#,
        course_id,
    )
    .fetch_all(db)
    .await
}

/// Total thumbs-up and thumbs-down counts for all conversations in a course.
pub async fn total_ratings_for_course(
    db: &PgPool,
    course_id: Uuid,
) -> Result<CourseFeedbackSummaryRow, sqlx::Error> {
    sqlx::query_as!(
        CourseFeedbackSummaryRow,
        r#"SELECT
            COALESCE(SUM(CASE WHEN f.rating = 'up' THEN 1 ELSE 0 END), 0) AS "total_up!: i64",
            COALESCE(SUM(CASE WHEN f.rating = 'down' THEN 1 ELSE 0 END), 0) AS "total_down!: i64"
        FROM message_feedback f
        JOIN messages m ON m.id = f.message_id
        JOIN conversations c ON c.id = m.conversation_id
        WHERE c.course_id = $1"#,
        course_id,
    )
    .fetch_one(db)
    .await
}
