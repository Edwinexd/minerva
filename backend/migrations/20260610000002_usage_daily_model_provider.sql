-- Record which model (+ provider) produced each batch of usage tokens,
-- so USD cost can be derived on read from the model's current rate in
-- chat_models. We do NOT store dollars in the ledger: the price is a
-- single source of truth (chat_models), and computing $ on read means a
-- later re-price never rewrites historical spend.
--
-- messages.model_used and course_token_usage.model already carry the
-- billing model, so only usage_daily needs the new dimension. To keep
-- per-model token sums (a teacher could switch a course's model mid-day,
-- and the two models may have different rates), the daily aggregate is
-- re-keyed to include the model.

ALTER TABLE usage_daily
    ADD COLUMN model    TEXT,
    ADD COLUMN provider TEXT;

-- Backfill existing aggregate rows to the seeded Cerebras default: all
-- spend to date was on gpt-oss-120b, so this keeps historical rows
-- cost-attributable under the on-read price join.
UPDATE usage_daily
   SET model = 'gpt-oss-120b', provider = 'cerebras'
 WHERE model IS NULL;

ALTER TABLE usage_daily ALTER COLUMN model SET NOT NULL;
ALTER TABLE usage_daily ALTER COLUMN provider SET NOT NULL;

-- Re-key the daily aggregate to include the model so per-model token
-- sums (and thus on-read cost) are preserved when a course changes model.
ALTER TABLE usage_daily DROP CONSTRAINT usage_daily_user_id_course_id_date_key;
ALTER TABLE usage_daily
    ADD CONSTRAINT usage_daily_user_id_course_id_date_model_key
        UNIQUE (user_id, course_id, date, model);
