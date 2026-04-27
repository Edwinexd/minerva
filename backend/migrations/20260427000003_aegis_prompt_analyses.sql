-- Aegis: per-user-message prompt-coaching scores + dimensional
-- feedback. Populated by the analyzer LLM call that runs in parallel
-- with the assistant generation when the `aegis` feature flag is on.
--
-- One row per user message at most; the analyzer is best-effort
-- (soft-fails on transport / JSON / DB errors) so messages without a
-- corresponding row simply render no panel content for that turn.
-- The chat path itself never waits on this table; failure here
-- doesn't block inference.
--
-- Scores are 0..=10 inclusive (CHECK below). The five dimensions
-- mirror the rubric in the project description (clarity, context,
-- constraints, reasoning demand, critical thinking signals); the
-- overall_score is the analyzer's aggregate (NOT a server-side
-- average) so the model can weight the dimensions per-prompt rather
-- than trusting an arithmetic mean.
--
-- The structural / terminology / constraint feedback strings are the
-- short three-bullet writeup shown in the figma's "Prompt Analysis"
-- section. Empty string = the dimension didn't merit a callout for
-- this prompt.
--
-- ON DELETE CASCADE: when a message is deleted (conversation purge,
-- user wipe), the analysis goes with it.

CREATE TABLE prompt_analyses (
    id                            UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    message_id                    UUID NOT NULL UNIQUE REFERENCES messages(id) ON DELETE CASCADE,
    overall_score                 INTEGER NOT NULL CHECK (overall_score BETWEEN 0 AND 10),
    clarity_score                 INTEGER NOT NULL CHECK (clarity_score BETWEEN 0 AND 10),
    context_score                 INTEGER NOT NULL CHECK (context_score BETWEEN 0 AND 10),
    constraints_score             INTEGER NOT NULL CHECK (constraints_score BETWEEN 0 AND 10),
    reasoning_demand_score        INTEGER NOT NULL CHECK (reasoning_demand_score BETWEEN 0 AND 10),
    critical_thinking_score       INTEGER NOT NULL CHECK (critical_thinking_score BETWEEN 0 AND 10),
    structural_clarity_label      TEXT NOT NULL,
    structural_clarity_feedback   TEXT NOT NULL,
    terminology_label             TEXT NOT NULL,
    terminology_feedback          TEXT NOT NULL,
    missing_constraint_label      TEXT NOT NULL,
    missing_constraint_feedback   TEXT NOT NULL,
    model_used                    TEXT NOT NULL,
    created_at                    TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_prompt_analyses_message ON prompt_analyses(message_id);
