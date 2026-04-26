-- Admin-managed enabled list of local embedding models.
--
-- The compile-time `VALID_LOCAL_MODELS` slice is the catalog of models
-- the runtime knows how to load (i.e. "code exists for these"). This
-- table is a *policy* layer on top: only enabled rows show up in the
-- teacher's per-course config dropdown. Disabling a model removes it
-- from the picker but does NOT touch existing courses on it -- those
-- keep working until an admin explicitly force-migrates them via the
-- existing `rotate_embedding` path on `PUT /courses/{id}`.
--
-- Why a separate table rather than `feature_flags`: model ids contain
-- characters (slashes, dots) and a per-flag global row can't carry
-- per-row metadata like `dimensions` for the future. Keeping it
-- isolated also means a flag-system rewrite doesn't accidentally
-- enable/disable a model.
--
-- Startup behaviour: `AppState::new` upserts every `VALID_LOCAL_MODELS`
-- entry that's missing from this table, leaving the existing rows'
-- `enabled` value untouched. So this migration's seed is the *initial*
-- policy; subsequent admin toggles persist through restarts.
--
-- Initial policy:
--   * The four models that have been in production since launch
--     (all-MiniLM-L6-v2, bge-small-en-v1.5, bge-base-en-v1.5,
--     nomic-embed-text-v1.5) are enabled. Disabling them now would
--     break the picker for courses already using them on save.
--   * Newly-added models (multilingual-e5-*, bge-m3, embeddinggemma,
--     mxbai, gte-large, arctic-l, Qwen3) are NOT seeded here. They
--     come in via the runtime sync in `AppState::new` defaulting to
--     `enabled = FALSE` so an admin has to opt in deliberately --
--     loading any of them on the prod pod is a multi-hundred-MB
--     event we don't want to start by accident.

CREATE TABLE embedding_models (
    model       TEXT PRIMARY KEY,
    enabled     BOOLEAN NOT NULL DEFAULT FALSE,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

INSERT INTO embedding_models (model, enabled) VALUES
    ('sentence-transformers/all-MiniLM-L6-v2', TRUE),
    ('BAAI/bge-small-en-v1.5',                 TRUE),
    ('BAAI/bge-base-en-v1.5',                  TRUE),
    ('nomic-ai/nomic-embed-text-v1.5',         TRUE)
ON CONFLICT (model) DO NOTHING;
