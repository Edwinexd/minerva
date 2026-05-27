-- Admin-gated staging area for Daisy auto-imports.
--
-- Previously, the daily sync wrote courses straight into the
-- `courses` table. That worked but gave admins no review surface
-- before the live UI started showing rows; a misconfigured sync
-- could mass-create courses that then need mass-archiving. New
-- shape: every Daisy course offering the sync sees lands in
-- `daisy_pending_imports` first; an admin reviews + clicks Apply
-- to promote selected rows into `courses`. Once the workflow is
-- trusted, the singleton `daisy_settings.auto_apply` flag flips ON
-- and the sync bypasses staging.

CREATE TABLE daisy_pending_imports (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    -- Dedup key. Daily syncs upsert here; an existing pending row
    -- gets refreshed in place rather than piling up duplicates.
    momenttillf_id TEXT NOT NULL UNIQUE,
    -- Snapshot of what an apply would write. Mirrors the columns
    -- on `courses` that the Daisy importer sets.
    course_code TEXT NOT NULL,
    name TEXT NOT NULL,
    semester_label TEXT NOT NULL,
    daisy_info_url TEXT,
    daisy_syllabus_url TEXT,
    daisy_unit TEXT,
    -- Frozen participant list at sync time. Each element is a
    -- `{eppns, display_name, daisy_roles, kind, person_id}` object
    -- matching the wire shape the python script emits. Stored as
    -- JSONB so admin queries can drill into it without joining
    -- another table.
    participants JSONB NOT NULL DEFAULT '[]'::jsonb,
    -- NULL when this is a brand-new course offering (`Apply` will
    -- INSERT). Set to the existing `courses.id` when the offering
    -- already lives in Minerva (`Apply` will refresh metadata +
    -- additively sync members). Frontend uses this to render the
    -- "New" vs "Update" badge.
    existing_course_id UUID REFERENCES courses(id) ON DELETE SET NULL,
    -- First time we ever saw this momenttillf_id this run. Stays
    -- stable across re-stages.
    first_seen_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    -- Most recent sync that touched this row. Updated on every
    -- re-stage so an admin can spot stale pendings (Daisy stopped
    -- listing the offering = sync stopped refreshing the row).
    last_seen_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_daisy_pending_imports_semester
    ON daisy_pending_imports (semester_label);

-- Singleton settings row. The CHECK pins it to id=1 so a future
-- bug can't accidentally insert multiple rows; a follow-up
-- migration that needs additional knobs just ALTERs this table.
CREATE TABLE daisy_settings (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    -- When TRUE, the sync skips `daisy_pending_imports` and writes
    -- straight to `courses` (original v1 behaviour). When FALSE
    -- (default), the sync only stages; an admin promotes.
    auto_apply BOOLEAN NOT NULL DEFAULT FALSE,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    -- Admin who last flipped the toggle; nullable because the
    -- initial row is system-seeded.
    updated_by UUID REFERENCES users(id) ON DELETE SET NULL
);

INSERT INTO daisy_settings (id, auto_apply) VALUES (1, FALSE);
