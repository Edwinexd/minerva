-- A lecture Play cannot serve yet (no transcript, no video) waits before
-- Actions is offered it again. The wait doubles with every such release:
-- new lectures usually become ready within hours, old ones may never.
ALTER TABLE visual_extraction_jobs
    ADD COLUMN retry_after TIMESTAMPTZ,
    ADD COLUMN not_ready_releases INTEGER NOT NULL DEFAULT 0;
