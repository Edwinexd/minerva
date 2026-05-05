-- Maps each study task slot to the conversation that backs it. A task
-- gets its own fresh conversation (created on first /task/{i}/start
-- hit and cached here) so the per-task transcript is cleanly separated
-- in the export.
--
-- This is the ONLY new join key the export needs: messages,
-- aegis_prompt_analyses, aegis_suggestions, course_token_usage,
-- and any other interaction logging all already key off
-- conversation_id, so the export joins through here and pulls the
-- whole picture without re-instrumenting the chat path.
--
-- UNIQUE(course_id, user_id, task_index) means resuming after a tab
-- close returns the same conversation -- intentional, since researchers
-- want the full transcript even if the participant clicked Done and
-- then poked at the chat afterwards (we record marked_done_at but
-- don't gate further messages, and the lockout middleware kicks in
-- only at stage = 'done').
--
-- ON DELETE CASCADE on conversation_id means deleting the underlying
-- conversation (e.g. via course delete) cleans this row up
-- automatically; conversations.course_id does NOT cascade in the
-- initial schema, so we cascade explicitly on course_id here too.

CREATE TABLE study_task_conversations (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    course_id       UUID NOT NULL REFERENCES courses(id) ON DELETE CASCADE,
    user_id         UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    task_index      INTEGER NOT NULL CHECK (task_index >= 0),
    conversation_id UUID NOT NULL UNIQUE REFERENCES conversations(id) ON DELETE CASCADE,
    started_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    marked_done_at  TIMESTAMPTZ,
    UNIQUE (course_id, user_id, task_index)
);

CREATE INDEX idx_study_task_conversations_course_user
    ON study_task_conversations(course_id, user_id);
