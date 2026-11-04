-- OAuth 2.1 authorization server for Minerva's MCP endpoint (`/mcp`), per
-- the MCP 2026-07-28 authorization spec. Minerva is both the authorization
-- server and the resource server: staff sign in through Shibboleth at
-- `/api/oauth/authorize`, approve the client on a consent page, and the
-- client receives tokens bound to `{base}/mcp`.
--
-- Tokens are opaque random strings; only their SHA-256 is stored.

-- Clients are public (no secret, PKCE only). Either a Client ID Metadata
-- Document (client_id is the HTTPS URL of the document, fetched and cached)
-- or, for older clients, dynamic registration (RFC 7591, client_id minted
-- here).
CREATE TABLE oauth_clients (
    client_id TEXT PRIMARY KEY,
    client_name TEXT NOT NULL,
    redirect_uris TEXT[] NOT NULL,
    registration TEXT NOT NULL
        CONSTRAINT oauth_clients_registration_check CHECK (registration IN ('cimd', 'dcr')),
    -- When a metadata document was last fetched; refetched when stale.
    fetched_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- An authorization request waiting for the user's decision on the consent
-- page. Short-lived; the id travels through the browser.
CREATE TABLE oauth_pending_authorizations (
    id UUID PRIMARY KEY,
    client_id TEXT NOT NULL REFERENCES oauth_clients(client_id) ON DELETE CASCADE,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    redirect_uri TEXT NOT NULL,
    state TEXT,
    code_challenge TEXT NOT NULL,
    scope TEXT NOT NULL,
    resource TEXT NOT NULL,
    expires_at TIMESTAMPTZ NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- One row per (user, client) the user approved: what "connected apps" lists
-- and what revoking removes. Tokens hang off it.
CREATE TABLE oauth_grants (
    id UUID PRIMARY KEY,
    client_id TEXT NOT NULL REFERENCES oauth_clients(client_id) ON DELETE CASCADE,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    scope TEXT NOT NULL,
    resource TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    last_used_at TIMESTAMPTZ,
    revoked_at TIMESTAMPTZ
);

CREATE INDEX idx_oauth_grants_user ON oauth_grants (user_id) WHERE revoked_at IS NULL;

-- Authorization codes are one-time and short-lived; `consumed_at` makes a
-- replay detectable (OAuth 2.1 says to revoke the grant then).
CREATE TABLE oauth_authorization_codes (
    code_hash TEXT PRIMARY KEY,
    grant_id UUID NOT NULL REFERENCES oauth_grants(id) ON DELETE CASCADE,
    redirect_uri TEXT NOT NULL,
    code_challenge TEXT NOT NULL,
    expires_at TIMESTAMPTZ NOT NULL,
    consumed_at TIMESTAMPTZ
);

-- Access and refresh tokens. Refresh tokens rotate: using one marks it
-- `rotated_at` and issues a new pair; presenting a rotated one again is a
-- replay and revokes the whole grant.
CREATE TABLE oauth_tokens (
    token_hash TEXT PRIMARY KEY,
    grant_id UUID NOT NULL REFERENCES oauth_grants(id) ON DELETE CASCADE,
    kind TEXT NOT NULL CONSTRAINT oauth_tokens_kind_check CHECK (kind IN ('access', 'refresh')),
    expires_at TIMESTAMPTZ NOT NULL,
    rotated_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_oauth_tokens_grant ON oauth_tokens (grant_id);
CREATE INDEX idx_oauth_tokens_expiry ON oauth_tokens (expires_at);
