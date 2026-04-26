-- Embedding-rotation version. Bumped each time a course's
-- embedding_provider or embedding_model changes, so the runtime can
-- pick a fresh Qdrant collection (`course_{id}_v{n}` for n >= 2)
-- without colliding with vectors produced under the previous model.
--
-- Existing collections were created with the legacy name
-- `course_{id}` (no suffix); the helper in `minerva-ingest::pipeline`
-- treats version=1 as that legacy name so this migration needs no
-- Qdrant data move.
ALTER TABLE courses
    ADD COLUMN embedding_version INTEGER NOT NULL DEFAULT 1;
