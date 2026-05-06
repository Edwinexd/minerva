-- OCR + video-indexing pipeline foundation: schema additions on `documents`.
--
-- This is step 4a of the plan in docs/plans/ocr-video-pipeline.md: ship the
-- migrations and state-machine vocabulary behind a feature flag, no endpoints
-- yet. The new columns are all nullable / safely defaulted so existing rows
-- stay untouched and the pipeline is a no-op until
-- `MINERVA_OCR_PIPELINE_ENABLED=true` flips on the new worker routes.
--
-- New `documents.status` values introduced (text, no enum to alter):
--   'awaiting_ocr'           - PDF/image queued for DeepSeek-OCR
--   'processing_ocr'         - RunPod OCR job in flight
--   'ocr_failed'             - dead-letter, admin retryable
--   'awaiting_video_index'   - play.dsv video, frames bundle ready for OCR
--   'vtt_pending'            - frames ready but VTT not yet captioned by play
--   'processing_video_index' - RunPod video-index job in flight
--   'video_index_failed'     - dead-letter
--
-- These coexist with the existing `pending` / `processing` / `ready` /
-- `failed` / `awaiting_transcript` / `unsupported` values.

-- OCR-quality marker so admin UI can show which docs went through DeepSeek
-- vs the legacy pdftotext fallback. NULL on existing rows = unknown / legacy.
ALTER TABLE documents ADD COLUMN ocr_quality TEXT;

-- play.dsv tracks have no labels distinguishing presenter cam vs screen
-- capture; the GH ingest worker classifies visually and records its choice
-- here. `selected_track_index` is which mp4 from the candidate list got
-- picked; `slide_track_score` lets us retrospectively audit/tune the
-- classifier. `slide_track_missing` flags lectures where no track had
-- usable slides (transcript-only fallback fired).
-- `slide_track_user_corrected` flips true when a teacher overrides the
-- automatic choice via the correct-track endpoint, so we can sample those
-- as ground truth for re-tuning later.
ALTER TABLE documents ADD COLUMN selected_track_index INT;
ALTER TABLE documents ADD COLUMN slide_track_score REAL;
ALTER TABLE documents ADD COLUMN slide_track_missing BOOLEAN NOT NULL DEFAULT FALSE;
ALTER TABLE documents ADD COLUMN slide_track_user_corrected BOOLEAN NOT NULL DEFAULT FALSE;

-- Picture-in-picture / composite slide region as detected by the GH worker,
-- in original-frame pixel coords: {x, y, w, h}. NULL means "no crop, OCR
-- the whole frame". Stored so re-extracting frames at a different fps later
-- uses the same crop and keeps figure bboxes consistent.
ALTER TABLE documents ADD COLUMN crop_bbox JSONB;

-- Sample rate used when the bundle was extracted, as an ffmpeg-style
-- fraction (e.g. '1/5'). Persisted per-doc rather than per-course so a
-- re-process at a different rate produces a recognizable provenance trail.
ALTER TABLE documents ADD COLUMN sample_fps TEXT;

-- Cumulative GPU-seconds billed for OCRing this doc across all attempts.
-- Per-job detail lives in runpod_jobs; this aggregate is what the admin UI
-- and per-owner cost rollup read so a single re-OCR doesn't require a join.
ALTER TABLE documents ADD COLUMN ocr_gpu_seconds REAL NOT NULL DEFAULT 0;
