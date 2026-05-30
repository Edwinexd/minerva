//! Admin-managed allowlist of cross-encoder re-ranker model ids the
//! teacher dropdown is allowed to surface. See migration
//! `20260529000001_reranker_models.sql` for the schema and seed.
//!
//! The compile-time `VALID_RERANKER_MODELS` slice
//! (`minerva_embed_engine::reranker`) is the catalog of models the runtime
//! *can* load. This table is the *policy* layer on top: only
//! `enabled = TRUE` rows show up in the per-course picker. Disabling a
//! model never touches existing courses already on it; the admin can
//! force-migrate a course onto any catalog model via `PUT /courses/{id}`
//! (no re-embed: the re-ranker reads chunk text, not vectors).

use sqlx::PgPool;

#[derive(Debug, Clone)]
pub struct RerankerModelRow {
    pub model: String,
    pub enabled: bool,
    /// True for the single row that new courses should default to.
    /// Exactly zero or one row in the table carries this; the invariant
    /// is enforced by a partial unique index. Set via `set_default`
    /// (which atomically clears the previous default).
    pub is_default: bool,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

/// Every row in the table, ordered by model id for stable UI rendering.
/// Cheap (the whole table is at most a handful of rows).
pub async fn list_all(db: &PgPool) -> Result<Vec<RerankerModelRow>, sqlx::Error> {
    sqlx::query_as!(
        RerankerModelRow,
        r#"SELECT model, enabled, is_default, created_at, updated_at
           FROM reranker_models
           ORDER BY model ASC"#,
    )
    .fetch_all(db)
    .await
}

/// Look up one row. Returns `Ok(None)` if the model id isn't registered
/// (admin route maps that to a 404).
pub async fn find(db: &PgPool, model: &str) -> Result<Option<RerankerModelRow>, sqlx::Error> {
    sqlx::query_as!(
        RerankerModelRow,
        r#"SELECT model, enabled, is_default, created_at, updated_at
           FROM reranker_models
           WHERE model = $1"#,
        model,
    )
    .fetch_optional(db)
    .await
}

/// Cheap scalar lookup used by the course PUT validator. Returns
/// `Ok(false)` for a model that isn't even registered (treats unknown as
/// disabled; the route layer separately rejects with a clearer code).
pub async fn is_enabled(db: &PgPool, model: &str) -> Result<bool, sqlx::Error> {
    let row: Option<bool> = sqlx::query_scalar!(
        "SELECT enabled FROM reranker_models WHERE model = $1",
        model,
    )
    .fetch_optional(db)
    .await?;
    Ok(row.unwrap_or(false))
}

/// Toggle the `enabled` flag for an existing row. Returns `Ok(None)` if
/// no row matches the model id, so the admin route can 404 properly.
pub async fn set_enabled(
    db: &PgPool,
    model: &str,
    enabled: bool,
) -> Result<Option<RerankerModelRow>, sqlx::Error> {
    sqlx::query_as!(
        RerankerModelRow,
        r#"UPDATE reranker_models
           SET enabled = $2, updated_at = NOW()
           WHERE model = $1
           RETURNING model, enabled, is_default, created_at, updated_at"#,
        model,
        enabled,
    )
    .fetch_optional(db)
    .await
}

/// Read the model id new courses should default to. Returns `Ok(None)`
/// if no row has `is_default = TRUE` (shouldn't happen post-migration,
/// but the route layer falls back to the column DEFAULT in that case).
pub async fn current_default(db: &PgPool) -> Result<Option<String>, sqlx::Error> {
    sqlx::query_scalar!("SELECT model FROM reranker_models WHERE is_default = TRUE LIMIT 1",)
        .fetch_optional(db)
        .await
}

/// Atomically promote one model to the default and demote the previous
/// holder. Both writes happen in a single transaction so the partial
/// unique index never sees two `TRUE` rows mid-flip.
///
/// The target must already exist and must be `enabled = TRUE`; a
/// disabled default is a contradiction (the picker would refuse to
/// surface it). Returns a typed error so the admin route can map
/// "missing" -> 404 and "disabled" -> 400 without parsing SQL strings.
pub async fn set_default(db: &PgPool, model: &str) -> Result<RerankerModelRow, SetDefaultError> {
    let mut tx = db.begin().await.map_err(SetDefaultError::Db)?;

    // Lock the target row inside the transaction so a concurrent
    // `set_enabled(false)` between the check and the UPDATE can't slip
    // through.
    let target = sqlx::query!(
        "SELECT enabled FROM reranker_models WHERE model = $1 FOR UPDATE",
        model,
    )
    .fetch_optional(&mut *tx)
    .await
    .map_err(SetDefaultError::Db)?
    .ok_or(SetDefaultError::NotFound)?;
    if !target.enabled {
        return Err(SetDefaultError::Disabled);
    }

    sqlx::query!(
        "UPDATE reranker_models SET is_default = FALSE, updated_at = NOW() WHERE is_default = TRUE",
    )
    .execute(&mut *tx)
    .await
    .map_err(SetDefaultError::Db)?;

    let row = sqlx::query_as!(
        RerankerModelRow,
        r#"UPDATE reranker_models
           SET is_default = TRUE, updated_at = NOW()
           WHERE model = $1
           RETURNING model, enabled, is_default, created_at, updated_at"#,
        model,
    )
    .fetch_one(&mut *tx)
    .await
    .map_err(SetDefaultError::Db)?;

    tx.commit().await.map_err(SetDefaultError::Db)?;
    Ok(row)
}

#[derive(Debug)]
pub enum SetDefaultError {
    NotFound,
    Disabled,
    Db(sqlx::Error),
}

impl std::fmt::Display for SetDefaultError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SetDefaultError::NotFound => write!(f, "reranker model not in catalog"),
            SetDefaultError::Disabled => {
                write!(f, "reranker model is disabled and cannot be the default")
            }
            SetDefaultError::Db(e) => write!(f, "{}", e),
        }
    }
}

impl std::error::Error for SetDefaultError {}

/// Idempotent insert used by the runtime sync at startup. Newly-added
/// `VALID_RERANKER_MODELS` entries land here with `enabled = $2` only on
/// first sight; subsequent boots leave the row untouched so an admin's
/// runtime toggle survives restarts. Returns `true` if a row was
/// inserted.
pub async fn seed_if_missing(
    db: &PgPool,
    model: &str,
    initial_enabled: bool,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query!(
        r#"INSERT INTO reranker_models (model, enabled)
           VALUES ($1, $2)
           ON CONFLICT (model) DO NOTHING"#,
        model,
        initial_enabled,
    )
    .execute(db)
    .await?;
    Ok(result.rows_affected() > 0)
}
