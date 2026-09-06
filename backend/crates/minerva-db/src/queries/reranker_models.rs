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

crate::queries::model_catalog::model_catalog_queries!(
    row: RerankerModelRow,
    list_all: r#"SELECT model, enabled, is_default, created_at, updated_at
           FROM reranker_models
           ORDER BY model ASC"#,
    find: r#"SELECT model, enabled, is_default, created_at, updated_at
           FROM reranker_models
           WHERE model = $1"#,
    is_enabled: "SELECT enabled FROM reranker_models WHERE model = $1",
    set_enabled: r#"UPDATE reranker_models
           SET enabled = $2, updated_at = NOW()
           WHERE model = $1
           RETURNING model, enabled, is_default, created_at, updated_at"#,
    current_default: "SELECT model FROM reranker_models WHERE is_default = TRUE LIMIT 1",
    lock_target: "SELECT enabled FROM reranker_models WHERE model = $1 FOR UPDATE",
    clear_default: "UPDATE reranker_models SET is_default = FALSE, updated_at = NOW() WHERE is_default = TRUE",
    promote_default: r#"UPDATE reranker_models
           SET is_default = TRUE, updated_at = NOW()
           WHERE model = $1
           RETURNING model, enabled, is_default, created_at, updated_at"#,
    seed_if_missing: r#"INSERT INTO reranker_models (model, enabled)
           VALUES ($1, $2)
           ON CONFLICT (model) DO NOTHING"#,
    not_found_msg: "reranker model not in catalog",
    disabled_msg: "reranker model is disabled and cannot be the default",
);
