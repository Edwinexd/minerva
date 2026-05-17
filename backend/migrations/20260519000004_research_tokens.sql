-- Carve the research/agentic phase token spend out of the prompt and
-- completion totals on assistant messages produced by
-- `tool_use_enabled` courses. The per-message footer and the daily
-- usage dashboards render research / writeup as honest subsets of the
-- prompt and completion totals; rather than pretending research is a
-- third top-level category sitting alongside prompt and completion.
-- Prompt vs completion is the input/output axis; research vs writeup
-- is the phase axis; the dashboard needs to nest the second inside
-- the first.
--
-- `research_prompt_tokens` / `research_completion_tokens` are the
-- research-phase share. The writeup share is derivable as
-- `tokens_prompt - research_prompt_tokens` and
-- `tokens_completion - research_completion_tokens`. NULL on assistant
-- messages produced by the legacy single-pass paths (no research
-- phase ran) and on user messages.
ALTER TABLE messages
    ADD COLUMN research_prompt_tokens INTEGER,
    ADD COLUMN research_completion_tokens INTEGER;

-- Daily-aggregate breakdown for the teacher usage view. Defaulted to
-- 0 so existing `record_usage` call sites that don't know about the
-- split still produce valid rows; only chat-route token records
-- coming out of `tool_use::run` pass non-zero values.
ALTER TABLE usage_daily
    ADD COLUMN research_prompt_tokens BIGINT NOT NULL DEFAULT 0,
    ADD COLUMN research_completion_tokens BIGINT NOT NULL DEFAULT 0;
