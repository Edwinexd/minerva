-- Index to support efficient queue polling: find pending documents quickly.
-- Also covers crash recovery queries for stale "processing" rows.
CREATE INDEX idx_documents_status_created ON documents(status, created_at);
