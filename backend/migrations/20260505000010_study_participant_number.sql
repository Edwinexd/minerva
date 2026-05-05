-- Persistent per-course participant numbering.
--
-- The original export computed `participant_id` at export time as
-- the row index in `consented_at ASC` order. That worked until
-- researchers wanted (a) a stable identifier they could bookmark
-- ("participant 5") and (b) the ability to delete a person's data
-- on request (GDPR Article 17) without renumbering everyone behind
-- them. Computed-at-export IDs fail both: re-export after a delete
-- shifts every later participant down by one.
--
-- This migration adds `participant_number`, assigned once at consent
-- (see `record_consent` in `minerva-db::queries::study`). Nullable so
-- pre-consent rows that never advance are exempt; UNIQUE per
-- (course_id, participant_number) for the rows that do get assigned.
-- Existing consented rows are backfilled in their existing
-- `consented_at` order so persistent numbers match what the
-- previous index-based export would have produced.

ALTER TABLE study_participant_state
    ADD COLUMN participant_number INTEGER;

-- Backfill existing consented participants in stable order; a row
-- with a NULL `consented_at` is pre-consent and stays NULL.
WITH numbered AS (
    SELECT
        course_id,
        user_id,
        ROW_NUMBER() OVER (
            PARTITION BY course_id
            ORDER BY consented_at, created_at, user_id
        ) AS num
    FROM study_participant_state
    WHERE consented_at IS NOT NULL
)
UPDATE study_participant_state s
SET participant_number = numbered.num
FROM numbered
WHERE s.course_id = numbered.course_id
  AND s.user_id = numbered.user_id;

CREATE UNIQUE INDEX idx_study_participant_number_unique
    ON study_participant_state(course_id, participant_number)
    WHERE participant_number IS NOT NULL;
