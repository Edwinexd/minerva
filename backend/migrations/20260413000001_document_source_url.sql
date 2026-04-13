-- Track the origin URL for URL-sourced documents (e.g. play.dsv.su.se
-- presentations) so dedup survives the transcript-fetch flow that rewrites
-- mime_type from 'text/x-url' to 'text/plain'. Prior to this column, discovery
-- re-created the same URL doc every hourly run because the old check filtered
-- on mime_type.
ALTER TABLE documents ADD COLUMN source_url TEXT;

-- Partial unique index enforces atomic per-course dedup for URL docs and
-- prevents concurrent discovery runs from racing past the application check.
CREATE UNIQUE INDEX idx_documents_course_source_url
    ON documents(course_id, source_url)
    WHERE source_url IS NOT NULL;
