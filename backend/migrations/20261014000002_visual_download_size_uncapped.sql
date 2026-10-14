-- The staging window, not a fixed per-video cap, decides whether a lecture
-- fits: an offer grows its reservation when the window has room. Keep only
-- the sanity floor.
ALTER TABLE visual_extraction_jobs
    DROP CONSTRAINT visual_download_size,
    ADD CONSTRAINT visual_download_size CHECK (
        download_size_bytes IS NULL OR download_size_bytes >= 12
    );
