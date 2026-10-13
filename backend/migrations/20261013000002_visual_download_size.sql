-- The media host supplies an exact size before the internal download.
-- Keep it separate from staged_bytes so unknown-size authorizations still
-- reserve the full cap, and the stream cannot outgrow its smaller reservation.
ALTER TABLE visual_extraction_jobs
    ADD COLUMN download_size_bytes BIGINT,
    ADD CONSTRAINT visual_download_size CHECK (
        download_size_bytes IS NULL
        OR download_size_bytes BETWEEN 12 AND 1073741824
    );
