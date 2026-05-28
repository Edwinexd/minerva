-- Normalize Daisy linkage: one Minerva course can map to many Daisy
-- offerings.
--
-- The original auto-import (20260527000004) stored a single Daisy
-- momenttillfID inline on `courses` with a partial UNIQUE index. That
-- shape can't express "this one Minerva course is the same content
-- delivered as both a 7.5 and a 15 ECTS offering" (two distinct Daisy
-- momenttillfIDs), which is exactly what the admin course-merge needs:
-- after merging two Daisy courses, BOTH offerings must keep feeding the
-- surviving course on every nightly sync. With the inline column the
-- archived shell would still own its momenttillfID (the unique index
-- has no `active` filter), so the next sync would re-feed the dead
-- course and the merge would silently unravel.
--
-- New shape: `course_daisy_offerings` is a child table keyed on
-- momenttillfID (still globally unique: one Daisy offering maps to
-- exactly one Minerva course), pointing at its course. The Daisy sync
-- matches on this table; the merge just re-points a source's offering
-- rows at the survivor. `courses.auto_managed` stays (course-level
-- origin flag); `courses.semester_label` and `courses.course_code`
-- stay as denormalized display/grouping defaults stamped at creation.

CREATE TABLE course_daisy_offerings (
    -- Daisy momenttillfID, e.g. '7620'. One offering -> one course.
    momenttillf_id  TEXT PRIMARY KEY,
    course_id       UUID NOT NULL REFERENCES courses(id) ON DELETE CASCADE,
    -- Per-offering Daisy metadata (beteckning, Swedish name, semester,
    -- public info / syllabus URLs, owning unit). Mirrors the columns
    -- the sync used to write inline; kept per-offering so a multi-link
    -- course can surface each offering's details in the UI.
    course_code     TEXT,
    name            TEXT,
    semester_label  TEXT,
    info_url        TEXT,
    syllabus_url    TEXT,
    unit            TEXT,
    last_synced_at  TIMESTAMPTZ,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_course_daisy_offerings_course
    ON course_daisy_offerings (course_id);

-- Backfill from the inline columns before dropping them. Every course
-- that currently carries a momenttillfID becomes a single offering row;
-- the offering name seeds from the course name (best available source).
INSERT INTO course_daisy_offerings
    (momenttillf_id, course_id, course_code, name, semester_label,
     info_url, syllabus_url, unit, last_synced_at, created_at)
SELECT daisy_momenttillf_id, id, course_code, name, semester_label,
       daisy_info_url, daisy_syllabus_url, daisy_unit,
       daisy_last_synced_at, created_at
FROM courses
WHERE daisy_momenttillf_id IS NOT NULL;

-- Drop the inline identity columns. The partial UNIQUE index on
-- daisy_momenttillf_id is dropped automatically with its column.
ALTER TABLE courses
    DROP COLUMN daisy_momenttillf_id,
    DROP COLUMN daisy_info_url,
    DROP COLUMN daisy_syllabus_url,
    DROP COLUMN daisy_unit,
    DROP COLUMN daisy_last_synced_at;
