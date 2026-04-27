-- Admin-managed default embedding model for new courses.
--
-- The `courses` column has had a SQL-level DEFAULT
-- ('sentence-transformers/all-MiniLM-L6-v2', set in
-- 20260401000001_default_local_embedding.sql) since the local-embedding
-- launch. That default is fine in steady state but it's hard-coded:
-- swapping the recommended starting model meant editing a migration
-- and shipping a release.
--
-- This migration moves the choice into `embedding_models` so admins
-- can flip it from the UI:
--   * `is_default` is the new authoritative flag. Exactly one row may
--     have `is_default = TRUE`; enforced by a partial unique index
--     since postgres doesn't support `CHECK (...)` against an aggregate
--     count.
--   * The course-create path reads this flag (with the SQL DEFAULT as
--     a backstop for unforeseen "no default set" states) instead of
--     relying solely on the column DEFAULT.
--   * Picking a new default in the admin UI is two writes in one
--     transaction: clear `is_default` everywhere, then set it on the
--     chosen row. Models that aren't enabled can't be set as default.
--
-- The seed picks `all-MiniLM-L6-v2` as the initial default because
-- that's what the SQL DEFAULT has been pointing at since launch; no
-- behavioural change for existing prod, just a lift-and-shift of the
-- choice into a row that admins can rewrite.

ALTER TABLE embedding_models
    ADD COLUMN is_default BOOLEAN NOT NULL DEFAULT FALSE;

-- Only one row may carry the flag. Partial unique index so the index
-- doesn't try to enforce uniqueness across every `FALSE` row (which
-- would block any second non-default model).
CREATE UNIQUE INDEX embedding_models_single_default
    ON embedding_models ((is_default))
    WHERE is_default = TRUE;

-- Lift the prod default into the table. Idempotent: if the row was
-- somehow disabled earlier it's promoted back to enabled here too,
-- because a disabled default is a contradiction (the picker would
-- reject it).
UPDATE embedding_models
   SET is_default = TRUE,
       enabled    = TRUE,
       updated_at = NOW()
 WHERE model = 'sentence-transformers/all-MiniLM-L6-v2';
