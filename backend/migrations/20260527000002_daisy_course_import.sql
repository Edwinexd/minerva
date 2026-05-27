-- Auto-import courses from Daisy.
--
-- The transcript pipeline (scripts/fetch_transcripts.py) gains a new phase
-- that enumerates DSV course offerings per semester via dsv-wrapper's new
-- `daisy.get_courses(semester)` + `daisy.get_course_participants(...)` APIs
-- and pushes them to a new service endpoint. The endpoint upserts a Minerva
-- course per Daisy `momenttillfID`, registers the course code (`beteckning`)
-- as a play designation so transcript discovery flows in, and additively
-- adds resolved staff as course members.
--
-- Semester scope is automatic: the python phase derives the current and
-- next semester from today's date (VT = Jan-Jul, HT = Aug-Dec, per Daisy's
-- conventional split). No admin watchlist; new term auto-onboards once the
-- date rolls into it.

-- Daisy linkage + per-semester grouping on courses.
ALTER TABLE courses
    -- Free-text 'VT2026' / 'HT2025'. Mirrors `Semester.label` from
    -- dsv-wrapper. Drives the per-semester grouping on the My Courses
    -- page. NULL for ad-hoc / non-auto-imported courses.
    ADD COLUMN semester_label TEXT,
    -- Daisy momenttillfID (e.g. '7620'). Primary dedup key for the
    -- import phase; we use a partial UNIQUE index so non-imported
    -- courses (NULL) don't collide with each other.
    ADD COLUMN daisy_momenttillf_id TEXT,
    -- Public Daisy info URL for the offering (momentinfo.Momentinfo).
    ADD COLUMN daisy_info_url TEXT,
    -- External syllabus URL (utbildning.su.se planarkiv). Only set
    -- after the detail-page fetch (`get_course`).
    ADD COLUMN daisy_syllabus_url TEXT,
    -- Owning unit at DSV, e.g. 'ACT'. Detail-page only.
    ADD COLUMN daisy_unit TEXT,
    -- Wall-clock of the most recent successful Daisy import run that
    -- touched this row. Used by the admin UI to surface stale courses.
    ADD COLUMN daisy_last_synced_at TIMESTAMPTZ,
    -- TRUE when the course was created by the Daisy auto-import phase.
    -- Membership sync stays additive on these (never demote/remove);
    -- owner may be swapped from the env-var fallback to a real
    -- kursansvarig the first time one is identified, but never
    -- replaced after that.
    ADD COLUMN auto_managed BOOLEAN NOT NULL DEFAULT FALSE;

CREATE UNIQUE INDEX idx_courses_daisy_momenttillf_id
    ON courses (daisy_momenttillf_id)
    WHERE daisy_momenttillf_id IS NOT NULL;

CREATE INDEX idx_courses_semester_label
    ON courses (semester_label)
    WHERE semester_label IS NOT NULL;

-- EPPN aliases.
--
-- A single Daisy staff person can have multiple SU usernames over their
-- career (system migrations, name changes, multiple roles). The Daisy
-- staff profile lists every login they've held. We treat eppn as the
-- promotion target: the most recently observed login becomes the primary
-- `users.eppn`; older logins live here as aliases. On every successful
-- auth, the inbound eppn is matched against primary first then aliases;
-- an alias hit swaps the rows (demote previous primary, promote the
-- alias) and bumps `last_seen_at`.
CREATE TABLE user_eppn_aliases (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    -- Lowercased, same normalization rules as users.eppn (see
    -- auth_middleware). UNIQUE across the whole table; the primary
    -- users.eppn is implicitly part of the same namespace.
    eppn TEXT NOT NULL UNIQUE,
    -- Last time we saw this alias either via Daisy staff usernames or
    -- via an inbound auth header. Drives the "promote most-recent" rule.
    last_seen_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_user_eppn_aliases_user_id ON user_eppn_aliases (user_id);

-- Pending course memberships.
--
-- When the Daisy import phase identifies a staff person we can't yet
-- match to a Minerva user (they've never logged in), we record their
-- eppn here. The auth middleware drains this table on every login,
-- so the first time the user authenticates they immediately land in
-- every course they should have been in.
CREATE TABLE pending_course_memberships (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    course_id UUID NOT NULL REFERENCES courses(id) ON DELETE CASCADE,
    -- Lowercased; same normalization as users.eppn.
    eppn TEXT NOT NULL,
    display_name TEXT,
    -- 'teacher' | 'ta'. Mirrors course_members.role; students are
    -- never auto-added by the Daisy phase (no roster sync today).
    role TEXT NOT NULL CHECK (role IN ('teacher', 'ta')),
    -- TRUE when this person was identified as a Kurs-/delkursansvarig
    -- with a staff (not student) profile. On drain, if the course
    -- currently sits on the env-var fallback owner, the first such
    -- person to log in is promoted to owner. After ownership is
    -- handed to a real human, additional eligible_for_owner=TRUE
    -- rows still convert to teacher memberships but never re-swap
    -- ownership.
    eligible_for_owner BOOLEAN NOT NULL DEFAULT FALSE,
    -- Free-text role labels from Daisy (Kurs-/delkursansvarig,
    -- Examination, Handledare, Laborationsledare, Administration).
    -- Stored for audit / debugging; the Minerva role is in `role`.
    daisy_roles TEXT[] NOT NULL DEFAULT '{}',
    -- Last Daisy momenttillfID that re-registered this pending row.
    -- Updated on every additive sync so we can detect dead pendings
    -- (course offering no longer in Daisy => safe to GC, though we
    -- never need to since draining on login handles cleanup).
    daisy_momenttillf_id TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),

    -- One pending row per (course, eppn). A second sync with the same
    -- pair becomes an UPDATE (refresh daisy_roles, bump updated_at).
    UNIQUE (course_id, eppn)
);

CREATE INDEX idx_pending_course_memberships_eppn
    ON pending_course_memberships (eppn);
