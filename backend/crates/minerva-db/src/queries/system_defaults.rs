//! Key/value store for admin-tunable system-wide defaults. See
//! migration `20260527000003_system_defaults.sql` for the schema and
//! `backend/crates/minerva-server/src/system_defaults.rs` for the
//! registry of supported keys, their types, and seed sources.
//!
//! This module is intentionally type-agnostic: callers pass a
//! `serde::Deserialize` target type and the helpers ferry JSONB rows
//! in and out via `serde_json`. The registry layer owns the per-key
//! type discipline; we own the storage discipline (atomic upsert,
//! `updated_at` bump, cheap full-table reads).

use serde::{de::DeserializeOwned, Serialize};
use sqlx::PgPool;

/// One row in `system_defaults`. The route layer pairs this with
/// per-key registry metadata (label, type, min/max, env-var fallback
/// name) before returning to the admin UI; the metadata is the same
/// for every deployment and so doesn't live in the DB.
#[derive(Debug, Clone)]
pub struct SystemDefaultRow {
    pub key: String,
    pub value: serde_json::Value,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

/// Every row in the table, sorted by key for stable UI rendering.
/// Cheap (the table has ~20 rows at most for the foreseeable future).
pub async fn list_all(db: &PgPool) -> Result<Vec<SystemDefaultRow>, sqlx::Error> {
    sqlx::query_as!(
        SystemDefaultRow,
        r#"SELECT key, value, updated_at
           FROM system_defaults
           ORDER BY key ASC"#,
    )
    .fetch_all(db)
    .await
}

/// Read one key and decode it into `T`. Returns `Ok(None)` when the
/// row is missing (so callers can fall back to their hard-coded
/// default) or when JSON decoding into `T` fails (logged at warn,
/// treated as "no value set" so the caller's fallback applies). The
/// decode-failure path is deliberately non-fatal: a typo in an admin
/// edit shouldn't bring down a hot path; logging + fallback is the
/// safer contract.
pub async fn get<T: DeserializeOwned>(db: &PgPool, key: &str) -> Result<Option<T>, sqlx::Error> {
    let row: Option<serde_json::Value> =
        sqlx::query_scalar!("SELECT value FROM system_defaults WHERE key = $1", key)
            .fetch_optional(db)
            .await?;
    match row {
        None => Ok(None),
        Some(v) => match serde_json::from_value::<T>(v.clone()) {
            Ok(t) => Ok(Some(t)),
            Err(e) => {
                tracing::warn!(
                    "system_defaults: failed to decode `{}` as {}: {}; value={}",
                    key,
                    std::any::type_name::<T>(),
                    e,
                    v,
                );
                Ok(None)
            }
        },
    }
}

/// Upsert one key. Always bumps `updated_at`. Serialization of `T`
/// failing is an internal error (caller's type doesn't round-trip
/// through JSON); we surface it via `sqlx::Error::Encode` to share
/// the same error type as the storage layer ; no extra `Result`
/// shape for callers to thread.
pub async fn set<T: Serialize>(
    db: &PgPool,
    key: &str,
    value: &T,
) -> Result<SystemDefaultRow, sqlx::Error> {
    let json = serde_json::to_value(value).map_err(|e| sqlx::Error::Encode(Box::new(e)))?;
    sqlx::query_as!(
        SystemDefaultRow,
        r#"INSERT INTO system_defaults (key, value)
           VALUES ($1, $2)
           ON CONFLICT (key) DO UPDATE
             SET value = EXCLUDED.value, updated_at = NOW()
           RETURNING key, value, updated_at"#,
        key,
        json,
    )
    .fetch_one(db)
    .await
}

/// Insert-or-leave-alone, used by the startup seeder. Returns `true`
/// if the row was inserted (i.e. first boot for this key), `false` if
/// the row already existed and we left the admin's edit untouched.
pub async fn seed_if_missing<T: Serialize>(
    db: &PgPool,
    key: &str,
    value: &T,
) -> Result<bool, sqlx::Error> {
    let json = serde_json::to_value(value).map_err(|e| sqlx::Error::Encode(Box::new(e)))?;
    let result = sqlx::query!(
        r#"INSERT INTO system_defaults (key, value)
           VALUES ($1, $2)
           ON CONFLICT (key) DO NOTHING"#,
        key,
        json,
    )
    .execute(db)
    .await?;
    Ok(result.rows_affected() > 0)
}

/// Delete one key. Used by the admin "reset to fallback" button: the
/// next read falls back to env-var/hard-coded; the next startup seed
/// reseeds the row. Returns `true` if a row was actually removed.
pub async fn delete(db: &PgPool, key: &str) -> Result<bool, sqlx::Error> {
    let result = sqlx::query!("DELETE FROM system_defaults WHERE key = $1", key)
        .execute(db)
        .await?;
    Ok(result.rows_affected() > 0)
}
