-- Server-side dedup + Moodle source identity + soft orphaning.
--
-- Designed once across three rollout slices:
--   slice 1: content_hash + orphaned_at (idempotent uploads, retrieval filter)
--   slice 2: source_system + source_ref (plugin tells server which Moodle
--            object each doc maps to so re-uploads can supersede the old
--            row and reconcile can orphan deleted sources)
--   slice 3: same columns reused for one-doc-per-forum mod_forum syncs
--
-- Orphaning is soft (orphaned_at TIMESTAMPTZ NULL) because documents are
-- referenced by chat history (messages.chunks_used cites doc ids); hard
-- deletes would 404 user-visible citations. Retrieval queries are taught
-- to skip rows where orphaned_at IS NOT NULL; chat history can still
-- resolve them by id.

ALTER TABLE documents
    ADD COLUMN content_hash  TEXT,
    ADD COLUMN source_system TEXT,
    ADD COLUMN source_ref    TEXT,
    ADD COLUMN orphaned_at   TIMESTAMPTZ;

-- Server-side dedup: at most one active (non-orphaned) doc per
-- (course, content_hash). Plugin no longer needs a local sync_log to
-- prevent duplicates; double-uploads now collapse to the existing row.
-- Legacy rows (content_hash IS NULL until backfilled) are excluded so
-- they don't deadlock each other.
CREATE UNIQUE INDEX idx_documents_course_content_hash_active
    ON documents (course_id, content_hash)
    WHERE content_hash IS NOT NULL AND orphaned_at IS NULL;

-- Source identity: at most one active doc per Moodle (or other plugin)
-- object. On re-upload with a new content_hash but the same source_ref,
-- the route layer orphans the old row first to free this index, then
-- inserts the new one.
CREATE UNIQUE INDEX idx_documents_course_source_active
    ON documents (course_id, source_system, source_ref)
    WHERE source_ref IS NOT NULL AND orphaned_at IS NULL;

-- Reconcile sweep + observer-event deletes need a fast "find all
-- active docs from a given source system for this course" lookup.
CREATE INDEX idx_documents_course_source_system
    ON documents (course_id, source_system)
    WHERE source_system IS NOT NULL AND orphaned_at IS NULL;
