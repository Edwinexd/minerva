-- Role-elevation suggestions for LTI-launched users. Default behaviour is
-- that an LTI launch adds the user as `student` even when the remote LMS
-- claims an instructor role -- trusting cross-system role claims lets any
-- Moodle site admin become a Minerva teacher on any linked course. Instead
-- we record the suggested role here and surface it on the course members
-- tab so an existing course teacher/owner must explicitly approve it.
CREATE TABLE course_member_role_suggestions (
    id UUID PRIMARY KEY,
    course_id UUID NOT NULL REFERENCES courses(id) ON DELETE CASCADE,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    suggested_role TEXT NOT NULL,
    source TEXT NOT NULL,
    source_detail JSONB,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    resolved_at TIMESTAMPTZ,
    resolved_by UUID REFERENCES users(id),
    -- NULL while pending; 'approved' or 'declined' once acted on.
    resolution TEXT,
    CHECK (resolution IS NULL OR resolution IN ('approved', 'declined')),
    CHECK ((resolution IS NULL) = (resolved_at IS NULL))
);

-- One active record per (course, user, role): a second LTI launch claiming
-- the same role must not stack duplicate pending rows, and a previous
-- 'declined' row permanently silences that (user, role) pair (approved rows
-- are also unique so we never reopen a resolved suggestion).
CREATE UNIQUE INDEX idx_course_member_role_suggestions_unique
    ON course_member_role_suggestions (course_id, user_id, suggested_role);

CREATE INDEX idx_course_member_role_suggestions_pending
    ON course_member_role_suggestions (course_id)
    WHERE resolution IS NULL;
