-- Split out the research-phase token spend from the writeup spend on
-- assistant messages produced by `tool_use_enabled` courses, so the
-- per-message footer and the daily usage view can render
-- `total = research + writeup` instead of a single opaque number.
--
-- `research_tokens` is the SUM of research-phase prompt + completion
-- tokens (one number per row to keep the columns tight; the writeup
-- portion is derivable as `tokens_prompt + tokens_completion -
-- research_tokens`). NULL on assistant messages produced by the
-- legacy single-pass paths (no research phase ran) and on user
-- messages.
ALTER TABLE messages ADD COLUMN research_tokens INTEGER;

-- Daily-aggregate breakdown for the teacher usage view. Defaulted to
-- 0 so the existing `record_usage` call sites that don't know about
-- the split still produce valid rows; only chat-route token records
-- coming out of `tool_use::run` will pass a non-zero value.
ALTER TABLE usage_daily ADD COLUMN research_tokens BIGINT NOT NULL DEFAULT 0;
