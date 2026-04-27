-- Site-level integration keys. Admin-minted; authorize the Moodle plugin to
-- *provision* per-course api_keys on behalf of specific users (identified by
-- eppn at call time). The site key itself cannot access course data; it can
-- only list a user's teachable courses and mint regular api_keys scoped to one
-- of them. That preserves the existing per-course audit trail while removing
-- the manual copy/paste at link time.
CREATE TABLE site_integration_keys (
    id UUID PRIMARY KEY,
    name TEXT NOT NULL,
    key_hash TEXT NOT NULL UNIQUE,
    key_prefix TEXT NOT NULL,
    created_by UUID NOT NULL REFERENCES users(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    last_used_at TIMESTAMPTZ
);

CREATE INDEX idx_site_integration_keys_hash ON site_integration_keys(key_hash);
