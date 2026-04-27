//! Admin-managed allowlist of embedding model ids the teacher dropdown
//! is allowed to surface. See migration
//! `20260427000001_embedding_models.sql` for the schema and seed.
//!
//! The compile-time `VALID_LOCAL_MODELS` slice is the catalog of models
//! the runtime *can* load (code, dimensions, Qdrant collection sizing
//! all depend on it). This table is the *policy* layer on top: only
//! `enabled = TRUE` rows show up in the picker. Disabling a model never
//! touches existing courses already on it; the admin still has to
//! force-migrate per course via `rotate_embedding`.

use sqlx::PgPool;

#[derive(Debug, Clone)]
pub struct EmbeddingModelRow {
    pub model: String,
    pub enabled: bool,
    /// True for the single row that new courses should default to.
    /// Exactly zero or one row in the table carries this; the
    /// invariant is enforced by a partial unique index. Set via
    /// `set_default` (which atomically clears the previous default).
    /// See migration `20260427000002_embedding_model_is_default.sql`.
    pub is_default: bool,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

/// Every row in the table, ordered by model id for stable UI rendering.
/// Cheap (the whole table is at most ~20 rows).
pub async fn list_all(db: &PgPool) -> Result<Vec<EmbeddingModelRow>, sqlx::Error> {
    sqlx::query_as!(
        EmbeddingModelRow,
        r#"SELECT model, enabled, is_default, created_at, updated_at
           FROM embedding_models
           ORDER BY model ASC"#,
    )
    .fetch_all(db)
    .await
}

/// Look up one row. Returns `Ok(None)` if the model id isn't registered
/// (admin route maps that to a 404).
pub async fn find(db: &PgPool, model: &str) -> Result<Option<EmbeddingModelRow>, sqlx::Error> {
    sqlx::query_as!(
        EmbeddingModelRow,
        r#"SELECT model, enabled, is_default, created_at, updated_at
           FROM embedding_models
           WHERE model = $1"#,
        model,
    )
    .fetch_optional(db)
    .await
}

/// Cheap scalar lookup used by the course PUT validator. Returns
/// `Ok(false)` for a model that isn't even registered (treats unknown
/// as disabled; the route layer separately rejects with the
/// `local_embedding_model_invalid` code so the admin/teacher message
/// is still useful).
pub async fn is_enabled(db: &PgPool, model: &str) -> Result<bool, sqlx::Error> {
    let row: Option<bool> = sqlx::query_scalar!(
        "SELECT enabled FROM embedding_models WHERE model = $1",
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
) -> Result<Option<EmbeddingModelRow>, sqlx::Error> {
    sqlx::query_as!(
        EmbeddingModelRow,
        r#"UPDATE embedding_models
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
///
/// Cheap enough that the course-create path can call it on every POST;
/// the partial index on `is_default` makes this an index-only fetch.
pub async fn current_default(db: &PgPool) -> Result<Option<String>, sqlx::Error> {
    sqlx::query_scalar!("SELECT model FROM embedding_models WHERE is_default = TRUE LIMIT 1",)
        .fetch_optional(db)
        .await
}

/// Atomically promote one model to the default and demote the previous
/// holder. Both writes happen in a single transaction so the partial
/// unique index never sees two `TRUE` rows mid-flip.
///
/// The target must already exist in the table and must be
/// `enabled = TRUE`; a disabled default is a contradiction (the
/// picker would refuse to surface it). Returns the updated row, or a
/// typed error so the admin route can map "missing" -> 404 and
/// "disabled" -> 400 without parsing SQL strings.
pub async fn set_default(db: &PgPool, model: &str) -> Result<EmbeddingModelRow, SetDefaultError> {
    let mut tx = db.begin().await.map_err(SetDefaultError::Db)?;

    // Lock the target row inside the transaction so a concurrent
    // `set_enabled(false)` between the check and the UPDATE can't slip
    // through.
    let target = sqlx::query!(
        "SELECT enabled FROM embedding_models WHERE model = $1 FOR UPDATE",
        model,
    )
    .fetch_optional(&mut *tx)
    .await
    .map_err(SetDefaultError::Db)?
    .ok_or(SetDefaultError::NotFound)?;
    if !target.enabled {
        return Err(SetDefaultError::Disabled);
    }

    // Clear any existing default first; the partial unique index would
    // otherwise reject the promotion below.
    sqlx::query!(
        "UPDATE embedding_models SET is_default = FALSE, updated_at = NOW() WHERE is_default = TRUE",
    )
    .execute(&mut *tx)
    .await
    .map_err(SetDefaultError::Db)?;

    let row = sqlx::query_as!(
        EmbeddingModelRow,
        r#"UPDATE embedding_models
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
            SetDefaultError::NotFound => write!(f, "embedding model not in catalog"),
            SetDefaultError::Disabled => {
                write!(f, "embedding model is disabled and cannot be the default")
            }
            SetDefaultError::Db(e) => write!(f, "{}", e),
        }
    }
}

impl std::error::Error for SetDefaultError {}

/// Idempotent insert used by the runtime sync at startup. Newly-added
/// `VALID_LOCAL_MODELS` entries land here with `enabled = $2` (the
/// caller's "default policy" choice) only on first sight; subsequent
/// boots leave the row untouched so an admin's runtime toggle survives
/// restarts. Returns `true` if a row was inserted.
pub async fn seed_if_missing(
    db: &PgPool,
    model: &str,
    initial_enabled: bool,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query!(
        r#"INSERT INTO embedding_models (model, enabled)
           VALUES ($1, $2)
           ON CONFLICT (model) DO NOTHING"#,
        model,
        initial_enabled,
    )
    .execute(db)
    .await?;
    Ok(result.rows_affected() > 0)
}
