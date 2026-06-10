-- Replace the per-student-per-course and per-owner daily TOKEN caps with
-- USD spending caps. Limits are budgets (config), so they are stored as
-- dollars; spend is computed on read from tokens x the model's current
-- rate (no stored $ in the ledger).
--
-- Existing token limits convert to an equivalent USD budget at the
-- seeded gpt-oss-120b blended rate (the model all spend to date ran on,
-- so the conversion preserves an equivalent budget). The blend is a
-- 50/50 input/output split: a token budget is just a prompt+completion
-- count, so any single-number conversion needs some assumed mix, and
-- 50/50 is the honest default; the admin re-tunes the resulting $ limits
-- anyway. A 0 (unlimited) token limit maps to 0 (still unlimited).
--
-- The chat_models migration (step 1) already inserted gpt-oss-120b with
-- its rates, so the blended rate is available here.

ALTER TABLE courses
    ADD COLUMN daily_cost_limit_usd NUMERIC(12,4) NOT NULL DEFAULT 0;
ALTER TABLE users
    ADD COLUMN owner_daily_cost_limit_usd NUMERIC(12,4) NOT NULL DEFAULT 0;

-- blended USD per token = (input + output) / 2 / 1e6, from gpt-oss-120b.
UPDATE courses c
   SET daily_cost_limit_usd = ROUND(
        c.daily_token_limit
        * (SELECT (input_usd_per_mtok + output_usd_per_mtok) / 2 / 1000000
             FROM chat_models WHERE model = 'gpt-oss-120b'),
        4)
 WHERE c.daily_token_limit > 0;

UPDATE users u
   SET owner_daily_cost_limit_usd = ROUND(
        u.owner_daily_token_limit
        * (SELECT (input_usd_per_mtok + output_usd_per_mtok) / 2 / 1000000
             FROM chat_models WHERE model = 'gpt-oss-120b'),
        4)
 WHERE u.owner_daily_token_limit > 0;

-- Drop the token-limit columns: the USD limit is now the single source
-- of truth for enforcement.
ALTER TABLE courses DROP COLUMN daily_token_limit;
ALTER TABLE users DROP COLUMN owner_daily_token_limit;
