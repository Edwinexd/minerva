-- Parent-child link for derived documents.
--
-- Previously, when the ingest worker materialized the bytes behind a
-- `text/x-url` stub (today: external transcript pipeline for
-- play.dsv.su.se; new: inline-download for GitHub PDF URLs), it rewrote
-- the URL row in place. `filename`, `mime_type`, `size_bytes`, and
-- `content_hash` all switched to the downloaded artifact's values and
-- the original URL identity survived only as the opaque `source_url`
-- column. That made it impossible to answer "what URL produced this
-- doc?" without joining through `source_url`, blurred dedup semantics
-- (content_hash flipped from a hash of the URL string to a hash of the
-- materialized bytes on the same row), and lost the historical fact
-- that the doc was discovered as a URL rather than uploaded directly.
--
-- New shape: the URL row is permanent. It stays `text/x-url` forever.
-- Once the bytes are available, a NEW doc row is inserted carrying
-- `parent_document_id = url_row.id` and the URL parent transitions to
-- a new status `tracked`. The child is what gets chunked, embedded,
-- and shown in chat retrieval; the parent is metadata.
--
-- Backfill: existing rewritten docs (where the URL identity was lost
-- before this migration) stay as they are. We could synthesize a
-- parent for every doc whose `source_url` is set, but those rows
-- already serve the application correctly and reverse-engineering
-- the parent's `created_at` from `source_url` alone would be a guess.
-- Future URL ingests use the parent-child model.

ALTER TABLE documents
    ADD COLUMN parent_document_id UUID REFERENCES documents(id) ON DELETE CASCADE;

-- At most one active child per parent. If the URL is re-fetched in
-- the future (manual "refresh" UX), the previous child gets
-- soft-orphaned first so the new one can claim the slot without
-- violating this constraint.
CREATE UNIQUE INDEX idx_documents_parent_active
    ON documents (parent_document_id)
    WHERE parent_document_id IS NOT NULL AND orphaned_at IS NULL;

-- Cheap lookup for "list every doc derived from this URL" (including
-- orphaned children). Used by the docs UI to surface the relationship
-- and by the worker to find a stale child to orphan on re-fetch.
CREATE INDEX idx_documents_parent_document_id
    ON documents (parent_document_id)
    WHERE parent_document_id IS NOT NULL;

-- Adjust the existing content-hash dedup index so URL-derived children
-- don't compete with teacher uploads for the same slot. Without this,
-- two URLs in the same course that resolve to the same PDF bytes
-- would collide; and a teacher uploading a PDF that happens to match
-- a URL child's bytes would dedup against the URL child (returning a
-- doc whose identity is "URL X" instead of inserting the upload),
-- which would be confusing UX.
--
-- New semantics: only first-class docs (parent_document_id IS NULL)
-- participate in content-hash dedup. URL children are exempt; their
-- identity is their parent URL, not their content.
DROP INDEX idx_documents_course_content_hash_active;
CREATE UNIQUE INDEX idx_documents_course_content_hash_active
    ON documents (course_id, content_hash)
    WHERE content_hash IS NOT NULL
      AND orphaned_at IS NULL
      AND parent_document_id IS NULL;
