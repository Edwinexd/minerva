-- API keys for external integrations (e.g. Moodle plugin).
-- Each key is scoped to a single course and created by the course teacher.
CREATE TABLE api_keys (
    id UUID PRIMARY KEY,
    course_id UUID NOT NULL REFERENCES courses(id) ON DELETE CASCADE,
    created_by UUID NOT NULL REFERENCES users(id),
    name TEXT NOT NULL,
    key_hash TEXT NOT NULL,
    key_prefix TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    last_used_at TIMESTAMPTZ
);

CREATE INDEX idx_api_keys_course ON api_keys(course_id);
CREATE INDEX idx_api_keys_hash ON api_keys(key_hash);
