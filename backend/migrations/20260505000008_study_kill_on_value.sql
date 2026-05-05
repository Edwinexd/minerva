-- Withdraw-on-answer kill switch for survey questions. The GDPR
-- consent question on the pre-survey is the motivating example: if a
-- participant answers "No, don't save my data" the study should
-- terminate cleanly rather than collect more data from them.
--
-- `kill_on_value` is interpreted only for `likert` questions: if the
-- answered `likert_value` matches, the application skips the normal
-- stage advance and jumps straight to `done` (with `locked_out_at`
-- set), so the chat path is blocked and only the thank-you screen
-- renders. NULL on every other question kind / whenever the kill
-- switch isn't wanted; that's the default.
--
-- Kept on the question (rather than as a survey-level setting)
-- because a single survey may have multiple kill switches (e.g.
-- "are you over 18?" + "do you consent to data storage?") that all
-- need to short-circuit independently.

ALTER TABLE study_survey_questions
    ADD COLUMN kill_on_value INTEGER;

ALTER TABLE study_survey_questions
    ADD CONSTRAINT study_survey_questions_kill_on_value_check
        CHECK (
            kill_on_value IS NULL
            OR (
                kind = 'likert'
                AND likert_min IS NOT NULL
                AND likert_max IS NOT NULL
                AND kill_on_value >= likert_min
                AND kill_on_value <= likert_max
            )
        );
