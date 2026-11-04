-- Credit transfer (tillgodoräknande) workspaces: a staff tool for comparing
-- external syllabuses against DSV course plans. Second workspace subtype
-- after `courses`; see 20261104000002_workspaces.sql for the pattern.
--
-- Access is implicit: every user whose Minerva role ranks at or above
-- `access_min_role` can use the workspace without a membership row, so
-- staff never need enrolling and a demotion takes effect immediately.
-- Explicit `workspace_members` rows still exist for the people who manage
-- it (owner, added teachers) and win over the implicit grant.

CREATE TABLE credit_transfer_workspaces (
    workspace_id UUID PRIMARY KEY,
    kind TEXT NOT NULL DEFAULT 'credit_transfer'
        CONSTRAINT credit_transfer_workspaces_kind_check CHECK (kind = 'credit_transfer'),
    -- Cases (conversations) and their uploaded syllabuses are deleted this
    -- many days after they were last touched. External syllabuses often
    -- arrive with an applicant's personal data, so nothing is kept for
    -- longer than the assessment needs.
    case_retention_days INTEGER NOT NULL DEFAULT 90
        CONSTRAINT credit_transfer_workspaces_retention_check
        CHECK (case_retention_days BETWEEN 1 AND 3650),
    -- Lowest `users.role` granted implicit access. Students never qualify.
    access_min_role TEXT NOT NULL DEFAULT 'teacher'
        CONSTRAINT credit_transfer_workspaces_access_min_role_check
        CHECK (access_min_role IN ('teacher', 'integrator', 'admin')),
    CONSTRAINT credit_transfer_workspaces_workspace_fkey FOREIGN KEY (workspace_id, kind)
        REFERENCES workspaces (id, kind) ON DELETE CASCADE
);

-- Same ordering as `UserRole::rank` in minerva-core.
CREATE FUNCTION user_role_rank(role TEXT) RETURNS INTEGER
    LANGUAGE sql IMMUTABLE STRICT PARALLEL SAFE
    AS $$ SELECT CASE role
                    WHEN 'admin' THEN 3
                    WHEN 'integrator' THEN 2
                    WHEN 'teacher' THEN 1
                    ELSE 0
                 END $$;

-- Who can use a workspace and in which workspace role. Every membership
-- check reads this instead of `workspace_members`, so implicit access is
-- decided in exactly one place. Implicit users get `student`: in a staff
-- tool that means "uses it" (chats, owns their cases), not "enrolled".
CREATE VIEW workspace_access AS
SELECT m.workspace_id, m.user_id, m.role, FALSE AS implicit
FROM workspace_members m
UNION ALL
SELECT ct.workspace_id, u.id, 'student', TRUE
FROM credit_transfer_workspaces ct
JOIN users u ON user_role_rank(u.role) >= user_role_rank(ct.access_min_role)
WHERE NOT EXISTS (
    SELECT 1 FROM workspace_members m
    WHERE m.workspace_id = ct.workspace_id AND m.user_id = u.id
);
