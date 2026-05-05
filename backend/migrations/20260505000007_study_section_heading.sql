-- Add a display-only `section_heading` question kind so researchers
-- can group long surveys into named blocks (e.g. "System Usability"
-- vs. "User Interface" in the post-survey). Section headings have
-- no answer; the application skips them in response validation and
-- never inserts a corresponding study_survey_responses row.
--
-- We DROP and re-create the kind CHECK because Postgres can't ALTER
-- a CHECK constraint in place. The shape CHECK that pairs `kind`
-- with the likert columns is also relaxed so section_heading is a
-- legal third branch with no likert metadata.

ALTER TABLE study_survey_questions
    DROP CONSTRAINT study_survey_questions_kind_check;
ALTER TABLE study_survey_questions
    ADD CONSTRAINT study_survey_questions_kind_check
        CHECK (kind IN ('likert', 'free_text', 'section_heading'));

ALTER TABLE study_survey_questions
    DROP CONSTRAINT study_survey_questions_check;
ALTER TABLE study_survey_questions
    ADD CONSTRAINT study_survey_questions_check
        CHECK (
            (kind = 'likert'
                AND likert_min IS NOT NULL
                AND likert_max IS NOT NULL
                AND likert_max > likert_min)
            OR
            (kind = 'free_text'
                AND likert_min IS NULL
                AND likert_max IS NULL
                AND likert_min_label IS NULL
                AND likert_max_label IS NULL)
            OR
            (kind = 'section_heading'
                AND likert_min IS NULL
                AND likert_max IS NULL
                AND likert_min_label IS NULL
                AND likert_max_label IS NULL
                AND is_required = FALSE)
        );
