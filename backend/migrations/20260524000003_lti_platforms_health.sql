-- Per-platform health bookkeeping for the orphan-LMS sweep.
--
-- LTI 1.3 doesn't notify the tool when the platform deletes a tool
-- registration on its side, but a probe call to the platform's
-- token endpoint with our client_credentials JWT will come back as
-- `invalid_client` once the platform has dropped us. Worker probes
-- every active platform daily; rows where the platform has been
-- continuously rejecting us for 30+ days are cascade-deleted.
--
-- Schema:
--   last_health_check_at     timestamp of most recent probe (any result)
--   last_health_check_status `ok` | `invalid_client` | `http_<code>` |
--                            `network` | `parse_error` (free-form, the
--                            UI just buckets on `ok` vs `invalid_client`
--                            vs other)
--   invalid_client_since     first probe after the most recent `ok` that
--                            came back `invalid_client`. NULL while the
--                            platform is healthy OR while we've never
--                            seen `invalid_client`. Resets to NULL on any
--                            `ok`. Transient errors (network, 5xx) do
--                            NOT touch it so a flaky LMS can't trigger
--                            the auto-delete; only an explicit reject
--                            does.

ALTER TABLE lti_platforms
    ADD COLUMN last_health_check_at TIMESTAMPTZ,
    ADD COLUMN last_health_check_status TEXT,
    ADD COLUMN invalid_client_since TIMESTAMPTZ;

CREATE INDEX idx_lti_platforms_invalid_client_since
    ON lti_platforms (invalid_client_since)
    WHERE invalid_client_since IS NOT NULL;
