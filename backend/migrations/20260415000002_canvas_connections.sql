-- Canvas LMS connections for course resource syncing.
-- Teachers link a Minerva course to a Canvas course and Minerva pulls
-- files via the Canvas REST API.

CREATE TABLE canvas_connections (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    course_id       UUID NOT NULL REFERENCES courses(id) ON DELETE CASCADE,
    name            TEXT NOT NULL,
    -- e.g. https://canvas.instructure.com
    canvas_base_url TEXT NOT NULL,
    -- Canvas personal access token
    canvas_api_token TEXT NOT NULL,
    -- Canvas course ID (numeric string)
    canvas_course_id TEXT NOT NULL,
    auto_sync       BOOLEAN NOT NULL DEFAULT false,
    created_by      UUID NOT NULL REFERENCES users(id),
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    last_synced_at  TIMESTAMPTZ,
    UNIQUE (course_id, canvas_course_id)
);

-- Track which Canvas files have already been synced to avoid duplicates.
CREATE TABLE canvas_sync_log (
    id                  UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    connection_id       UUID NOT NULL REFERENCES canvas_connections(id) ON DELETE CASCADE,
    -- Canvas file ID
    canvas_file_id      TEXT NOT NULL,
    filename            TEXT NOT NULL,
    content_type        TEXT,
    minerva_document_id UUID REFERENCES documents(id) ON DELETE SET NULL,
    synced_at           TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (connection_id, canvas_file_id)
);
