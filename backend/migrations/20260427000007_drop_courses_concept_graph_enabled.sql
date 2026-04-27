-- Roll back 20260427000006: the concept-graph toggle should live in the
-- existing `feature_flags` table alongside `course_kg`, `extraction_guard`,
-- and `aegis`, not as a dedicated column on `courses`. The previous
-- migration was redundant; admin UI already enumerates `feature_flags`
-- via `feature_flags::ALL_FLAGS`. Adding `FLAG_CONCEPT_GRAPH` there is
-- the single source of truth.
--
-- The dropped column was always FALSE in production (default since
-- 20260427000006 shipped with no UI to flip it), so no data is lost.
ALTER TABLE courses DROP COLUMN IF EXISTS concept_graph_enabled;
