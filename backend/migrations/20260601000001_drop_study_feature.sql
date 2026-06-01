-- Remove the temporary research-study feature.
--
-- The study pipeline (consent screen, surveys, hardcoded tasks, lockout,
-- JSONL export) and its study-only Aegis live-iteration capture have been
-- stripped from the application. This forward-only migration drops the
-- schema they introduced (migrations 20260505000001..20260511000001).
--
-- The original study migrations are kept on disk so SQLx's migrator does
-- not trip on already-applied versions; this migration is what actually
-- removes the tables on existing databases. CASCADE clears the study
-- tables' own indexes and self-referential FKs; no core table references
-- these, so nothing outside the study feature is affected.

DROP TABLE IF EXISTS aegis_iterations CASCADE;
DROP TABLE IF EXISTS study_task_conversations CASCADE;
DROP TABLE IF EXISTS study_participant_state CASCADE;
DROP TABLE IF EXISTS study_survey_responses CASCADE;
DROP TABLE IF EXISTS study_survey_questions CASCADE;
DROP TABLE IF EXISTS study_surveys CASCADE;
DROP TABLE IF EXISTS study_tasks CASCADE;
DROP TABLE IF EXISTS study_courses CASCADE;

-- Drop any lingering per-course / global study_mode feature-flag rows.
DELETE FROM feature_flags WHERE flag = 'study_mode';
