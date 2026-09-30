-- The per-conversation token ceilings and the topic-switch nudge now
-- apply to every course, so their opt-in gates are gone from the
-- application. Drop the per-course rows so the admin UI does not keep
-- offering dead toggles.

DELETE FROM feature_flags WHERE flag IN ('conversation_limits', 'topic_switch_nudge');

-- Courses created before the `course.conversation_*` system defaults
-- existed still hold the column defaults (300000 / 1000000), which were
-- inert while the flag was off. Move those onto the system defaults,
-- the values the ceilings were validated at, so going live does not
-- start every existing course on thresholds nobody has run. A course
-- whose limits were edited is left alone.

UPDATE courses
SET conversation_soft_token_limit = d.soft,
    conversation_hard_token_limit = d.hard
FROM (
    SELECT
        (SELECT (value #>> '{}')::bigint FROM system_defaults
          WHERE key = 'course.conversation_soft_token_limit') AS soft,
        (SELECT (value #>> '{}')::bigint FROM system_defaults
          WHERE key = 'course.conversation_hard_token_limit') AS hard
) d
WHERE courses.conversation_soft_token_limit = 300000
  AND courses.conversation_hard_token_limit = 1000000
  AND d.soft IS NOT NULL
  AND d.hard IS NOT NULL
  AND (d.hard = 0 OR d.soft = 0 OR d.hard >= d.soft);
