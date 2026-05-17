-- Cache of LLM-generated starter questions for the chat empty
-- state. Stored as its own table (rather than a JSONB column on
-- `courses`) so a global TRUNCATE forces a regen without touching
-- unrelated course state, and so row presence is the
-- "have we ever generated for this course" signal. Cache lifecycle
-- lives in routes/suggested_questions.rs.

CREATE TABLE course_suggested_questions (
    course_id        UUID PRIMARY KEY REFERENCES courses(id) ON DELETE CASCADE,
    questions        JSONB NOT NULL,
    source_doc_ids   UUID[] NOT NULL,
    model            TEXT NOT NULL,
    generated_at     TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    last_checked_at  TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
