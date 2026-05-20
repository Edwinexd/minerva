-- LTI Advantage Names and Role Provisioning Service (NRPS) support.
--
-- NRPS lets the tool *pull* a context's roster from the platform (the
-- `context_memberships_url` advertised in the launch JWT) and reconcile it
-- against Minerva course membership. Each member carries a `status`
-- (Active / Inactive / Deleted), which is what enables removal of users
-- who have left the LMS course; the capability the original review comment
-- was pointing at.
--
-- Two launch sources exist and BOTH support NRPS:
--   * per-course registrations (lti_registrations, 1:1 with a Minerva course)
--   * site-level platforms     (lti_platforms + lti_course_bindings)
-- We mirror the lti_launches pattern: exactly one of (registration_id,
-- platform_id) is set per row.

CREATE TABLE lti_nrps_contexts (
    id UUID PRIMARY KEY,
    -- Exactly one of these is set (see CHECK below), mirroring lti_launches.
    registration_id UUID REFERENCES lti_registrations(id) ON DELETE CASCADE,
    platform_id UUID REFERENCES lti_platforms(id) ON DELETE CASCADE,
    -- The LMS context (course) id from the launch `context` claim. For a
    -- per-course registration this is usually a single value; for a
    -- site-level platform it pairs with the binding's context.
    context_id TEXT NOT NULL,
    -- Minerva course this roster reconciles into.
    course_id UUID NOT NULL REFERENCES courses(id) ON DELETE CASCADE,
    -- The NRPS membership endpoint advertised in the launch JWT's
    -- `...lti-nrps/claim/namesroleservice.context_memberships_url`.
    memberships_url TEXT NOT NULL,
    -- Sync bookkeeping, surfaced read-only in the LTI UI.
    last_sync_at TIMESTAMPTZ,
    last_sync_status TEXT,           -- 'ok' | 'error'
    last_sync_error TEXT,
    last_sync_added INTEGER,
    last_sync_removed INTEGER,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CONSTRAINT lti_nrps_source_exclusive
        CHECK ((registration_id IS NOT NULL) <> (platform_id IS NOT NULL))
);

-- One NRPS context per (source, LMS context). Partial unique indexes
-- because exactly one source FK is non-null per row.
CREATE UNIQUE INDEX uq_lti_nrps_registration_context
    ON lti_nrps_contexts (registration_id, context_id)
    WHERE registration_id IS NOT NULL;
CREATE UNIQUE INDEX uq_lti_nrps_platform_context
    ON lti_nrps_contexts (platform_id, context_id)
    WHERE platform_id IS NOT NULL;

CREATE INDEX idx_lti_nrps_contexts_course ON lti_nrps_contexts (course_id);
CREATE INDEX idx_lti_nrps_contexts_last_sync ON lti_nrps_contexts (last_sync_at);

-- Provenance: which (context, user) memberships were provisioned by NRPS.
-- This is what makes "LTI-sourced only" removal safe; the reconcile loop
-- only ever removes course members it can find a row for here, so
-- Shibboleth direct-login users, manually-added members, and the course
-- owner are never touched.
CREATE TABLE lti_nrps_memberships (
    nrps_context_id UUID NOT NULL REFERENCES lti_nrps_contexts(id) ON DELETE CASCADE,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    -- The platform-side user id (the launch `sub`), kept for audit/debug.
    lti_user_id TEXT NOT NULL,
    -- Last status observed for this member in an NRPS fetch.
    last_status TEXT NOT NULL,       -- 'Active' | 'Inactive' | 'Deleted'
    last_seen_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (nrps_context_id, user_id)
);

CREATE INDEX idx_lti_nrps_memberships_user ON lti_nrps_memberships (user_id);
