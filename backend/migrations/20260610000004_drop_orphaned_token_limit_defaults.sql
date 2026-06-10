-- Remove the now-orphaned system_defaults rows for the retired token-cap
-- knobs. A deployment that booted a prior version seeded these into the
-- JSONB key/value table; the registry no longer lists them (replaced by
-- course.daily_cost_limit_usd / platform.owner_daily_cost_limit_usd in the
-- cost_limits change), so without this they linger as dead rows the admin
-- UI never renders. A fresh install never had them, so this is a harmless
-- no-op there.
--
-- Split out from 20260610000003_cost_limits.sql because that migration is
-- already committed (and so immutable); amending a migration is only safe
-- as a new file with a later timestamp.
DELETE FROM system_defaults
 WHERE key IN ('course.daily_token_limit', 'platform.owner_daily_token_limit');
