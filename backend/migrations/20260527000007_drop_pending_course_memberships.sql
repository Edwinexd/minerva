-- Drop `pending_course_memberships`.
--
-- Introduced by `20260527000004_daisy_course_import.sql` as a queue
-- for course memberships whose target user didn't yet have a Minerva
-- row. Obsoleted in the same hour by the refactor that resolves
-- Daisy participants via `users::find_or_create_by_eppn` (mirroring
-- the auth middleware's first-Shib-launch path); there's no longer a
-- "user doesn't exist yet" state for the Daisy importer to queue
-- against. Table never received production writes between the
-- introducing and dropping migrations, so the data loss is zero.

DROP TABLE IF EXISTS pending_course_memberships;
