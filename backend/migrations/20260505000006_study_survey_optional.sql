-- Researchers need to mark some questions as optional (e.g. an email
-- field on the pre-survey, or a "anything else?" free-text at the end
-- of the post-survey). The original migration treated every question
-- as required; both DB and application validation enforced that. Here
-- we add `is_required` (default TRUE so existing rows stay required)
-- and let the application skip empty-answer validation when the flag
-- is false.

ALTER TABLE study_survey_questions
    ADD COLUMN is_required BOOLEAN NOT NULL DEFAULT TRUE;
