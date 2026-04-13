-- External-auth invites: lets admins grant time-limited access to people
-- without Stockholm University Shibboleth accounts. The admin mints a JWT
-- link; the external user clicks it; Apache (via a small Lua subrequest to
-- the verify endpoint) checks signature + this table on every request. The
-- jti column is the unique token identifier embedded in the JWT, and lets
-- us revoke individual invites without rotating the shared HMAC secret.
CREATE TABLE external_auth_invites (
    id UUID PRIMARY KEY,
    jti UUID NOT NULL UNIQUE,
    eppn TEXT NOT NULL,
    display_name TEXT,
    created_by UUID NOT NULL REFERENCES users(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    expires_at TIMESTAMPTZ NOT NULL,
    revoked_at TIMESTAMPTZ
);

CREATE INDEX idx_external_auth_invites_jti ON external_auth_invites(jti);
CREATE INDEX idx_external_auth_invites_eppn ON external_auth_invites(eppn);
