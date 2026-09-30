-- Two additions to NRPS roster sync.
--
-- 1. A per-context opt-out. Roster sync provisions every enrolled LMS user
--    as a Minerva course member, whether or not the LTI activity is visible
--    to students in the LMS. A teacher who keeps the activity hidden can
--    switch sync off; members then join one at a time when they launch the
--    tool. Defaults to TRUE so existing links keep syncing.
--
-- 2. A history of sync runs. `last_sync_added` / `last_sync_removed` on the
--    context only describe the most recent run, and "0 added, 0 removed"
--    says nothing about the run before it that added the whole roster.
--    Only runs that changed membership or failed are kept, so the table
--    grows with events rather than with the sync interval.

ALTER TABLE lti_nrps_contexts
    ADD COLUMN sync_enabled BOOLEAN NOT NULL DEFAULT TRUE;

CREATE TABLE lti_nrps_sync_runs (
    id UUID PRIMARY KEY,
    nrps_context_id UUID NOT NULL REFERENCES lti_nrps_contexts(id) ON DELETE CASCADE,
    ran_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    status TEXT NOT NULL,            -- 'ok' | 'error'
    error TEXT,
    warning TEXT,
    added INTEGER,
    removed INTEGER
);

CREATE INDEX idx_lti_nrps_sync_runs_context_ran_at
    ON lti_nrps_sync_runs (nrps_context_id, ran_at DESC);

-- Seed the history with each context's latest run where it would have been
-- recorded, so the log does not start out empty for links that already
-- synced a roster.
INSERT INTO lti_nrps_sync_runs
    (id, nrps_context_id, ran_at, status, error, warning, added, removed)
SELECT gen_random_uuid(), id, last_sync_at, last_sync_status, last_sync_error,
       last_sync_warning, last_sync_added, last_sync_removed
FROM lti_nrps_contexts
WHERE last_sync_at IS NOT NULL
  AND last_sync_status IS NOT NULL
  AND (last_sync_status <> 'ok'
       OR COALESCE(last_sync_added, 0) + COALESCE(last_sync_removed, 0) > 0);
