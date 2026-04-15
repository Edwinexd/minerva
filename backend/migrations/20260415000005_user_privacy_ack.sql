-- Track when a user acknowledged the in-app data-handling disclosure.
-- NULL = not yet acknowledged. Students with NULL are blocked from sending
-- chat messages until they agree; reading existing conversations still works.
ALTER TABLE users ADD COLUMN privacy_acknowledged_at TIMESTAMPTZ;
