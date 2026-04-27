-- Per-course token usage log for the KG / guard pipeline. One row
-- per Cerebras call attributed to a course. Append-only; aggregated
-- by category for the teacher / admin dashboards.
--
-- Categories tracked in this commit (free-form text so future
-- additions don't require a migration):
--   * document_classifier
--   * linker
--   * adversarial_filter
--   * extraction_guard
--
-- Embeddings (OpenAI / fastembed) are deliberately NOT tracked here
-- -- pocket change relative to LLM calls and not attributed to a
-- specific Cerebras-style API.
--
-- No spending limits in this iteration -- the dashboard surfaces
-- usage so teachers / admins can see what's burning tokens, but
-- nothing 429s on threshold. Limits can be added later by joining
-- against a per-course / per-category cap table.

CREATE TABLE course_token_usage (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    course_id UUID NOT NULL REFERENCES courses(id) ON DELETE CASCADE,
    category TEXT NOT NULL,
    -- The actual model name the call ran against (e.g.
    -- 'gpt-oss-120b', 'llama3.1-8b'). Lets the dashboard show
    -- model mix per category and helps when we tune which
    -- operations should run on which size.
    model TEXT NOT NULL,
    prompt_tokens INTEGER NOT NULL DEFAULT 0,
    completion_tokens INTEGER NOT NULL DEFAULT 0,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Course-level "what did I burn this month" rollup.
CREATE INDEX idx_course_token_usage_course_created
    ON course_token_usage (course_id, created_at DESC);

-- Per-category breakdown for a course (the primary dashboard
-- query: "of this course's spend, how much went to the linker vs
-- the guard vs ...").
CREATE INDEX idx_course_token_usage_course_category_created
    ON course_token_usage (course_id, category, created_at DESC);
