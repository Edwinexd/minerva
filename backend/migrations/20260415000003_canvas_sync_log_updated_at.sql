-- Track Canvas's reported updated_at on each synced item so we can detect
-- changes and re-sync (re-download + re-chunk) when Canvas advances it.
-- NULL means the log row pre-dates this column or the source doesn't carry
-- a timestamp (e.g. ExternalUrl items).
ALTER TABLE canvas_sync_log
    ADD COLUMN canvas_updated_at TIMESTAMPTZ;
