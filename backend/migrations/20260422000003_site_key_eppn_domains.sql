-- Restrict a site integration key to acting only on behalf of eppns ending
-- in one of a fixed set of domains. Admin sets this at mint time
-- (immutable thereafter; rotate to change).
--
-- NULL = no restriction (backwards compatible with existing rows).
-- Non-empty array = the acting eppn must end with `@<domain>` for some
-- domain in the list, case-insensitively. Empty array is treated as NULL
-- for operational sanity (otherwise the key would be immediately useless).
ALTER TABLE site_integration_keys
    ADD COLUMN allowed_eppn_domains TEXT[];
