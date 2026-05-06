-- Figures (slide images, document diagrams) extracted by DeepSeek-OCR as
-- first-class rows linked to their source document. Resolves issue #36.
--
-- Each figure carries enough provenance to render a thumbnail in chat
-- citations and to re-locate the bbox in the original page/frame if a
-- teacher reports a mis-crop:
--
-- * For PDFs: `page` is set, `t_start_seconds`/`t_end_seconds` are NULL.
-- * For videos: `page` is NULL, `t_*_seconds` cover the timeline span the
--   figure was extracted from.
--
-- bbox coordinate system is normalized to the OCRed image, post-crop. To
-- recover original-frame pixel coords combine with `documents.crop_bbox`.
-- This single rule covers both PDF pages (no crop, so bbox is page-relative)
-- and video frames (cropped to slide region before OCR).
--
-- Visual embeddings (CLIP-style) are deliberately out of scope for v1;
-- caption text gets embedded via the existing chunker by referencing
-- `figure_id` on the chunk's Qdrant payload.

CREATE TABLE document_figures (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    document_id     UUID NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
    page            INT,                   -- PDF page (1-based), or NULL for video
    t_start_seconds REAL,                  -- video timeline span start, or NULL for PDF
    t_end_seconds   REAL,                  -- video timeline span end, or NULL for PDF
    bbox            JSONB,                 -- {x,y,w,h} normalized 0..1 within the OCRed image
    caption         TEXT,                  -- caption text recognized by DeepSeek-OCR
    storage_path    TEXT NOT NULL,         -- absolute path under MINERVA_DOCS_PATH/figures/
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_document_figures_document_id ON document_figures(document_id);
