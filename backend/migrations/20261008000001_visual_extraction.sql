-- Visual extraction: slide OCR for Play lectures and figure extraction for
-- PDFs, run on DSV's Olympus Slurm cluster. See "Visual extraction
-- pipeline" in docs/ARCHITECTURE.md for who does what.
--
-- Minerva is the only place this state lives. GitHub Actions stages lecture
-- videos into a bounded window, minerva-scheduler submits and monitors Slurm
-- workers, and workers pull one job at a time through presigned URLs.

-- One row per Slurm worker job the scheduler submitted. `id` is what the
-- worker's signed URL names, so revoking a worker is a state change here.
CREATE TABLE visual_extraction_workers (
    id UUID PRIMARY KEY,
    slurm_job_id BIGINT,
    state TEXT NOT NULL DEFAULT 'submitting'
        CHECK (state IN ('submitting', 'queued', 'running', 'finished', 'failed')),
    submitted_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    -- The worker's signed URL stops working here; matches the Slurm wall
    -- time plus the time it may wait in the queue.
    expires_at TIMESTAMPTZ NOT NULL,
    last_seen_at TIMESTAMPTZ,
    finished_at TIMESTAMPTZ,
    error_msg TEXT
);

CREATE INDEX idx_visual_extraction_workers_active
    ON visual_extraction_workers(state)
    WHERE state IN ('submitting', 'queued', 'running');

-- One row per source document: the `.url` parent of a Play lecture, or a
-- PDF. Lifecycle:
--   play_lecture: needs_source -> fetching -> ready -> leased -> done
--   pdf:                                      ready -> leased -> done
-- with `failed` after too many attempts and `unavailable` when Play has no
-- video. A lease that expires, or whose worker dies, goes back to `ready`.
CREATE TABLE visual_extraction_jobs (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    document_id UUID NOT NULL UNIQUE REFERENCES documents(id) ON DELETE CASCADE,
    course_id UUID NOT NULL REFERENCES courses(id) ON DELETE CASCADE,
    kind TEXT NOT NULL CHECK (kind IN ('play_lecture', 'pdf')),
    status TEXT NOT NULL
        CHECK (status IN ('needs_source', 'fetching', 'ready', 'leased', 'done', 'failed', 'unavailable')),
    priority INTEGER NOT NULL DEFAULT 0,
    attempts INTEGER NOT NULL DEFAULT 0,
    -- Bumped in code when the model or thresholds change; done rows with an
    -- older version are re-enqueued at a bounded rate.
    pipeline_version INTEGER NOT NULL,
    -- Staged lecture video, relative to the staging directory. NULL for a
    -- PDF, whose source is the document file itself.
    staged_path TEXT,
    -- Bytes counted against the staging budget: the size GitHub Actions
    -- announced when reserving a slot, then the size actually stored.
    staged_bytes BIGINT,
    -- Timed transcript cues ([{start, end, text}]) uploaded with a lecture's
    -- video; speech is aligned to slides from these at ingest.
    cues JSONB,
    fetch_reserved_at TIMESTAMPTZ,
    worker_id UUID REFERENCES visual_extraction_workers(id) ON DELETE SET NULL,
    lease_expires_at TIMESTAMPTZ,
    error_msg TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    completed_at TIMESTAMPTZ
);

-- The lease query scans ready jobs by priority then age.
CREATE INDEX idx_visual_extraction_jobs_ready
    ON visual_extraction_jobs(priority DESC, created_at)
    WHERE status = 'ready';

-- The staging budget sums bytes over reserved and staged lecture jobs.
CREATE INDEX idx_visual_extraction_jobs_staged
    ON visual_extraction_jobs(status)
    WHERE staged_bytes IS NOT NULL;

-- Per-page (PDF) or per-slide (lecture) layout from the OCR pass, keyed to
-- the job's source document so a lecture's slides survive its text child
-- being replaced. Rewritten wholesale on every accepted result.
CREATE TABLE document_visual_pages (
    document_id UUID NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
    position INTEGER NOT NULL,
    -- The number printed on the slide or the PDF page index, when known.
    page_number INTEGER,
    start_seconds REAL,
    end_seconds REAL,
    -- Stored slide frame for lectures (shown with citations). NULL for PDF
    -- pages, which render from the PDF itself.
    image_path TEXT,
    -- Labelled layout blocks with boxes as frame fractions:
    -- [{label, boxes: [[x0, y0, x1, y1]], text}].
    blocks JSONB NOT NULL,
    text TEXT NOT NULL,
    PRIMARY KEY (document_id, position)
);

-- Figures cropped out of slides and pages. Searched two ways, both always
-- run: the context text through the course's text embedder, and the crop's
-- CLIP vector (computed on Olympus) through the paired CLIP text encoder.
CREATE TABLE document_figures (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    document_id UUID NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
    course_id UUID NOT NULL REFERENCES courses(id) ON DELETE CASCADE,
    -- The document_visual_pages position the figure was cropped from.
    page_position INTEGER NOT NULL,
    box REAL[] NOT NULL,
    image_path TEXT NOT NULL,
    caption TEXT,
    -- Title, nearby text, and for lectures what was said while the slide
    -- was up: what students describe a figure by.
    context TEXT NOT NULL,
    -- CLIP image embedding computed on Olympus. Kept here so the Qdrant
    -- point can be (re)built without a GPU, e.g. after a collection reset.
    visual_model TEXT NOT NULL,
    visual_vector REAL[] NOT NULL,
    -- Set once both vectors (visual, and context through the course's text
    -- embedder) are in Qdrant; the indexing sweep picks up NULLs.
    indexed_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_document_figures_document ON document_figures(document_id);
CREATE INDEX idx_document_figures_course ON document_figures(course_id);
CREATE INDEX idx_document_figures_unindexed ON document_figures(created_at) WHERE indexed_at IS NULL;

-- Figures shown with an assistant reply, in the order they were offered.
-- Rows go with the figure when a newer OCR result replaces it, so an old
-- reply never points at an image that is gone.
CREATE TABLE message_figures (
    message_id UUID NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
    position INTEGER NOT NULL,
    figure_id UUID NOT NULL REFERENCES document_figures(id) ON DELETE CASCADE,
    PRIMARY KEY (message_id, position)
);

CREATE INDEX idx_message_figures_figure ON message_figures(figure_id);
