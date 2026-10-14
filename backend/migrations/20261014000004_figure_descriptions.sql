-- Independent post-extraction enrichment. Existing crops are backfilled
-- without re-running OCR. Leases and retries belong to this stage alone.
CREATE TABLE figure_descriptions (
    figure_id UUID PRIMARY KEY REFERENCES document_figures(id) ON DELETE CASCADE,
    model TEXT NOT NULL,
    version INTEGER NOT NULL,
    description TEXT,
    caption TEXT,
    is_decorative BOOLEAN,
    described_at TIMESTAMPTZ,
    attempts INTEGER NOT NULL DEFAULT 0,
    retry_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    lease UUID,
    lease_expires_at TIMESTAMPTZ,
    error_msg TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_figure_descriptions_pending
    ON figure_descriptions(retry_at, created_at) WHERE described_at IS NULL;
