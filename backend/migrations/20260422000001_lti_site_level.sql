-- Site-level LTI 1.3 platforms. Admin-managed; one per Moodle/Canvas instance.
-- A platform is NOT tied to a single Minerva course; instead teachers bind
-- each Moodle context (course) to one of their Minerva courses on first launch.
--
-- Per-course registrations (lti_registrations) remain for backwards compatibility.
-- The launch flow checks lti_registrations first by (issuer, client_id); if no
-- per-course registration exists, it falls back to lti_platforms.
CREATE TABLE lti_platforms (
    id UUID PRIMARY KEY,
    name TEXT NOT NULL,
    issuer TEXT NOT NULL,
    client_id TEXT NOT NULL,
    deployment_id TEXT,
    auth_login_url TEXT NOT NULL,
    auth_token_url TEXT NOT NULL,
    platform_jwks_url TEXT NOT NULL,
    created_by UUID NOT NULL REFERENCES users(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (issuer, client_id)
);

CREATE INDEX idx_lti_platforms_issuer ON lti_platforms(issuer, client_id);

-- Binding between a Moodle/Canvas course (context) and a Minerva course, scoped
-- to a specific LTI platform. Created on first launch when a teacher picks
-- which Minerva course to link.
CREATE TABLE lti_course_bindings (
    id UUID PRIMARY KEY,
    platform_id UUID NOT NULL REFERENCES lti_platforms(id) ON DELETE CASCADE,
    context_id TEXT NOT NULL,
    context_label TEXT,
    context_title TEXT,
    course_id UUID NOT NULL REFERENCES courses(id) ON DELETE CASCADE,
    created_by UUID NOT NULL REFERENCES users(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (platform_id, context_id)
);

CREATE INDEX idx_lti_course_bindings_platform_context ON lti_course_bindings(platform_id, context_id);
CREATE INDEX idx_lti_course_bindings_course ON lti_course_bindings(course_id);

-- lti_launches previously FK'd to lti_registrations only. Platform launches
-- need their own reference path; exactly one of (registration_id, platform_id)
-- is set per row.
ALTER TABLE lti_launches ALTER COLUMN registration_id DROP NOT NULL;
ALTER TABLE lti_launches ADD COLUMN platform_id UUID REFERENCES lti_platforms(id) ON DELETE CASCADE;
ALTER TABLE lti_launches ADD CONSTRAINT lti_launches_source_exclusive
    CHECK ((registration_id IS NOT NULL) <> (platform_id IS NOT NULL));
