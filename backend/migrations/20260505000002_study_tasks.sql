-- Study tasks: the ordered list of prompts a participant works through.
-- For the current Aegis evaluation these are hardcoded by the researcher
-- via the admin UI before launch; the table is the natural shape so
-- future studies can reuse the infra without code changes.
--
-- task_index is 0-based and dense within a course (the application
-- enforces this when the admin saves the list); UNIQUE(course_id,
-- task_index) protects against two rows claiming the same slot.
-- description holds the task body shown to the participant, e.g.
-- "Your task is to create a prompt that helps you understand what
-- environmental and technological challenges must be addressed for
-- humans to live on Mars." Plain text, rendered with whitespace
-- preserved.

CREATE TABLE study_tasks (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    course_id   UUID NOT NULL REFERENCES courses(id) ON DELETE CASCADE,
    task_index  INTEGER NOT NULL CHECK (task_index >= 0),
    title       TEXT NOT NULL,
    description TEXT NOT NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (course_id, task_index)
);

CREATE INDEX idx_study_tasks_course ON study_tasks(course_id);
