use rust_decimal::Decimal;
use sqlx::PgPool;
use uuid::Uuid;

#[derive(Debug, sqlx::FromRow)]
pub struct UserRow {
    pub id: Uuid,
    pub eppn: String,
    pub display_name: Option<String>,
    pub role: String,
    pub suspended: bool,
    pub role_manually_set: bool,
    /// Per-owner daily AI spending cap in USD (summed across owned
    /// courses). 0 = unlimited. Spend is derived on read.
    pub owner_daily_cost_limit_usd: Decimal,
    pub privacy_acknowledged_at: Option<chrono::DateTime<chrono::Utc>>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

pub async fn find_by_id(db: &PgPool, id: Uuid) -> Result<Option<UserRow>, sqlx::Error> {
    sqlx::query_as!(
        UserRow,
        "SELECT id, eppn, display_name, role, suspended, role_manually_set, owner_daily_cost_limit_usd, privacy_acknowledged_at, created_at, updated_at FROM users WHERE id = $1",
        id,
    )
    .fetch_optional(db)
    .await
}

pub async fn find_by_eppn(db: &PgPool, eppn: &str) -> Result<Option<UserRow>, sqlx::Error> {
    sqlx::query_as!(
        UserRow,
        "SELECT id, eppn, display_name, role, suspended, role_manually_set, owner_daily_cost_limit_usd, privacy_acknowledged_at, created_at, updated_at FROM users WHERE eppn = $1",
        eppn,
    )
    .fetch_optional(db)
    .await
}

/// Look up a user by inbound eppn, falling back to the alias table.
///
/// Returns `(user, via_alias)` where `via_alias` is TRUE iff the
/// match came from `user_eppn_aliases` rather than the primary
/// `users.eppn`. Callers (auth middleware) use that flag to
/// invoke `user_eppn_aliases::swap_primary_with_alias`; promoting
/// the most-recently-used eppn to primary keeps the user-facing
/// "current SU login" view in sync with reality without ever losing
/// the old logins.
pub async fn find_by_eppn_or_alias(
    db: &PgPool,
    eppn: &str,
) -> Result<Option<(UserRow, bool)>, sqlx::Error> {
    if let Some(row) = find_by_eppn(db, eppn).await? {
        return Ok(Some((row, false)));
    }
    let Some(user_id) =
        crate::queries::user_eppn_aliases::find_user_by_alias_eppn(db, eppn).await?
    else {
        return Ok(None);
    };
    let row = find_by_id(db, user_id).await?.ok_or_else(|| {
        // An alias pointing at a missing user is a referential-integrity
        // bug; the FK has ON DELETE CASCADE so this should never happen.
        sqlx::Error::RowNotFound
    })?;
    Ok(Some((row, true)))
}

/// Resolve a user by primary or alias eppn, or create one with the given
/// defaults if the identity is entirely unknown.
/// Returns `(user, created)` where `created` is true iff this call inserted
/// the row. The cross-table EPPN registry serializes a concurrent primary or
/// alias reservation; if another request wins, its canonical user is returned.
/// The owner cap is applied only on insert, never on the follow-up fetch,
/// mirroring `upsert`'s grandfathering semantics.
pub async fn find_or_create_by_eppn(
    db: &PgPool,
    eppn: &str,
    display_name: Option<&str>,
    role: &str,
    default_owner_daily_cost_limit_usd: Decimal,
) -> Result<(UserRow, bool), sqlx::Error> {
    // An alias is every bit as much an identity as a primary EPPN.  The old
    // implementation skipped this lookup and could deterministically create
    // a second user for an existing alias.
    if let Some((row, _via_alias)) = find_by_eppn_or_alias(db, eppn).await? {
        return Ok((row, false));
    }

    let inserted = sqlx::query_as!(
        UserRow,
        "INSERT INTO users (id, eppn, display_name, role, owner_daily_cost_limit_usd)
         VALUES ($1, $2, $3, $4, $5)
         ON CONFLICT (eppn) DO NOTHING
         RETURNING id, eppn, display_name, role, suspended, role_manually_set, owner_daily_cost_limit_usd, privacy_acknowledged_at, created_at, updated_at",
        Uuid::new_v4(),
        eppn,
        display_name,
        role,
        default_owner_daily_cost_limit_usd,
    )
    .fetch_optional(db)
    .await;

    let inserted = match inserted {
        Ok(row) => row,
        Err(error) if is_eppn_registry_conflict(&error) => {
            // A primary/alias reservation committed after the lookup above.
            // The registry's unique index made the competing statement wait,
            // so READ COMMITTED sees its owner in this follow-up query.
            let (row, _via_alias) = find_by_eppn_or_alias(db, eppn)
                .await?
                .ok_or(sqlx::Error::RowNotFound)?;
            return Ok((row, false));
        }
        Err(error) => return Err(error),
    };

    if let Some(row) = inserted {
        return Ok((row, true));
    }

    let existing = find_by_eppn_or_alias(db, eppn)
        .await?
        .map(|(row, _via_alias)| row)
        .ok_or(sqlx::Error::RowNotFound)?;
    Ok((existing, false))
}

fn is_eppn_registry_conflict(error: &sqlx::Error) -> bool {
    matches!(
        error,
        sqlx::Error::Database(db_error)
            if db_error.code().as_deref() == Some("23505")
                && db_error.constraint() == Some("user_eppn_registry_pkey")
    )
}

/// Refresh an authenticated user by resolved identity rather than by EPPN.
///
/// Alias promotion may be followed immediately by another login that swaps a
/// different alias to primary.  Updating by stable user id avoids turning that
/// harmless race into another INSERT attempt.  Manual role locks retain their
/// existing precedence.
pub async fn update_authenticated(
    db: &PgPool,
    user_id: Uuid,
    display_name: Option<&str>,
    role: &str,
) -> Result<UserRow, sqlx::Error> {
    sqlx::query_as::<_, UserRow>(
        r#"UPDATE users SET
            display_name = COALESCE($2, display_name),
            role = CASE WHEN role_manually_set THEN role ELSE $3 END,
            updated_at = NOW()
        WHERE id = $1
        RETURNING id, eppn, display_name, role, suspended, role_manually_set,
                  owner_daily_cost_limit_usd, privacy_acknowledged_at,
                  created_at, updated_at"#,
    )
    .bind(user_id)
    .bind(display_name)
    .bind(role)
    .fetch_one(db)
    .await
}

pub async fn list_all(db: &PgPool) -> Result<Vec<UserRow>, sqlx::Error> {
    sqlx::query_as!(
        UserRow,
        "SELECT id, eppn, display_name, role, suspended, role_manually_set, owner_daily_cost_limit_usd, privacy_acknowledged_at, created_at, updated_at FROM users ORDER BY eppn",
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

pub async fn update_owner_daily_cost_limit_usd(
    db: &PgPool,
    user_id: Uuid,
    limit: Decimal,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query!(
        "UPDATE users SET owner_daily_cost_limit_usd = $1, updated_at = NOW() WHERE id = $2",
        limit,
        user_id,
    )
    .execute(db)
    .await?;
    Ok(result.rows_affected() > 0)
}
