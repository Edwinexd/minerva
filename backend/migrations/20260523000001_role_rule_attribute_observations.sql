-- Per-(attribute, value, user) observations of every Shibboleth attribute the
-- role-rule engine reads. Lets the admin UI suggest concrete values for each
-- attribute when authoring conditions (only suggestions seen on >= 2 distinct
-- users are surfaced; rarer values stay free-text-only to avoid leaking
-- identifying singletons to admins who didn't look them up themselves).
--
-- Refreshed on every login by the auth middleware: each known attribute is
-- split on `;` (the multi-value Shib delimiter) and each atomic value upserts
-- a row with `last_seen = NOW()`. ON DELETE CASCADE on the user keeps the
-- table tidy when admins delete users.
--
-- GDPR retention: a background sweep in the app prunes rows whose
-- `last_seen` is older than `OBSERVATION_TTL_DAYS` (currently 7 days, see
-- `queries::role_rule_attribute_observations::OBSERVATION_TTL_DAYS`). The
-- data is reconstructible from the next login, so this is purely a privacy
-- floor on how long inactive users' attribute values may sit in the DB.
CREATE TABLE role_rule_attribute_observations (
    attribute TEXT NOT NULL,
    value TEXT NOT NULL,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    first_seen TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    last_seen TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (attribute, value, user_id)
);

-- The suggestion query groups by attribute and counts DISTINCT users; this
-- index keeps the per-attribute scan tight even once the table grows.
CREATE INDEX role_rule_attribute_observations_attribute_idx
    ON role_rule_attribute_observations(attribute);

-- The TTL prune sweep runs `WHERE last_seen < NOW() - INTERVAL '7 days'`
-- every 6 hours; this index turns it into a range scan over the (small)
-- aged-out tail instead of a full-table seq scan once the table grows.
-- Cheap to maintain because last_seen only moves forward via UPSERT, never
-- backwards or sideways.
CREATE INDEX role_rule_attribute_observations_last_seen_idx
    ON role_rule_attribute_observations(last_seen);

-- Seed observations from the data we already have on hand so the UI has
-- something to suggest immediately, without waiting for every user to log
-- back in. eppn is a singleton per user (won't pass the >= 2 threshold,
-- harmless to insert); displayName can already hit the threshold if two
-- users happen to share one. Other attributes only populate on next login.
INSERT INTO role_rule_attribute_observations (attribute, value, user_id)
SELECT 'eppn', eppn, id FROM users WHERE eppn IS NOT NULL AND eppn <> ''
ON CONFLICT DO NOTHING;

INSERT INTO role_rule_attribute_observations (attribute, value, user_id)
SELECT 'displayName', display_name, id
FROM users
WHERE display_name IS NOT NULL AND display_name <> ''
ON CONFLICT DO NOTHING;
