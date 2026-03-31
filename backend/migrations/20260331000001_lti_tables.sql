-- LTI 1.3 registrations, scoped to a Minerva course.
-- A teacher registers their Moodle course's LTI connection here.
CREATE TABLE lti_registrations (
    id UUID PRIMARY KEY,
    course_id UUID NOT NULL REFERENCES courses(id) ON DELETE CASCADE,
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

-- Ephemeral OIDC state for LTI 1.3 login flow (cleaned up on use / expiry)
CREATE TABLE lti_launches (
    id UUID PRIMARY KEY,
    state TEXT NOT NULL UNIQUE,
    nonce TEXT NOT NULL,
    registration_id UUID NOT NULL REFERENCES lti_registrations(id) ON DELETE CASCADE,
    target_link_uri TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    expires_at TIMESTAMPTZ NOT NULL DEFAULT NOW() + INTERVAL '10 minutes'
);

CREATE INDEX idx_lti_registrations_issuer ON lti_registrations(issuer, client_id);
CREATE INDEX idx_lti_registrations_course ON lti_registrations(course_id);
CREATE INDEX idx_lti_launches_state ON lti_launches(state);
CREATE INDEX idx_lti_launches_expires ON lti_launches(expires_at);
