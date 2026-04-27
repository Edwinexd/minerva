-- Cached catalog of play.dsv.su.se course designations.
-- Pushed by the transcript pipeline (which has SU auth) and read by the
-- frontend to provide autocomplete suggestions when configuring a course's
-- watched designations. The catalog is best-effort; free-text designation
-- entry is still accepted since this cache may not be exhaustive.
CREATE TABLE play_course_catalog (
    code TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
