-- NRPS sync can succeed (200 OK from the platform with a parseable membership
-- container) yet be silently degraded: the most common case is a platform
-- whose tool-privacy settings hide every member's name/email and don't surface
-- the `user_eppn` custom claim either, leaving the reconcile with no way to
-- map an LMS user to a real Minerva eppn. The sync still runs (it falls back
-- to a synthetic `lti_<source>_<sub>` identifier), but the resulting members
-- are unlinkable from any other identity source (Shibboleth, manual add).
--
-- `last_sync_warning` carries an actionable, human-readable note in that
-- case. It is independent of `last_sync_status`: a sync can be `ok` and still
-- carry a warning. The status UI shows it as a separate "needs attention"
-- badge so admins can fix the LMS-side config without first having to dig
-- into logs.

ALTER TABLE lti_nrps_contexts
    ADD COLUMN last_sync_warning TEXT;
