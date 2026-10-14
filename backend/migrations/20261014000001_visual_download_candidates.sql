-- A lecture with several Play sources is offered as candidates; the fetcher
-- probes their frames, then promotes the slide source to download_url. The
-- deadline therefore exists before a URL does. Both are bearer URLs and are
-- cleared with download_url.
ALTER TABLE visual_extraction_jobs
    ADD COLUMN download_candidates JSONB,
    ADD COLUMN probe_started_at TIMESTAMPTZ,
    DROP CONSTRAINT visual_download_pair,
    ADD CONSTRAINT visual_download_pair CHECK (
        (download_deadline IS NULL)
        = (download_url IS NULL AND download_candidates IS NULL)
    ),
    ADD CONSTRAINT visual_download_choice CHECK (
        download_url IS NULL OR download_candidates IS NULL
    );
