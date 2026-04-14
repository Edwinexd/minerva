-- Attribute-based role rules: auto-promote users to a target role at login
-- based on AND-composed conditions over Shibboleth attributes.

CREATE TABLE role_rules (
    id UUID PRIMARY KEY,
    name TEXT NOT NULL,
    target_role TEXT NOT NULL,
    enabled BOOLEAN NOT NULL DEFAULT TRUE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE role_rule_conditions (
    id UUID PRIMARY KEY,
    rule_id UUID NOT NULL REFERENCES role_rules(id) ON DELETE CASCADE,
    attribute TEXT NOT NULL,
    operator TEXT NOT NULL,
    value TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX role_rule_conditions_rule_id_idx ON role_rule_conditions(rule_id);

-- Admin-set role stays sticky; rules never touch locked users (except for
-- admins, whose role is still hard-overridden by MINERVA_ADMINS env).
ALTER TABLE users ADD COLUMN role_manually_set BOOLEAN NOT NULL DEFAULT FALSE;

-- Per-course-owner aggregate daily token cap. Sums tokens across every
-- course the user owns. 0 = unlimited (preserves existing users).
ALTER TABLE users ADD COLUMN owner_daily_token_limit BIGINT NOT NULL DEFAULT 0;
