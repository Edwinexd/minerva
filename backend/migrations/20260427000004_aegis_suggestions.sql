-- Aegis: replace the score-based prompt-analysis schema with a
-- suggestions-based one. The original 5-dimensional 0..=10 scoring
-- (overall_score, clarity_score, ...) framed the analyzer as a
-- judge; per the project brief it should be a partner offering
-- concrete improvements, not a grader. Scoring also encouraged
-- exactly the condescending tone Herodotou et al. warn against.
--
-- New shape:
--   * `suggestions` is a JSONB array of `{kind, text}` objects.
--     `kind` is a short tag ("context", "constraints", "specificity",
--     "alternatives", "clarification") so the panel can render an
--     icon / colour per category. `text` is a single-sentence
--     actionable improvement.
--     Empty array = "the prompt is fine, no suggestions to make".
--   * `mode` is the calibration the analyzer was running under for
--     this row ("beginner" | "expert"), so the History panel can
--     show "this analysis was made when you said you were a
--     beginner" if we ever want to surface that.
--   * `model_used` retained for audit (which Cerebras model produced
--     this row).
--
-- The previous table only rolled out a few hours ago and held a
-- handful of rows for one course; we DROP rather than migrate
-- because the data shape is fundamentally different and there's
-- nothing worth preserving.

DROP TABLE IF EXISTS prompt_analyses;

CREATE TABLE prompt_analyses (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    message_id  UUID NOT NULL UNIQUE REFERENCES messages(id) ON DELETE CASCADE,
    suggestions JSONB NOT NULL DEFAULT '[]'::jsonb,
    mode        TEXT  NOT NULL CHECK (mode IN ('beginner', 'expert')),
    model_used  TEXT  NOT NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_prompt_analyses_message ON prompt_analyses(message_id);
