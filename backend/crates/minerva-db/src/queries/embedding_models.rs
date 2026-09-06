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

crate::queries::model_catalog::model_catalog_queries!(
    row: EmbeddingModelRow,
    list_all: r#"SELECT model, enabled, is_default, created_at, updated_at
           FROM embedding_models
           ORDER BY model ASC"#,
    find: r#"SELECT model, enabled, is_default, created_at, updated_at
           FROM embedding_models
           WHERE model = $1"#,
    is_enabled: "SELECT enabled FROM embedding_models WHERE model = $1",
    set_enabled: r#"UPDATE embedding_models
           SET enabled = $2, updated_at = NOW()
           WHERE model = $1
           RETURNING model, enabled, is_default, created_at, updated_at"#,
    current_default: "SELECT model FROM embedding_models WHERE is_default = TRUE LIMIT 1",
    lock_target: "SELECT enabled FROM embedding_models WHERE model = $1 FOR UPDATE",
    clear_default: "UPDATE embedding_models SET is_default = FALSE, updated_at = NOW() WHERE is_default = TRUE",
    promote_default: r#"UPDATE embedding_models
           SET is_default = TRUE, updated_at = NOW()
           WHERE model = $1
           RETURNING model, enabled, is_default, created_at, updated_at"#,
    seed_if_missing: r#"INSERT INTO embedding_models (model, enabled)
           VALUES ($1, $2)
           ON CONFLICT (model) DO NOTHING"#,
    not_found_msg: "embedding model not in catalog",
    disabled_msg: "embedding model is disabled and cannot be the default",
);
