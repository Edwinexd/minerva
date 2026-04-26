-- Feature flags: admin-managed gates for opt-in features.
--
-- Three scopes, picked at insert time:
--   * Course-scoped:  course_id IS NOT NULL, user_id IS NULL
--   * User-scoped:    user_id   IS NOT NULL, course_id IS NULL
--   * Global:         both NULL
--
-- The application's resolution order for "is flag X enabled here?":
--   1. course_id row if a course context exists
--   2. user_id row if a user context exists
--   3. global row
--   4. compiled-in default (typically false for opt-in features)
--
-- The CHECK ensures a row never claims to be both course- and
-- user-scoped at once. The three partial unique indexes ensure a
-- given (flag, scope) combination has at most one row.
--
-- Cleanup is automatic via FK ON DELETE CASCADE: deleting a course
-- removes its flag rows; same for users.
--
-- Why a single table rather than three: the management story (admin
-- UI, audit log, "list all flags currently set anywhere") is much
-- simpler with one table, and the partial-index trick gives us the
-- per-scope uniqueness we want without paying for separate tables.

CREATE TABLE feature_flags (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    flag        TEXT NOT NULL,
    course_id   UUID REFERENCES courses(id) ON DELETE CASCADE,
    user_id     UUID REFERENCES users(id) ON DELETE CASCADE,
    enabled     BOOLEAN NOT NULL DEFAULT TRUE,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CHECK (NOT (course_id IS NOT NULL AND user_id IS NOT NULL))
);

CREATE UNIQUE INDEX feature_flags_course_idx
    ON feature_flags (flag, course_id)
    WHERE course_id IS NOT NULL;

CREATE UNIQUE INDEX feature_flags_user_idx
    ON feature_flags (flag, user_id)
    WHERE user_id IS NOT NULL;

CREATE UNIQUE INDEX feature_flags_global_idx
    ON feature_flags (flag)
    WHERE course_id IS NULL AND user_id IS NULL;

CREATE INDEX feature_flags_flag_idx ON feature_flags (flag);
