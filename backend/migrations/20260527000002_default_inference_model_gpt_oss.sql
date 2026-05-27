-- Cerebras deprecated qwen-3-235b-a22b-instruct-2507 (the previous
-- default course inference model, set by 20260401000002) and
-- llama3.1-8b (the cheap small model the classifier / linker / aegis
-- / extraction-guard / suggested-questions code paths all used to
-- target). Both will start returning 4xx on the Cerebras API soon.
--
-- This migration:
--   1. Sets the new default for `courses.model` to gpt-oss-120b so
--      newly-created courses don't have to think about it.
--   2. Bumps every existing course row that's still pointed at one
--      of the two deprecated models to gpt-oss-120b. We do NOT touch
--      llama-3.3-70b rows (older default from 20260324000001, not on
--      Cerebras' deprecation list) or any other custom string a
--      teacher may have set via /admin/courses.
--
-- Historical `course_token_usage` rows keep their original `model`
-- text; the dashboard's per-model bucket split stays honest about
-- which calls actually went to which model at the time.

ALTER TABLE courses ALTER COLUMN model SET DEFAULT 'gpt-oss-120b';

UPDATE courses
   SET model = 'gpt-oss-120b'
 WHERE model IN ('qwen-3-235b-a22b-instruct-2507', 'llama3.1-8b');
