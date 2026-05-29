-- Admin-managed catalog of cross-encoder re-ranker models.
--
-- Mirrors `embedding_models`: the compile-time `VALID_RERANKER_MODELS`
-- slice (crates/minerva-ingest/src/reranker.rs) is the set of models the
-- runtime knows how to load; this table is the policy layer on top. Only
-- `enabled = TRUE` rows show up in the teacher's per-course picker, and
-- the single `is_default = TRUE` row is what new courses are created
-- with.
--
-- Unlike embeddings, switching a course's re-ranker has NO re-embed side
-- effect: the cross-encoder reads chunk text, not vectors, so the
-- per-course change on PUT /courses/{id} applies instantly (no Qdrant
-- collection rotation, no document re-queue).
--
-- Startup behaviour: AppState::new upserts every VALID_RERANKER_MODELS
-- entry missing from this table with enabled = FALSE, leaving existing
-- rows untouched. So this migration's seed is the *initial* policy;
-- subsequent admin toggles persist through restarts.

CREATE TABLE reranker_models (
    model       TEXT PRIMARY KEY,
    enabled     BOOLEAN NOT NULL DEFAULT FALSE,
    is_default  BOOLEAN NOT NULL DEFAULT FALSE,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Only one row may carry the default flag. Partial unique index because
-- postgres has no CHECK against an aggregate count, and a plain unique
-- index on is_default would block any second non-default row.
CREATE UNIQUE INDEX reranker_models_single_default
    ON reranker_models ((is_default))
    WHERE is_default = TRUE;

-- Seed the multilingual default (Swedish + English), enabled + default.
-- The heavier multilingual model (rozgo/bge-reranker-v2-m3) and the
-- English-only models come in via the AppState startup sync defaulting
-- to disabled, so loading any of them on the prod pod is a deliberate
-- admin opt-in (each is a multi-hundred-MB model load).
INSERT INTO reranker_models (model, enabled, is_default) VALUES
    ('jinaai/jina-reranker-v2-base-multilingual', TRUE, TRUE)
ON CONFLICT (model) DO NOTHING;
