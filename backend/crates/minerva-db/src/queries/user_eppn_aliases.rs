//! Secondary eppns for a single Minerva user.
//!
//! Daisy staff profiles expose every login a person has held (system
//! migrations, name changes, multiple roles all leave their trail). We
//! track them here so an inbound auth header matching any historical
//! login still resolves to the right Minerva user; the most recently
//! observed login wins promotion to primary `users.eppn`.
//!
//! Promotion is handled by `swap_primary_with_alias` (called from the
//! auth middleware when an alias matches the inbound eppn). The
//! `register` helper is called by the Daisy import phase to record
//! every staff username it sees; it's a no-op when the eppn is already
//! the primary for some user or already an alias of one.

use sqlx::PgPool;
use uuid::Uuid;

#[derive(Debug)]
pub struct AliasRow {
    pub id: Uuid,
    pub user_id: Uuid,
    pub eppn: String,
    pub last_seen_at: chrono::DateTime<chrono::Utc>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// Look up the owning user of a non-primary alias eppn. Returns `None`
/// if no alias matches (caller falls through to "create new user").
pub async fn find_user_by_alias_eppn(db: &PgPool, eppn: &str) -> Result<Option<Uuid>, sqlx::Error> {
    let row = sqlx::query!(
        "SELECT user_id FROM user_eppn_aliases WHERE eppn = $1",
        eppn,
    )
    .fetch_optional(db)
    .await?;
    Ok(row.map(|r| r.user_id))
}

/// All aliases attached to a given user, newest-first by `last_seen_at`.
pub async fn list_for_user(db: &PgPool, user_id: Uuid) -> Result<Vec<AliasRow>, sqlx::Error> {
    sqlx::query_as!(
        AliasRow,
        r#"SELECT id, user_id, eppn, last_seen_at, created_at
        FROM user_eppn_aliases
        WHERE user_id = $1
        ORDER BY last_seen_at DESC"#,
        user_id,
    )
    .fetch_all(db)
    .await
}

/// Bump `last_seen_at` on an alias. Called when a login arrives with the
/// alias eppn (after swap), or when the Daisy import phase re-confirms
/// a staff person still holds that username.
pub async fn touch(db: &PgPool, eppn: &str) -> Result<(), sqlx::Error> {
    sqlx::query!(
        "UPDATE user_eppn_aliases SET last_seen_at = NOW() WHERE eppn = $1",
        eppn,
    )
    .execute(db)
    .await?;
    Ok(())
}

/// Register a secondary login for a user. No-op if the eppn is already
/// the user's primary or already registered (by them or someone else).
/// The caller is responsible for verifying ownership before invoking;
/// the Daisy import path does that via Daisy's per-person staff page,
/// which is an authoritative join of (Daisy person_id, usernames).
pub async fn register(db: &PgPool, user_id: Uuid, eppn: &str) -> Result<bool, sqlx::Error> {
    // Skip when this eppn already lives on the user's primary row.
    let primary = sqlx::query!(
        "SELECT 1 AS exists FROM users WHERE id = $1 AND eppn = $2",
        user_id,
        eppn,
    )
    .fetch_optional(db)
    .await?;
    if primary.is_some() {
        // Bump last_seen on the primary side via users.updated_at is
        // wrong (it'd thrash on every request); we just leave the
        // primary alone and rely on auth_middleware's existing
        // updated_at touch.
        return Ok(false);
    }

    let result = sqlx::query!(
        r#"INSERT INTO user_eppn_aliases (user_id, eppn)
        VALUES ($1, $2)
        ON CONFLICT (eppn) DO UPDATE SET last_seen_at = NOW()
        RETURNING xmax = 0 AS "inserted!""#,
        user_id,
        eppn,
    )
    .fetch_one(db)
    .await?;

    Ok(result.inserted)
}

/// Swap the primary `users.eppn` with one of its aliases.
///
/// Used when an inbound auth header matches an alias instead of the
/// primary: we promote the matched alias to primary and demote the
/// previous primary into the aliases table. This is wrapped in a
/// transaction so the per-row UNIQUE constraint on `users.eppn` and
/// `user_eppn_aliases.eppn` can never both be violated mid-swap.
///
/// Steps inside the txn:
///   1. Read the current primary eppn off `users`.
///   2. Delete the alias row for `new_primary_eppn` (it's about to
///      become the primary; we can't leave it pointing at itself).
///   3. UPDATE users.eppn = new_primary_eppn.
///   4. INSERT (user_id, old_primary_eppn) into aliases with
///      `last_seen_at = NOW()` so it carries forward its visit history.
pub async fn swap_primary_with_alias(
    db: &PgPool,
    user_id: Uuid,
    new_primary_eppn: &str,
) -> Result<(), sqlx::Error> {
    let mut tx = db.begin().await?;

    let old_primary = sqlx::query!("SELECT eppn FROM users WHERE id = $1", user_id)
        .fetch_one(&mut *tx)
        .await?
        .eppn;

    if old_primary == new_primary_eppn {
        tx.rollback().await?;
        return Ok(());
    }

    sqlx::query!(
        "DELETE FROM user_eppn_aliases WHERE user_id = $1 AND eppn = $2",
        user_id,
        new_primary_eppn,
    )
    .execute(&mut *tx)
    .await?;

    sqlx::query!(
        "UPDATE users SET eppn = $1, updated_at = NOW() WHERE id = $2",
        new_primary_eppn,
        user_id,
    )
    .execute(&mut *tx)
    .await?;

    sqlx::query!(
        r#"INSERT INTO user_eppn_aliases (user_id, eppn, last_seen_at)
        VALUES ($1, $2, NOW())
        ON CONFLICT (eppn) DO UPDATE SET
            user_id = EXCLUDED.user_id,
            last_seen_at = NOW()"#,
        user_id,
        old_primary,
    )
    .execute(&mut *tx)
    .await?;

    tx.commit().await
}
