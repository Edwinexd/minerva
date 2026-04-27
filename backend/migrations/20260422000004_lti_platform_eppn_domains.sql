-- Restrict a site-level LTI platform to issuing launches only for eppns
-- ending in one of a fixed set of domains. Set at registration time
-- (immutable thereafter; rotate by re-registering to change).
--
-- Matches the model on site_integration_keys: NULL or empty array means
-- "no restriction" (backwards compatible with existing lti_platforms rows
-- minted earlier on this branch). Non-empty means the eppn resolved from
-- the launch JWT must end with `@<d>` for some `d` in the list. Per-course
-- `lti_registrations` are not scoped here; they're 1:1 with a Minerva
-- course so the blast radius of a misclaim is already limited to that
-- teacher's own course.
ALTER TABLE lti_platforms
    ADD COLUMN allowed_eppn_domains TEXT[];
