-- RunPod async-job ledger. Each row tracks one GPU inference submission
-- (PDF OCR, image OCR, or video timeline indexing) with idempotency-first
-- semantics so a worker crash between submit and persist never leaks a
-- billed job we can't reconcile.
--
-- Submission flow:
--   1. Worker generates a unique `client_request_id` and pre-writes a row
--      with status='submitting' and runpod_job_id=NULL.
--   2. Worker calls RunPod, embedding `client_request_id` in the input
--      payload. RunPod returns its own job id.
--   3. Worker PATCHes runpod_job_id and flips status to 'in_queue'.
--
-- If the worker crashes between (1) and (3), startup reconciliation lists
-- recent RunPod jobs and matches by `client_request_id` from the input
-- payload. No leaked GPU spend.
--
-- Single doc per job for v1: cross-doc batching is deferred until billing
-- data shows the per-doc warmup amortization is worth the partial-failure
-- complexity. See docs/plans/ocr-video-pipeline.md for the trade-off.

CREATE TABLE runpod_jobs (
    id                  UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    -- Our idempotency key, embedded in the RunPod input payload so we can
    -- find the job again if the submit-then-PATCH window crashes.
    client_request_id   TEXT NOT NULL UNIQUE,
    -- RunPod's own job id; NULL while status='submitting', filled in on PATCH.
    runpod_job_id       TEXT UNIQUE,
    -- 'ocr_pdf' | 'ocr_image' | 'video_index'
    task                TEXT NOT NULL,
    document_id         UUID NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
    -- 'submitting' | 'in_queue' | 'in_progress' | 'completed' | 'failed'
    status              TEXT NOT NULL,
    submitted_at        TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    completed_at        TIMESTAMPTZ,
    -- Full RunPod response payload on completion (timeline, OCR pages, etc).
    output              JSONB,
    error               TEXT,
    retry_count         INT NOT NULL DEFAULT 0,
    -- GPU-seconds billed (from RunPod completion payload). Cumulative cost
    -- per doc lives on documents.ocr_gpu_seconds; this is the per-attempt detail.
    gpu_seconds         REAL,
    -- Cached at completion using the RunPod per-second rate so the daily
    -- circuit breaker doesn't have to re-multiply on every poll.
    estimated_cost_usd  REAL
);

-- Worker poll loop hits this every tick to find work. Partial index keeps
-- it tiny because completed/failed rows accumulate forever.
CREATE INDEX idx_runpod_jobs_active_status
    ON runpod_jobs(status)
    WHERE status IN ('submitting', 'in_queue', 'in_progress');

-- Per-doc lookup for "show me the OCR history of this document" admin views
-- and for the cumulative gpu_seconds rollup on documents.
CREATE INDEX idx_runpod_jobs_document_id ON runpod_jobs(document_id);
