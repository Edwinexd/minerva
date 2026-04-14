-- Normalize all stored eppns to lowercase so `alice@su.se` and `alice@SU.SE`
-- resolve to the same user row. Auth code now lowercases on ingress; this
-- backfills existing rows so prior mixed-case data is consistent.
--
-- If two rows differ only in case (e.g. one row `alice@SU.SE` and another
-- `alice@su.se`), this migration fails on the UNIQUE constraint. Resolve by
-- merging the duplicates manually, then re-run.

UPDATE users
SET eppn = lower(eppn)
WHERE eppn <> lower(eppn);

UPDATE external_auth_invites
SET eppn = lower(eppn)
WHERE eppn <> lower(eppn);
