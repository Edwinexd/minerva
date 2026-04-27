//! Admin-managed feature flags. See migration
//! `20260426000003_feature_flags.sql` for the schema and resolution
//! semantics. Two layers in this module:
//!
//!   1. `set` / `delete` / `list_*`; raw CRUD that the admin routes
//!      surface to the operator.
//!   2. `is_enabled_for_course` / `is_enabled_for_user`; application
//!      helpers that resolve a flag against the documented order
//!      (course/user > global > compiled-in default).
//!
//! Compiled-in defaults are not stored in the DB; callers pass them.
//! Most opt-in flags should default to FALSE so the table being empty
//! is a safe baseline.

use sqlx::PgPool;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct FeatureFlagRow {
    pub id: Uuid,
    pub flag: String,
    pub course_id: Option<Uuid>,
    pub user_id: Option<Uuid>,
    pub enabled: bool,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

/// Scope a flag write/read targets. Constructed by callers; the DB
/// layer doesn't expose the (course_id, user_id) rawness directly so
/// it's easy to enforce the "at most one of these is set" invariant.
#[derive(Debug, Clone, Copy)]
pub enum Scope {
    Global,
    Course(Uuid),
    User(Uuid),
}

impl Scope {
    fn as_pair(self) -> (Option<Uuid>, Option<Uuid>) {
        match self {
            Scope::Global => (None, None),
            Scope::Course(id) => (Some(id), None),
            Scope::User(id) => (None, Some(id)),
        }
    }
}

/// Insert or update a flag at the given scope. Returns the resulting
/// row. Idempotent under the appropriate partial unique index.
pub async fn set(
    db: &PgPool,
    flag: &str,
    scope: Scope,
    enabled: bool,
) -> Result<FeatureFlagRow, sqlx::Error> {
    let (course_id, user_id) = scope.as_pair();
    // We can't use a single `ON CONFLICT (flag, course_id, user_id)`
    // because the unique constraint is split across three partial
    // indexes (one per scope shape). Branch in Rust instead; this
    // is plenty fast for an admin-rate operation and keeps the SQL
    // honest.
    match scope {
        Scope::Course(_) => {
            sqlx::query_as!(
                FeatureFlagRow,
                r#"INSERT INTO feature_flags (flag, course_id, user_id, enabled)
               VALUES ($1, $2, $3, $4)
               ON CONFLICT (flag, course_id) WHERE course_id IS NOT NULL
               DO UPDATE SET enabled = EXCLUDED.enabled, updated_at = NOW()
               RETURNING id, flag, course_id, user_id, enabled, created_at, updated_at"#,
                flag,
                course_id,
                user_id,
                enabled,
            )
            .fetch_one(db)
            .await
        }
        Scope::User(_) => {
            sqlx::query_as!(
                FeatureFlagRow,
                r#"INSERT INTO feature_flags (flag, course_id, user_id, enabled)
               VALUES ($1, $2, $3, $4)
               ON CONFLICT (flag, user_id) WHERE user_id IS NOT NULL
               DO UPDATE SET enabled = EXCLUDED.enabled, updated_at = NOW()
               RETURNING id, flag, course_id, user_id, enabled, created_at, updated_at"#,
                flag,
                course_id,
                user_id,
                enabled,
            )
            .fetch_one(db)
            .await
        }
        Scope::Global => {
            sqlx::query_as!(
                FeatureFlagRow,
                r#"INSERT INTO feature_flags (flag, course_id, user_id, enabled)
               VALUES ($1, NULL, NULL, $2)
               ON CONFLICT (flag) WHERE course_id IS NULL AND user_id IS NULL
               DO UPDATE SET enabled = EXCLUDED.enabled, updated_at = NOW()
               RETURNING id, flag, course_id, user_id, enabled, created_at, updated_at"#,
                flag,
                enabled,
            )
            .fetch_one(db)
            .await
        }
    }
}

/// Remove a flag row at the given scope. Returns true iff a row was
/// actually deleted. Use to revert a course back to the global default
/// rather than leaving an explicit `enabled = false` row in place.
pub async fn delete(db: &PgPool, flag: &str, scope: Scope) -> Result<bool, sqlx::Error> {
    let result = match scope {
        Scope::Course(course_id) => {
            sqlx::query!(
                "DELETE FROM feature_flags WHERE flag = $1 AND course_id = $2",
                flag,
                course_id
            )
            .execute(db)
            .await?
        }
        Scope::User(user_id) => {
            sqlx::query!(
                "DELETE FROM feature_flags WHERE flag = $1 AND user_id = $2",
                flag,
                user_id
            )
            .execute(db)
            .await?
        }
        Scope::Global => {
            sqlx::query!(
            "DELETE FROM feature_flags WHERE flag = $1 AND course_id IS NULL AND user_id IS NULL",
            flag,
        )
            .execute(db)
            .await?
        }
    };
    Ok(result.rows_affected() > 0)
}

/// All rows in the table. Used by the admin overview to show every
/// override across every scope at a glance.
#[allow(dead_code)]
pub async fn list_all(db: &PgPool) -> Result<Vec<FeatureFlagRow>, sqlx::Error> {
    sqlx::query_as!(
        FeatureFlagRow,
        "SELECT id, flag, course_id, user_id, enabled, created_at, updated_at FROM feature_flags ORDER BY flag, created_at"
    )
    .fetch_all(db)
    .await
}

/// Course-scoped rows for one course. Used to populate the per-course
/// admin panel.
pub async fn list_for_course(
    db: &PgPool,
    course_id: Uuid,
) -> Result<Vec<FeatureFlagRow>, sqlx::Error> {
    sqlx::query_as!(
        FeatureFlagRow,
        "SELECT id, flag, course_id, user_id, enabled, created_at, updated_at FROM feature_flags WHERE course_id = $1",
        course_id,
    )
    .fetch_all(db)
    .await
}

/// Resolve a flag for a course: course-scoped row wins, then global,
/// then `default`. Three queries in the worst case but in practice
/// one (course row exists) or two (no course row, fall through to
/// global). Cheap enough that callers don't need a cache for now.
pub async fn is_enabled_for_course(
    db: &PgPool,
    flag: &str,
    course_id: Uuid,
    default: bool,
) -> Result<bool, sqlx::Error> {
    // 1. course-scoped row?
    let course_row: Option<bool> = sqlx::query_scalar!(
        "SELECT enabled FROM feature_flags WHERE flag = $1 AND course_id = $2",
        flag,
        course_id,
    )
    .fetch_optional(db)
    .await?;
    if let Some(enabled) = course_row {
        return Ok(enabled);
    }
    // 2. global row?
    let global_row: Option<bool> = sqlx::query_scalar!(
        "SELECT enabled FROM feature_flags WHERE flag = $1 AND course_id IS NULL AND user_id IS NULL",
        flag,
    )
    .fetch_optional(db)
    .await?;
    Ok(global_row.unwrap_or(default))
}

/// Resolve a flag for a user. Same shape as `is_enabled_for_course`.
#[allow(dead_code)]
pub async fn is_enabled_for_user(
    db: &PgPool,
    flag: &str,
    user_id: Uuid,
    default: bool,
) -> Result<bool, sqlx::Error> {
    let user_row: Option<bool> = sqlx::query_scalar!(
        "SELECT enabled FROM feature_flags WHERE flag = $1 AND user_id = $2",
        flag,
        user_id,
    )
    .fetch_optional(db)
    .await?;
    if let Some(enabled) = user_row {
        return Ok(enabled);
    }
    let global_row: Option<bool> = sqlx::query_scalar!(
        "SELECT enabled FROM feature_flags WHERE flag = $1 AND course_id IS NULL AND user_id IS NULL",
        flag,
    )
    .fetch_optional(db)
    .await?;
    Ok(global_row.unwrap_or(default))
}
