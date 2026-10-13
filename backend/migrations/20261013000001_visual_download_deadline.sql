-- Actions hands off short-lived Play URLs, never MP4 bytes. The scheduler
-- drains this queue by the token's expiry rather than by recording age.
ALTER TABLE visual_extraction_jobs
    ADD COLUMN download_url TEXT,
    ADD COLUMN download_deadline TIMESTAMPTZ,
    ADD COLUMN download_started_at TIMESTAMPTZ,
    ADD CONSTRAINT visual_download_pair CHECK (
        (download_url IS NULL) = (download_deadline IS NULL)
    );
CREATE INDEX idx_visual_download_deadline
    ON visual_extraction_jobs(download_deadline, id)
    WHERE status = 'fetching' AND download_started_at IS NULL
        AND download_url IS NOT NULL;
