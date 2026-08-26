-- Retryable ingest failures.
--
-- Every error out of the ingest pipeline used to land the document in
-- `failed`, terminal, including transient ones like "embed RPC failed:
-- tcp connect error". A single embedder restart therefore burned 5032
-- documents in one sweep, all of them perfectly ingestable, and getting
-- them back took a hand-written UPDATE against prod.
--
-- These two columns let the worker distinguish "this document is broken"
-- from "the pod was busy, come back later". `retry_after` keeps a
-- requeued document out of `claim_pending`'s ORDER BY created_at window
-- until its backoff expires, which is what stops a deferred document
-- from being re-claimed on the very next tick and hot-looping.
ALTER TABLE documents
    ADD COLUMN ingest_attempts INTEGER NOT NULL DEFAULT 0,
    ADD COLUMN retry_after TIMESTAMPTZ;

-- claim_pending filters on (status, retry_after) and orders by
-- created_at, so index the pending-and-due set it actually scans.
CREATE INDEX IF NOT EXISTS idx_documents_pending_retry
    ON documents(retry_after, created_at)
    WHERE status = 'pending';
