//! Queue of (course, eppn) pairs the Daisy import phase wants to
//! enrol but couldn't because the target user hasn't logged in yet.
//!
//! Drained by `auth_middleware` on every successful authentication:
//! for the inbound eppn (and any of its aliases), we add each pending
//! row to `course_members` and delete the pending row. The first
//! drained row flagged `eligible_for_owner` may also be promoted to
//! `courses.owner_id` if the course currently sits on the env-var
//! fallback (see auth_middleware for the exact rule).

use sqlx::PgPool;
use uuid::Uuid;

#[derive(Debug)]
pub struct PendingRow {
    pub id: Uuid,
    pub course_id: Uuid,
    pub eppn: String,
    pub display_name: Option<String>,
    pub role: String,
    pub eligible_for_owner: bool,
    pub daisy_roles: Vec<String>,
    pub daisy_momenttillf_id: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

/// Input bag for `upsert`. Kept as a struct so clippy doesn't
/// complain about the 7+ field arg list (course_id, eppn, name,
/// role, eligible_for_owner, daisy_roles, daisy_momenttillf_id).
pub struct PendingUpsert<'a> {
    pub course_id: Uuid,
    pub eppn: &'a str,
    pub display_name: Option<&'a str>,
    pub role: &'a str,
    pub eligible_for_owner: bool,
    pub daisy_roles: &'a [String],
    pub daisy_momenttillf_id: Option<&'a str>,
}

/// Upsert a pending row. Subsequent calls for the same (course, eppn)
/// refresh `display_name`, `role`, `eligible_for_owner`, and
/// `daisy_roles`; the OR-aggregation on `eligible_for_owner` is
/// deliberate (once a person is identified as course-responsible on
/// this course, a later sync that doesn't see them in that role
/// shouldn't retroactively demote them in the pending queue).
pub async fn upsert(db: &PgPool, input: &PendingUpsert<'_>) -> Result<PendingRow, sqlx::Error> {
    sqlx::query_as!(
        PendingRow,
        r#"INSERT INTO pending_course_memberships
            (course_id, eppn, display_name, role, eligible_for_owner, daisy_roles, daisy_momenttillf_id)
        VALUES ($1, $2, $3, $4, $5, $6, $7)
        ON CONFLICT (course_id, eppn) DO UPDATE SET
            display_name = COALESCE(EXCLUDED.display_name, pending_course_memberships.display_name),
            role = EXCLUDED.role,
            eligible_for_owner = pending_course_memberships.eligible_for_owner OR EXCLUDED.eligible_for_owner,
            daisy_roles = EXCLUDED.daisy_roles,
            daisy_momenttillf_id = COALESCE(EXCLUDED.daisy_momenttillf_id, pending_course_memberships.daisy_momenttillf_id),
            updated_at = NOW()
        RETURNING id, course_id, eppn, display_name, role, eligible_for_owner, daisy_roles, daisy_momenttillf_id, created_at, updated_at"#,
        input.course_id,
        input.eppn,
        input.display_name,
        input.role,
        input.eligible_for_owner,
        input.daisy_roles,
        input.daisy_momenttillf_id,
    )
    .fetch_one(db)
    .await
}

/// All pending rows whose eppn matches the given list. Used by the
/// auth-middleware drain: caller passes [primary_eppn, alias_eppn_1,
/// alias_eppn_2, ...] so a login via an alias still picks up rows
/// originally queued against the primary (or vice versa).
pub async fn list_for_eppns(db: &PgPool, eppns: &[String]) -> Result<Vec<PendingRow>, sqlx::Error> {
    if eppns.is_empty() {
        return Ok(Vec::new());
    }
    sqlx::query_as!(
        PendingRow,
        r#"SELECT id, course_id, eppn, display_name, role, eligible_for_owner, daisy_roles, daisy_momenttillf_id, created_at, updated_at
        FROM pending_course_memberships
        WHERE eppn = ANY($1)
        ORDER BY created_at ASC"#,
        eppns,
    )
    .fetch_all(db)
    .await
}

/// All pending rows on a given course. Powers the admin "pending
/// invites" view per course.
pub async fn list_by_course(db: &PgPool, course_id: Uuid) -> Result<Vec<PendingRow>, sqlx::Error> {
    sqlx::query_as!(
        PendingRow,
        r#"SELECT id, course_id, eppn, display_name, role, eligible_for_owner, daisy_roles, daisy_momenttillf_id, created_at, updated_at
        FROM pending_course_memberships
        WHERE course_id = $1
        ORDER BY created_at ASC"#,
        course_id,
    )
    .fetch_all(db)
    .await
}

/// Delete by id (after a successful drain). Returns the deleted row so
/// the caller can attribute the consumed pending to the actual member
/// row that replaced it (audit / tracing).
pub async fn delete(db: &PgPool, id: Uuid) -> Result<Option<PendingRow>, sqlx::Error> {
    sqlx::query_as!(
        PendingRow,
        r#"DELETE FROM pending_course_memberships WHERE id = $1
        RETURNING id, course_id, eppn, display_name, role, eligible_for_owner, daisy_roles, daisy_momenttillf_id, created_at, updated_at"#,
        id,
    )
    .fetch_optional(db)
    .await
}
