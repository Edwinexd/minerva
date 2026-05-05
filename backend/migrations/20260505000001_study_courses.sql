-- Study mode: per-course config row that turns a course into a research
-- evaluation pipeline (consent -> pre-survey -> N tasks -> post-survey ->
-- thank-you + lockout). The runtime gate is the `study_mode` feature flag
-- (see feature_flags table); this row holds the configuration the flag
-- needs once it's on. Existence of the row WITHOUT the flag being on is
-- harmless (config is just dormant), and the flag being on without a row
-- is a misconfiguration that the application surfaces as a 500 (admin
-- shouldn't be able to enable the flag from the UI without first having
-- saved the row).
--
-- `completion_gate_kind` is intentionally a column rather than hardcoded:
-- the user is currently picking "messages_only" (>= 1 user message in the
-- task's conversation, which under forced-on Aegis also implies >= 1
-- Aegis analysis row), but other gate definitions (e.g. requiring an
-- Aegis suggestion click) are easy to add later without a migration --
-- just extend the CHECK list and the gate-evaluation function.
--
-- `consent_html` and `thank_you_html` are researcher-editable raw HTML
-- rendered into the consent and thank-you screens. Sanitisation happens
-- frontend-side at render time (DOMPurify); admins-only can edit, and
-- admins can already inject arbitrary HTML elsewhere in the system, so
-- the trust boundary is unchanged.

CREATE TABLE study_courses (
    course_id            UUID PRIMARY KEY REFERENCES courses(id) ON DELETE CASCADE,
    number_of_tasks      INTEGER NOT NULL CHECK (number_of_tasks > 0),
    completion_gate_kind TEXT NOT NULL DEFAULT 'messages_only'
        CHECK (completion_gate_kind IN ('messages_only')),
    consent_html         TEXT NOT NULL DEFAULT '',
    thank_you_html       TEXT NOT NULL DEFAULT '',
    created_at           TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at           TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
