-- Course syllabi (kursplaner) from SU's public syllabus archive.
--
-- Daisy links every offering to a planarkiv page per Ladok course code. The
-- nightly Daisy workflow resolves each code through dsv-wrapper's
-- `SyllabusClient` and PUTs the full listing to
-- /api/service/syllabus-plans/{code}; the scheduler then downloads the PDFs.
--
-- This is global reference data, not course material: rows are keyed by
-- Ladok code, independent of any Minerva course.

CREATE TABLE syllabus_plans (
    -- Ladok code, e.g. 'IB440C'. Not the Daisy designation ('PROG2').
    code TEXT PRIMARY KEY,
    name_sv TEXT NOT NULL,
    name_en TEXT,
    -- 'course' or 'programme' as reported by the archive.
    education_type TEXT NOT NULL,
    ects REAL,
    -- One line: kind, level, subject, department.
    description TEXT,
    is_discontinued BOOLEAN NOT NULL DEFAULT FALSE,
    -- Last sync that listed this code. Codes stop being listed when no
    -- current Daisy offering references them; the row is kept.
    last_listed_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- One published PDF. A version is valid from its term until a newer one
-- for the same code replaces it. The archive offers a Swedish PDF for every
-- version and an English one for some, each its own row.
CREATE TABLE syllabus_versions (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    code TEXT NOT NULL REFERENCES syllabus_plans(code) ON DELETE CASCADE,
    -- 'HT2020'; `valid_from_term_id` is Daisy's numeric form (20202) and
    -- what "latest" orders by.
    valid_from TEXT NOT NULL,
    valid_from_term_id INTEGER NOT NULL,
    language TEXT NOT NULL CHECK (language IN ('sv', 'en')),
    pdf_url TEXT NOT NULL UNIQUE,
    title TEXT NOT NULL,
    -- SHA-256 of the last downloaded PDF; NULL until the first download.
    -- The file lives at {docs_path}/syllabi/{content_hash}.pdf, so a PDF
    -- re-published under the same URL keeps the previous bytes on disk.
    content_hash TEXT,
    size_bytes BIGINT,
    -- When the bytes behind the URL last changed (first download included).
    -- Downstream indexing re-processes a version when this moves.
    content_changed_at TIMESTAMPTZ,
    last_checked_at TIMESTAMPTZ,
    -- The archive gives no ETag/Last-Modified, so unchanged-looking
    -- versions are re-downloaded on a schedule to catch silent revisions.
    next_check_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    fetch_error TEXT,
    fetch_failures INTEGER NOT NULL DEFAULT 0,
    -- Set when a sync no longer lists the version; cleared if it returns.
    delisted_at TIMESTAMPTZ,
    first_seen_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (code, valid_from, language)
);

CREATE INDEX idx_syllabus_versions_due
    ON syllabus_versions (next_check_at)
    WHERE delisted_at IS NULL;

-- Which Daisy offerings belong to which plan code (an offering can belong
-- to several). Keyed by momenttillf_id without a foreign key: with Daisy
-- auto-apply off the offering may still sit in staging.
CREATE TABLE syllabus_plan_offerings (
    code TEXT NOT NULL REFERENCES syllabus_plans(code) ON DELETE CASCADE,
    momenttillf_id TEXT NOT NULL,
    -- 'obligatorisk' / 'valbar' etc. from the Daisy course page.
    requirement TEXT,
    synced_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (code, momenttillf_id)
);

CREATE INDEX idx_syllabus_plan_offerings_offering
    ON syllabus_plan_offerings (momenttillf_id);
