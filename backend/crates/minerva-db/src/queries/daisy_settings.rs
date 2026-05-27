//! Singleton settings row for the Daisy auto-import.
//!
//! Pinned to `id = 1` by a CHECK constraint in migration
//! `20260527000004`. Holds the `auto_apply` toggle the admin flips
//! once they trust the staging workflow.

use sqlx::PgPool;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct DaisySettingsRow {
    pub auto_apply: bool,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    pub updated_by: Option<Uuid>,
}

/// Read the current setting. Always returns a row because the
/// migration seeds id=1 unconditionally; a missing row is a
/// referential-integrity bug rather than a normal state.
pub async fn get(db: &PgPool) -> Result<DaisySettingsRow, sqlx::Error> {
    sqlx::query_as!(
        DaisySettingsRow,
        r#"SELECT auto_apply, updated_at, updated_by
        FROM daisy_settings
        WHERE id = 1"#,
    )
    .fetch_one(db)
    .await
}

/// Cheap helper for the hot path. Avoids carrying the
/// updated_at/updated_by columns that the service endpoint doesn't
/// care about; one less round-trip's worth of bytes per import.
pub async fn auto_apply_enabled(db: &PgPool) -> Result<bool, sqlx::Error> {
    let row = sqlx::query!("SELECT auto_apply FROM daisy_settings WHERE id = 1")
        .fetch_one(db)
        .await?;
    Ok(row.auto_apply)
}

/// Flip the toggle. `updated_by` is the admin who clicked; nullable
/// so a future automated migration helper could touch this without
/// pretending to be a real user.
pub async fn set_auto_apply(
    db: &PgPool,
    enabled: bool,
    updated_by: Option<Uuid>,
) -> Result<DaisySettingsRow, sqlx::Error> {
    sqlx::query_as!(
        DaisySettingsRow,
        r#"UPDATE daisy_settings
           SET auto_apply = $1, updated_at = NOW(), updated_by = $2
           WHERE id = 1
           RETURNING auto_apply, updated_at, updated_by"#,
        enabled,
        updated_by,
    )
    .fetch_one(db)
    .await
}
