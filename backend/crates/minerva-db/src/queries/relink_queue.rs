//! Persistent dirty queue for the cross-doc linker.
//!
//! Backs the [`crate::classification::linker`] sweeper so a server
//! restart doesn't silently drop pending relinks. Same shape as the
//! old in-memory `HashMap<Uuid, Instant>` but the entries live in
//! `relink_queue` and the in-memory layer becomes a thin cache.
//!
//! Concurrency: the table has a primary key on `course_id`, so two
//! parallel `mark_dirty` calls collapse into one row via
//! `ON CONFLICT`. The `take_due` path uses
//! `DELETE … RETURNING course_id` so the sweeper can drain the queue
//! atomically without races between read and delete.

use sqlx::PgPool;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct RelinkEntry {
    pub course_id: Uuid,
    pub first_marked_at: chrono::DateTime<chrono::Utc>,
    pub due_at: chrono::DateTime<chrono::Utc>,
}

/// Mark a course dirty. The first call inserts a new row with
/// `first_marked_at = NOW()` and `due_at = NOW() + debounce`. Subsequent
/// calls push `due_at` forward but cap it at
/// `first_marked_at + max_pending_age` so a sustained burst can't
/// indefinitely defer the linker.
pub async fn mark_dirty(
    db: &PgPool,
    course_id: Uuid,
    debounce_seconds: i64,
    max_pending_age_seconds: i64,
) -> Result<RelinkEntry, sqlx::Error> {
    sqlx::query_as!(
        RelinkEntry,
        r#"
        INSERT INTO relink_queue (course_id, first_marked_at, due_at)
        VALUES (
            $1,
            NOW(),
            NOW() + make_interval(secs => $2)
        )
        ON CONFLICT (course_id) DO UPDATE
            SET due_at = LEAST(
                NOW() + make_interval(secs => $2),
                relink_queue.first_marked_at + make_interval(secs => $3)
            )
        RETURNING course_id, first_marked_at, due_at
        "#,
        course_id,
        debounce_seconds as f64,
        max_pending_age_seconds as f64,
    )
    .fetch_one(db)
    .await
}

/// Mark a course dirty for immediate processing (next sweep tick). Used
/// after teacher-driven kind changes / explicit "rebuild" / backfill
/// completion where waiting the debounce window would feel laggy.
pub async fn mark_dirty_immediate(
    db: &PgPool,
    course_id: Uuid,
) -> Result<RelinkEntry, sqlx::Error> {
    sqlx::query_as!(
        RelinkEntry,
        r#"
        INSERT INTO relink_queue (course_id, first_marked_at, due_at)
        VALUES ($1, NOW(), NOW())
        ON CONFLICT (course_id) DO UPDATE
            SET due_at = LEAST(relink_queue.due_at, NOW())
        RETURNING course_id, first_marked_at, due_at
        "#,
        course_id,
    )
    .fetch_one(db)
    .await
}

/// Atomically drain courses whose `due_at` has passed. Returns the
/// drained list and removes them from the table; if the linker fails
/// the caller is responsible for re-marking via `mark_dirty_immediate`.
pub async fn take_due(db: &PgPool) -> Result<Vec<RelinkEntry>, sqlx::Error> {
    sqlx::query_as!(
        RelinkEntry,
        r#"
        DELETE FROM relink_queue
        WHERE due_at <= NOW()
        RETURNING course_id, first_marked_at, due_at
        "#,
    )
    .fetch_all(db)
    .await
}

/// Number of courses currently waiting. Surface for telemetry / debug.
pub async fn pending_count(db: &PgPool) -> Result<i64, sqlx::Error> {
    let row = sqlx::query_scalar!(r#"SELECT COUNT(*) AS "count!" FROM relink_queue"#)
        .fetch_one(db)
        .await?;
    Ok(row)
}

/// Is this course currently queued for a relink? Surfaced to the
/// graph-viewer endpoint so the UI can show a "Linking..." indicator
/// while the sweep catches up. A row is present iff `mark_dirty` (or
/// `mark_dirty_immediate`) has fired for the course since the last
/// successful drain by `take_due`.
pub async fn is_queued(db: &PgPool, course_id: Uuid) -> Result<bool, sqlx::Error> {
    let row = sqlx::query_scalar!(
        r#"SELECT EXISTS (
               SELECT 1 FROM relink_queue WHERE course_id = $1
           ) AS "exists!""#,
        course_id,
    )
    .fetch_one(db)
    .await?;
    Ok(row)
}
