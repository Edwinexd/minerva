-- Admin-tunable system-wide defaults.
--
-- Replaces the previous pattern of "env var or hard-coded constant
-- means a redeploy to tune" with a database row the admin can edit
-- through the UI live. Each row is one configurable default; the
-- registry of supported keys + their JSON type + min/max/enum + the
-- env-var/hard-coded fallback used to seed the row lives in code at
-- `backend/crates/minerva-server/src/system_defaults.rs`. The DB
-- stores values; the registry interprets them.
--
-- Two categories of keys land here:
--
--   1. New-course AI defaults. Today the `courses` table column
--      DEFAULTs supply these (e.g. `temperature DEFAULT 0.3`). After
--      this change the `routes::courses::create_course` handler
--      reads each value from `system_defaults` and snapshots it into
--      the new course row. Existing courses are unaffected; teachers
--      keep their per-course overrides. To "apply defaults to all
--      courses" would be a separate, deliberate admin action.
--
--   2. Platform-wide policy values that the code used to read from
--      env vars or `pub const`s: per-owner daily token cap, upload
--      byte caps, Canvas/LTI sync interval hours, rule-attribute
--      observation TTL. The env vars still work as the *seed* for
--      a fresh deployment; once the row exists, the DB wins.
--
-- Why a single key/value table rather than typed columns: 17 knobs
-- today and the list will keep growing. A key/value table lets us
-- add a new knob with a single line in the registry, no migration.
-- The cost is that the SQL has no type-checking; the registry's
-- typed accessors absorb that on the Rust side.
--
-- Why JSONB rather than TEXT: lets us store booleans, ints, floats,
-- and (future) compound values without a parse step at every read.
-- The registry encodes/decodes through serde_json; an integer-typed
-- knob stored as `100000` round-trips without precision loss, and a
-- bool knob is `true`/`false`, not `"true"`.

CREATE TABLE system_defaults (
    key        TEXT PRIMARY KEY,
    value      JSONB NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- No seed data. The startup sync in `AppState::new` inserts every
-- registered key from its env-var (if set) or hard-coded fallback on
-- first boot. Keeping the seed in code (rather than splitting it
-- across migration + code) means there's one source of truth for
-- "what's the fallback for knob X".
