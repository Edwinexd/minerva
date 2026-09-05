-- Bring the topic-switch embedding cache onto the same generation
-- marker, and the same eventual-consistency contract, that document
-- embeddings already use.
--
-- The original cache (migration 20260905000002) keyed cached vectors on
-- `topic_embedding_model`, a model-id string, and treated a mismatch as
-- "ignore this row". That diverged from the established standard in two
-- ways that matter:
--
--   1. Wrong key. `courses.embedding_version` is this codebase's
--      generation marker; `queries::courses::rotate_embedding` bumps it
--      and the runtime composes the Qdrant collection name from it. A
--      parallel model-string key is a second way to express the same
--      idea, and the two can disagree.
--
--   2. No convergence. Documents are re-queued inside the rotation
--      transaction, so a rotated course walks back to fully embedded.
--      Message vectors were never re-embedded at all: after a rotation
--      every existing turn stayed permanently unusable, so detection
--      quality on an in-flight conversation was degraded forever rather
--      than briefly, and each dead row held ~3 KB of vector nobody
--      could ever read again.
--
-- This is the expand half of an expand/contract pair. `topic_embedding_model`
-- is left in place, and its CHECK dropped, so a pod running the
-- previous build can keep writing it through a rolling deploy without
-- violating a constraint it does not know about. The contract half
-- (drop the column, add the paired CHECK on the version) lands in a
-- later migration, once no old pod is serving.

ALTER TABLE messages
    ADD COLUMN topic_embedding_version INTEGER;

-- Old pods write (embedding, model) and no version; new pods write
-- (embedding, version) and no model. Neither shape can satisfy a
-- pairing CHECK written for the other, so the invariant is enforced in
-- code for the duration of the overlap and reinstated on the version
-- column by the contract migration.
ALTER TABLE messages
    DROP CONSTRAINT IF EXISTS messages_topic_embedding_paired;

-- Existing cached vectors carry no version, so they read as stale and
-- get re-embedded on next use by the same backfill that handles a rotation.
UPDATE messages
   SET topic_embedding = NULL,
       topic_embedding_model = NULL
 WHERE topic_embedding IS NOT NULL;
