-- Credit transfer workspaces record their users as members, like courses,
-- so the members list, usage names and the student count behave the same
-- for both kinds. A member row is added the first time someone uses the
-- workspace (`workspaces::record_access`) and stays after their access
-- ends: in this kind it records who has used it and grants nothing.
-- Access is still the role check, made on every request, so a demotion
-- or a raised `access_min_role` takes effect at once even for members.

CREATE OR REPLACE VIEW workspace_access AS
SELECT m.workspace_id, m.user_id, m.role, FALSE AS implicit
FROM workspace_members m
JOIN users u ON u.id = m.user_id
LEFT JOIN credit_transfer_workspaces ct ON ct.workspace_id = m.workspace_id
WHERE ct.workspace_id IS NULL
   OR user_role_rank(u.role) >= user_role_rank(ct.access_min_role)
UNION ALL
SELECT ct.workspace_id, u.id, 'student', TRUE
FROM credit_transfer_workspaces ct
JOIN users u ON user_role_rank(u.role) >= user_role_rank(ct.access_min_role)
WHERE NOT EXISTS (
    SELECT 1 FROM workspace_members m
    WHERE m.workspace_id = ct.workspace_id AND m.user_id = u.id
);

-- Everyone who already used one: a case or any recorded spend.
INSERT INTO workspace_members (workspace_id, user_id, role, added_at)
SELECT workspace_id, user_id, 'student', MIN(at)
FROM (
    SELECT c.workspace_id, c.user_id, c.created_at AS at
    FROM conversations c
    JOIN credit_transfer_workspaces ct ON ct.workspace_id = c.workspace_id
    UNION ALL
    SELECT u.workspace_id, u.user_id, u.date::timestamptz
    FROM usage_daily u
    JOIN credit_transfer_workspaces ct ON ct.workspace_id = u.workspace_id
) used
GROUP BY workspace_id, user_id
ON CONFLICT (workspace_id, user_id) DO NOTHING;
