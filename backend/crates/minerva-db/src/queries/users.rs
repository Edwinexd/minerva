use sqlx::PgPool;
use uuid::Uuid;

#[derive(Debug)]
pub struct UserRow {
    pub id: Uuid,
    pub eppn: String,
    pub display_name: Option<String>,
    pub role: String,
    pub suspended: bool,
    pub role_manually_set: bool,
    pub owner_daily_token_limit: i64,
    pub privacy_acknowledged_at: Option<chrono::DateTime<chrono::Utc>>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

pub async fn find_by_id(db: &PgPool, id: Uuid) -> Result<Option<UserRow>, sqlx::Error> {
    sqlx::query_as!(
        UserRow,
        "SELECT id, eppn, display_name, role, suspended, role_manually_set, owner_daily_token_limit, privacy_acknowledged_at, created_at, updated_at FROM users WHERE id = $1",
        id,
    )
    .fetch_optional(db)
    .await
}

pub async fn find_by_eppn(db: &PgPool, eppn: &str) -> Result<Option<UserRow>, sqlx::Error> {
    sqlx::query_as!(
        UserRow,
        "SELECT id, eppn, display_name, role, suspended, role_manually_set, owner_daily_token_limit, privacy_acknowledged_at, created_at, updated_at FROM users WHERE eppn = $1",
        eppn,
    )
    .fetch_optional(db)
    .await
}

/// Find a user by eppn, or create one with the given defaults if none exists.
/// Returns `(user, created)` where `created` is true iff this call inserted
/// the row. Race-safe via `ON CONFLICT (eppn) DO NOTHING RETURNING`: if a
/// concurrent request wins the insert, we fall through to a follow-up
/// `find_by_eppn`. The owner cap is applied only on insert, never on the
/// follow-up fetch, mirroring `upsert`'s grandfathering semantics.
pub async fn find_or_create_by_eppn(
    db: &PgPool,
    eppn: &str,
    display_name: Option<&str>,
    role: &str,
    default_owner_daily_token_limit: i64,
) -> Result<(UserRow, bool), sqlx::Error> {
    let inserted = sqlx::query_as!(
        UserRow,
        "INSERT INTO users (id, eppn, display_name, role, owner_daily_token_limit)
         VALUES ($1, $2, $3, $4, $5)
         ON CONFLICT (eppn) DO NOTHING
         RETURNING id, eppn, display_name, role, suspended, role_manually_set, owner_daily_token_limit, privacy_acknowledged_at, created_at, updated_at",
        Uuid::new_v4(),
        eppn,
        display_name,
        role,
        default_owner_daily_token_limit,
    )
    .fetch_optional(db)
    .await?;

    if let Some(row) = inserted {
        return Ok((row, true));
    }

    let existing = find_by_eppn(db, eppn)
        .await?
        .ok_or(sqlx::Error::RowNotFound)?;
    Ok((existing, false))
}

/// Upsert called on every authenticated request. The role argument is the
/// caller-computed role (admin allowlist + rule evaluation result). For
/// existing users with `role_manually_set = TRUE` the stored role is
/// preserved -- the admin's manual choice wins over rule-based promotion.
/// `display_name` is always refreshed from the IdP via COALESCE (not gated
/// by the role lock); the lock applies only to `role`. The
/// `default_owner_daily_token_limit` is applied only on INSERT, never on
/// update, so admin overrides via `update_owner_daily_token_limit` are
/// sticky.
pub async fn upsert(
    db: &PgPool,
    id: Uuid,
    eppn: &str,
    display_name: Option<&str>,
    role: &str,
    default_owner_daily_token_limit: i64,
) -> Result<UserRow, sqlx::Error> {
    sqlx::query_as!(
        UserRow,
        "INSERT INTO users (id, eppn, display_name, role, owner_daily_token_limit)
         VALUES ($1, $2, $3, $4, $5)
         ON CONFLICT (eppn) DO UPDATE SET
            display_name = COALESCE($3, users.display_name),
            role = CASE WHEN users.role_manually_set THEN users.role ELSE $4 END,
            updated_at = NOW()
         RETURNING id, eppn, display_name, role, suspended, role_manually_set, owner_daily_token_limit, privacy_acknowledged_at, created_at, updated_at",
        id,
        eppn,
        display_name,
        role,
        default_owner_daily_token_limit,
    )
    .fetch_one(db)
    .await
}

pub async fn list_all(db: &PgPool) -> Result<Vec<UserRow>, sqlx::Error> {
    sqlx::query_as!(
        UserRow,
        "SELECT id, eppn, display_name, role, suspended, role_manually_set, owner_daily_token_limit, privacy_acknowledged_at, created_at, updated_at FROM users ORDER BY eppn",
    )
    .fetch_all(db)
    .await
}

pub async fn set_suspended(
    db: &PgPool,
    user_id: Uuid,
    suspended: bool,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query!(
        "UPDATE users SET suspended = $1, updated_at = NOW() WHERE id = $2",
        suspended,
        user_id,
    )
    .execute(db)
    .await?;
    Ok(result.rows_affected() > 0)
}

/// Admin-driven role change: also locks the role (sets role_manually_set =
/// TRUE) so future rule evaluations leave it alone.
pub async fn update_role(db: &PgPool, user_id: Uuid, role: &str) -> Result<bool, sqlx::Error> {
    let result = sqlx::query!(
        "UPDATE users SET role = $1, role_manually_set = TRUE, updated_at = NOW() WHERE id = $2",
        role,
        user_id,
    )
    .execute(db)
    .await?;
    Ok(result.rows_affected() > 0)
}

/// Removes the manual lock so the next login lets rules re-evaluate.
pub async fn clear_role_lock(db: &PgPool, user_id: Uuid) -> Result<bool, sqlx::Error> {
    let result = sqlx::query!(
        "UPDATE users SET role_manually_set = FALSE, updated_at = NOW() WHERE id = $1",
        user_id,
    )
    .execute(db)
    .await?;
    Ok(result.rows_affected() > 0)
}

/// Records the user's acknowledgment of the in-app data-handling disclosure.
/// Idempotent: later acknowledgments leave the original timestamp in place,
/// so we preserve the first-ever agreement date.
pub async fn acknowledge_privacy(db: &PgPool, user_id: Uuid) -> Result<bool, sqlx::Error> {
    let result = sqlx::query!(
        "UPDATE users SET privacy_acknowledged_at = COALESCE(privacy_acknowledged_at, NOW()), updated_at = NOW() WHERE id = $1",
        user_id,
    )
    .execute(db)
    .await?;
    Ok(result.rows_affected() > 0)
}

pub async fn update_owner_daily_token_limit(
    db: &PgPool,
    user_id: Uuid,
    limit: i64,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query!(
        "UPDATE users SET owner_daily_token_limit = $1, updated_at = NOW() WHERE id = $2",
        limit,
        user_id,
    )
    .execute(db)
    .await?;
    Ok(result.rows_affected() > 0)
}
