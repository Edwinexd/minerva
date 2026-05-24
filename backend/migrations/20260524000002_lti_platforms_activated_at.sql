-- Approval gate for dynamic-registration-installed platforms.
--
-- `/lti/dynamic-register` is intentionally public (the IMS dynreg spec is
-- platform-to-tool; an `Authorization: Bearer` from the platform is the
-- spec-defined auth, and Shibboleth-gating the tool side would break the
-- LMS popup flow). That means a hostile actor with the URL can complete
-- a dynreg handshake against ANY OIDC-compliant server they control,
-- creating an `lti_platforms` row that trusts an attacker-controlled
-- JWKS for launch-JWT validation, which in turn lets them mint launches
-- claiming any eppn they want.
--
-- `activated_at` closes that hole. dynreg installs new rows with NULL =
-- pending; the OIDC login + launch validators filter on `activated_at IS
-- NOT NULL`, so a pending row can't authenticate anything. A site
-- integrator reviews + activates pending rows via the admin UI.
--
-- Manually-created platforms (the existing admin-UI form path) and all
-- pre-migration rows are activated immediately to preserve current
-- behaviour: legacy installs continue to work, only dynreg installs
-- require the approval click.

ALTER TABLE lti_platforms
    ADD COLUMN activated_at TIMESTAMPTZ;

-- Existing rows were all created via the admin UI (dynreg didn't exist
-- yet) and should remain active.
UPDATE lti_platforms SET activated_at = created_at WHERE activated_at IS NULL;

CREATE INDEX idx_lti_platforms_activated_at
    ON lti_platforms (activated_at)
    WHERE activated_at IS NOT NULL;
