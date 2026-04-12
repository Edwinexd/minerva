-- Play.dsv.su.se course designations watched by a Minerva course.
-- Each designation (e.g. 'PROG1', 'IDSV') is periodically scanned by the
-- transcript pipeline to discover and auto-ingest new presentations.
CREATE TABLE play_designations (
    id UUID PRIMARY KEY,
    course_id UUID NOT NULL REFERENCES courses(id) ON DELETE CASCADE,
    designation TEXT NOT NULL,
    added_by UUID NOT NULL REFERENCES users(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    last_synced_at TIMESTAMPTZ,
    last_error TEXT,
    UNIQUE (course_id, designation)
);

CREATE INDEX idx_play_designations_course ON play_designations(course_id);
