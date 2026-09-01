-- Cached semantic themes for the teacher conversation dashboard.
-- The source hash is computed from the bounded set of recent student
-- messages sent to the utility model; a changed hash forces regeneration.

CREATE TABLE course_conversation_topics (
    course_id       UUID PRIMARY KEY REFERENCES courses(id) ON DELETE CASCADE,
    topics          JSONB NOT NULL,
    source_hash     TEXT NOT NULL,
    model           TEXT NOT NULL,
    generated_at    TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
