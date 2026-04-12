-- Track when a document entered the 'processing' state so a periodic
-- sweeper can reset docs whose processing task died without updating
-- status (e.g. silent panic or OOM mid-embed). Single-pod-restart
-- recovery still works; this is the between-restart safety net.
ALTER TABLE documents ADD COLUMN processing_started_at TIMESTAMPTZ;

CREATE INDEX idx_documents_processing_started
    ON documents(processing_started_at)
    WHERE status = 'processing';
