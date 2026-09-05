-- Per-conversation token ceilings: nudge the student into a fresh
-- conversation, then stop the thread growing entirely.
--
-- Why per-conversation and not per-day: the existing caps
-- (`courses.daily_cost_limit_usd`, `users.owner_daily_cost_limit_usd`)
-- bound total spend, but say nothing about *how* it is spent. The chat
-- route re-sends the whole message history on every turn
-- (`strategy::common::build_chat_messages`), and the tool_use research
-- loop re-sends it once per iteration, so cost per turn grows with
-- conversation length and total cost per conversation grows roughly
-- quadratically. Measured on prod: mean prompt tokens per assistant
-- turn is 29k at turns 1-2 and 95k at turn 41+, and the 11 conversations
-- with 21+ turns account for a quarter of all prompt tokens ever billed.
--
-- Splitting also fixes teacher insight. `conversation_topics` clusters
-- one point per conversation and truncates each to 900 characters of
-- student messages, so a 55-turn thread covering ten different exercises
-- lands as a single truncated data point under one label. The threads
-- that cost the most are the ones the dashboard sees least of.
--
-- The unit is tokens, not USD, deliberately. The other two caps are
-- budgets and belong in dollars; this one is a hygiene limit on context
-- length, and context length is a token quantity regardless of what a
-- model happens to charge for it. It is also the number the per-message
-- footer and the usage dashboards already show the user.
--
-- Defaults are drawn from the observed distribution of cumulative
-- billed tokens per conversation (n=683): p50 36k, p75 101k, p90 189k,
-- p95 289k, p99 908k, max 3.4M.
--   * 300000 soft  -> nudges ~4.7% of conversations, on average at turn 9.
--   * 1000000 hard -> blocks ~0.9% of conversations, on average at turn 24.
-- The soft default is set well into the tail on purpose: a nudge that
-- fires on a routine conversation is one students learn to dismiss.
-- 0 disables either limit, matching the `0 = unlimited` convention the
-- spend caps already use.
--
-- These columns are inert until the `conversation_limits` feature flag
-- is on for the course (`minerva_app_core::feature_flags`), which it is
-- not by default. So this migration changes no behaviour on deploy; it
-- only stocks the per-course thresholds that take effect as the flag is
-- rolled out course by course. Turning the flag back off ignores the
-- thresholds without erasing them.

ALTER TABLE courses
    ADD COLUMN conversation_soft_token_limit BIGINT NOT NULL DEFAULT 300000,
    ADD COLUMN conversation_hard_token_limit BIGINT NOT NULL DEFAULT 1000000;

ALTER TABLE courses
    ADD CONSTRAINT courses_conversation_token_limits_non_negative
        CHECK (conversation_soft_token_limit >= 0
               AND conversation_hard_token_limit >= 0);

-- A hard limit below the soft one would block before it ever nudged,
-- so the student would hit the wall with no warning. Either limit may
-- still be 0 (disabled) independently of the other.
ALTER TABLE courses
    ADD CONSTRAINT courses_conversation_hard_limit_above_soft
        CHECK (conversation_hard_token_limit = 0
               OR conversation_soft_token_limit = 0
               OR conversation_hard_token_limit >= conversation_soft_token_limit);

-- Continuation link. When a student splits a capped thread, the new
-- conversation records which one it came from and carries a bounded
-- summary of it so the student does not restart from nothing.
--
-- `carryover_summary` is written once at split time and never updated.
-- It is model-generated from the previous conversation's messages, so
-- it is treated as untrusted content wherever it is rendered into a
-- prompt (see `strategy::common::build_system_prompt_with_signals`).
--
-- ON DELETE SET NULL rather than CASCADE: deleting the original thread
-- must not take the continuation (and its whole message history) with
-- it. The continuation stands on its own; it just loses the backlink.
ALTER TABLE conversations
    ADD COLUMN continued_from_id UUID REFERENCES conversations(id) ON DELETE SET NULL,
    ADD COLUMN carryover_summary TEXT;
