-- Make primary and alias EPPNs one database-enforced identity namespace.
--
-- `users.eppn` and `user_eppn_aliases.eppn` previously had independent
-- UNIQUE constraints.  An EPPN could therefore be an alias of one user and
-- the primary of another.  Several find-or-create callers also looked only
-- at `users`, so this did not require a race (although concurrent auth/import
-- made it easier to hit).
--
-- The first half of this migration repairs every existing collision.  Users
-- connected by any normalized primary/alias EPPN form one component; the
-- oldest user row survives, all dependent rows are redirected to it, and the
-- most recently observed EPPN remains primary.  The second half installs a
-- registry + triggers.  The registry's primary key is the actual cross-table
-- serialization point, so concurrent writes cannot recreate the split.

LOCK TABLE users IN SHARE ROW EXCLUSIVE MODE;
LOCK TABLE user_eppn_aliases IN SHARE ROW EXCLUSIVE MODE;

DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM users WHERE btrim(eppn) = '')
       OR EXISTS (SELECT 1 FROM user_eppn_aliases WHERE btrim(eppn) = '') THEN
        RAISE EXCEPTION 'cannot migrate empty EPPN';
    END IF;
END
$$;

-- Map every user to the oldest user in its connected identity component.
-- Including primary-primary and alias-alias matches also repairs historical
-- case-only duplicates, not just the known primary-vs-alias shape.
CREATE TEMP TABLE _eppn_owners ON COMMIT DROP AS
SELECT id AS user_id, lower(btrim(eppn)) AS eppn
FROM users
UNION
SELECT user_id, lower(btrim(eppn))
FROM user_eppn_aliases;

CREATE INDEX ON _eppn_owners (user_id);
CREATE INDEX ON _eppn_owners (eppn);

CREATE TEMP TABLE _user_merge_map ON COMMIT DROP AS
WITH RECURSIVE reach(user_id, reachable_user_id) AS (
    SELECT id, id FROM users
    UNION
    SELECT r.user_id, neighbour.user_id
    FROM reach r
    JOIN _eppn_owners owned ON owned.user_id = r.reachable_user_id
    JOIN _eppn_owners neighbour ON neighbour.eppn = owned.eppn
)
SELECT
    r.user_id,
    (array_agg(r.reachable_user_id ORDER BY u.created_at, r.reachable_user_id))[1]
        AS survivor_id
FROM reach r
JOIN users u ON u.id = r.reachable_user_id
GROUP BY r.user_id;

CREATE UNIQUE INDEX ON _user_merge_map (user_id);
CREATE INDEX ON _user_merge_map (survivor_id);

CREATE TEMP TABLE _merge_component_users ON COMMIT DROP AS
SELECT m.user_id, m.survivor_id
FROM _user_merge_map m
WHERE EXISTS (
    SELECT 1
    FROM _user_merge_map peer
    WHERE peer.survivor_id = m.survivor_id
      AND peer.user_id <> peer.survivor_id
);

-- Preserve every spelling-normalized identity and its best last-seen time.
CREATE TEMP TABLE _merged_eppns ON COMMIT DROP AS
SELECT m.survivor_id, lower(btrim(identity.eppn)) AS eppn,
       max(identity.last_seen_at) AS last_seen_at
FROM (
    SELECT id AS user_id, eppn, updated_at AS last_seen_at FROM users
    UNION ALL
    SELECT user_id, eppn, last_seen_at FROM user_eppn_aliases
) identity
JOIN _user_merge_map m ON m.user_id = identity.user_id
GROUP BY m.survivor_id, lower(btrim(identity.eppn));

CREATE TEMP TABLE _merged_primary_eppns ON COMMIT DROP AS
SELECT DISTINCT ON (survivor_id) survivor_id, eppn
FROM _merged_eppns
ORDER BY survivor_id, last_seen_at DESC, eppn;

-- Preserve user-level state.  Owner limits deliberately remain those of the
-- oldest (surviving) account: unlike role/privacy fields, they are operator
-- configuration and must not be replaced by a newer row's default.
CREATE TEMP TABLE _merged_user_values ON COMMIT DROP AS
SELECT
    m.survivor_id,
    (array_agg(u.display_name ORDER BY (u.display_name IS NOT NULL) DESC,
                                     u.updated_at DESC)
        FILTER (WHERE u.display_name IS NOT NULL))[1] AS display_name,
    (array_agg(u.role ORDER BY u.role_manually_set DESC,
                               CASE u.role
                                   WHEN 'admin' THEN 4
                                   WHEN 'integrator' THEN 3
                                   WHEN 'teacher' THEN 2
                                   ELSE 1
                               END DESC,
                               u.updated_at DESC))[1] AS role,
    bool_or(u.suspended) AS suspended,
    bool_or(u.role_manually_set) AS role_manually_set,
    min(u.privacy_acknowledged_at) AS privacy_acknowledged_at,
    min(u.created_at) AS created_at,
    max(u.updated_at) AS updated_at
FROM _user_merge_map m
JOIN users u ON u.id = m.user_id
GROUP BY m.survivor_id;

-- Rebuild the user-keyed tables whose uniqueness constraints need explicit
-- conflict resolution before the generic FK redirect below.
CREATE TEMP TABLE _merged_course_members ON COMMIT DROP AS
SELECT cm.course_id, m.survivor_id AS user_id,
       (array_agg(cm.role ORDER BY CASE cm.role
           WHEN 'owner' THEN 4 WHEN 'teacher' THEN 3 WHEN 'ta' THEN 2 ELSE 1 END DESC))[1] AS role,
       min(cm.added_at) AS added_at
FROM course_members cm
JOIN _merge_component_users m ON m.user_id = cm.user_id
GROUP BY cm.course_id, m.survivor_id;
DELETE FROM course_members cm
USING _merge_component_users m WHERE cm.user_id = m.user_id;
INSERT INTO course_members (course_id, user_id, role, added_at)
SELECT course_id, user_id, role, added_at FROM _merged_course_members;

CREATE TEMP TABLE _merged_role_suggestions ON COMMIT DROP AS
SELECT DISTINCT ON (s.course_id, m.survivor_id, s.suggested_role)
       s.id, s.course_id, m.survivor_id AS user_id, s.suggested_role,
       s.source, s.source_detail, s.created_at, s.resolved_at,
       s.resolved_by, s.resolution
FROM course_member_role_suggestions s
JOIN _merge_component_users m ON m.user_id = s.user_id
-- A resolved decision wins over a pending duplicate; otherwise retain the
-- newest observation of the suggestion.
ORDER BY s.course_id, m.survivor_id, s.suggested_role,
         (s.resolution IS NOT NULL) DESC, s.created_at DESC, s.id;
DELETE FROM course_member_role_suggestions s
USING _merge_component_users m WHERE s.user_id = m.user_id;
INSERT INTO course_member_role_suggestions
    (id, course_id, user_id, suggested_role, source, source_detail,
     created_at, resolved_at, resolved_by, resolution)
SELECT id, course_id, user_id, suggested_role, source, source_detail,
       created_at, resolved_at, resolved_by, resolution
FROM _merged_role_suggestions;

CREATE TEMP TABLE _merged_usage_daily ON COMMIT DROP AS
SELECT (array_agg(ud.id ORDER BY ud.id))[1] AS id,
       m.survivor_id AS user_id, ud.course_id, ud.date,
       sum(ud.prompt_tokens) AS prompt_tokens,
       sum(ud.completion_tokens) AS completion_tokens,
       sum(ud.embedding_tokens) AS embedding_tokens,
       sum(ud.request_count) AS request_count,
       sum(ud.research_prompt_tokens) AS research_prompt_tokens,
       sum(ud.research_completion_tokens) AS research_completion_tokens,
       ud.model, (array_agg(ud.provider ORDER BY ud.id))[1] AS provider
FROM usage_daily ud
JOIN _merge_component_users m ON m.user_id = ud.user_id
GROUP BY m.survivor_id, ud.course_id, ud.date, ud.model;
DELETE FROM usage_daily ud
USING _merge_component_users m WHERE ud.user_id = m.user_id;
INSERT INTO usage_daily
    (id, user_id, course_id, date, prompt_tokens, completion_tokens,
     embedding_tokens, request_count, research_prompt_tokens,
     research_completion_tokens, model, provider)
SELECT id, user_id, course_id, date, prompt_tokens, completion_tokens,
       embedding_tokens, request_count, research_prompt_tokens,
       research_completion_tokens, model, provider
FROM _merged_usage_daily;

CREATE TEMP TABLE _merged_message_feedback ON COMMIT DROP AS
SELECT DISTINCT ON (mf.message_id, m.survivor_id)
       mf.id, mf.message_id, m.survivor_id AS user_id, mf.rating,
       mf.category, mf.comment, mf.created_at, mf.updated_at,
       mf.acknowledged_at, mf.acknowledged_by
FROM message_feedback mf
JOIN _merge_component_users m ON m.user_id = mf.user_id
ORDER BY mf.message_id, m.survivor_id, mf.updated_at DESC, mf.id;
DELETE FROM message_feedback mf
USING _merge_component_users m WHERE mf.user_id = m.user_id;
INSERT INTO message_feedback
    (id, message_id, user_id, rating, category, comment, created_at,
     updated_at, acknowledged_at, acknowledged_by)
SELECT id, message_id, user_id, rating, category, comment, created_at,
       updated_at, acknowledged_at, acknowledged_by
FROM _merged_message_feedback;

CREATE TEMP TABLE _merged_feature_flags ON COMMIT DROP AS
SELECT DISTINCT ON (ff.flag, m.survivor_id)
       ff.id, ff.flag, ff.course_id, m.survivor_id AS user_id,
       bool_or(ff.enabled) OVER (PARTITION BY ff.flag, m.survivor_id) AS enabled,
       min(ff.created_at) OVER (PARTITION BY ff.flag, m.survivor_id) AS created_at,
       max(ff.updated_at) OVER (PARTITION BY ff.flag, m.survivor_id) AS updated_at
FROM feature_flags ff
JOIN _merge_component_users m ON m.user_id = ff.user_id
ORDER BY ff.flag, m.survivor_id, ff.updated_at DESC, ff.id;
DELETE FROM feature_flags ff
USING _merge_component_users m WHERE ff.user_id = m.user_id;
INSERT INTO feature_flags (id, flag, course_id, user_id, enabled, created_at, updated_at)
SELECT id, flag, course_id, user_id, enabled, created_at, updated_at
FROM _merged_feature_flags;

CREATE TEMP TABLE _merged_nrps_memberships ON COMMIT DROP AS
SELECT DISTINCT ON (n.nrps_context_id, m.survivor_id)
       n.nrps_context_id, m.survivor_id AS user_id, n.lti_user_id,
       n.last_status, n.last_seen_at, n.created_at
FROM lti_nrps_memberships n
JOIN _merge_component_users m ON m.user_id = n.user_id
ORDER BY n.nrps_context_id, m.survivor_id, n.last_seen_at DESC, n.lti_user_id;
DELETE FROM lti_nrps_memberships n
USING _merge_component_users m WHERE n.user_id = m.user_id;
INSERT INTO lti_nrps_memberships
    (nrps_context_id, user_id, lti_user_id, last_status, last_seen_at, created_at)
SELECT nrps_context_id, user_id, lti_user_id, last_status, last_seen_at, created_at
FROM _merged_nrps_memberships;

CREATE TEMP TABLE _merged_role_observations ON COMMIT DROP AS
SELECT o.attribute, o.value, m.survivor_id AS user_id,
       min(o.first_seen) AS first_seen, max(o.last_seen) AS last_seen
FROM role_rule_attribute_observations o
JOIN _merge_component_users m ON m.user_id = o.user_id
GROUP BY o.attribute, o.value, m.survivor_id;
DELETE FROM role_rule_attribute_observations o
USING _merge_component_users m WHERE o.user_id = m.user_id;
INSERT INTO role_rule_attribute_observations
    (attribute, value, user_id, first_seen, last_seen)
SELECT attribute, value, user_id, first_seen, last_seen
FROM _merged_role_observations;

-- Remove aliases while identities are rebuilt, and park every primary under
-- a collision-proof temporary value.  Both operations are transactional.
DELETE FROM user_eppn_aliases;
UPDATE users SET eppn = '__eppn_merge__:' || id::text;

-- Redirect every remaining single-column FK to users(id).  Keeping this
-- catalog-driven means future non-unique audit/ownership tables are included
-- automatically; uniqueness-sensitive tables above were already collapsed.
DO $$
DECLARE
    fk record;
BEGIN
    FOR fk IN
        SELECT ns.nspname AS schema_name, tbl.relname AS table_name,
               att.attname AS column_name
        FROM pg_constraint con
        JOIN pg_class tbl ON tbl.oid = con.conrelid
        JOIN pg_namespace ns ON ns.oid = tbl.relnamespace
        JOIN pg_attribute att
          ON att.attrelid = con.conrelid AND att.attnum = con.conkey[1]
        JOIN pg_attribute refatt
          ON refatt.attrelid = con.confrelid AND refatt.attnum = con.confkey[1]
        WHERE con.contype = 'f'
          AND con.confrelid = 'users'::regclass
          AND cardinality(con.conkey) = 1
          AND cardinality(con.confkey) = 1
          AND refatt.attname = 'id'
          AND tbl.relname <> 'user_eppn_aliases'
    LOOP
        EXECUTE format(
            'UPDATE %I.%I t SET %I = m.survivor_id '
            'FROM _user_merge_map m '
            'WHERE t.%I = m.user_id AND m.user_id <> m.survivor_id',
            fk.schema_name, fk.table_name, fk.column_name, fk.column_name
        );
    END LOOP;
END
$$;

DELETE FROM users u
USING _user_merge_map m
WHERE u.id = m.user_id AND m.user_id <> m.survivor_id;

UPDATE users u
SET eppn = p.eppn,
    display_name = v.display_name,
    role = v.role,
    suspended = v.suspended,
    role_manually_set = v.role_manually_set,
    privacy_acknowledged_at = v.privacy_acknowledged_at,
    created_at = v.created_at,
    updated_at = v.updated_at
FROM _merged_primary_eppns p
JOIN _merged_user_values v USING (survivor_id)
WHERE u.id = p.survivor_id;

INSERT INTO user_eppn_aliases (user_id, eppn, last_seen_at, created_at)
SELECT e.survivor_id, e.eppn, e.last_seen_at, e.last_seen_at
FROM _merged_eppns e
JOIN _merged_primary_eppns p USING (survivor_id)
WHERE e.eppn <> p.eppn;

ALTER TABLE users
    ADD CONSTRAINT users_eppn_normalized
    CHECK (eppn = lower(btrim(eppn)) AND eppn <> '');
ALTER TABLE user_eppn_aliases
    ADD CONSTRAINT user_eppn_aliases_eppn_normalized
    CHECK (eppn = lower(btrim(eppn)) AND eppn <> '');

CREATE TABLE user_eppn_registry (
    eppn TEXT PRIMARY KEY,
    user_id UUID NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('primary', 'alias')),
    UNIQUE (eppn, user_id)
);

INSERT INTO user_eppn_registry (eppn, user_id, kind)
SELECT eppn, id, 'primary' FROM users
UNION ALL
SELECT eppn, user_id, 'alias' FROM user_eppn_aliases;

CREATE OR REPLACE FUNCTION reserve_user_eppn()
RETURNS trigger
LANGUAGE plpgsql
AS $$
DECLARE
    wanted_user_id UUID;
    existing user_eppn_registry%ROWTYPE;
BEGIN
    IF TG_ARGV[0] = 'primary' THEN
        wanted_user_id := NEW.id;
    ELSE
        wanted_user_id := NEW.user_id;
    END IF;

    INSERT INTO user_eppn_registry AS registry (eppn, user_id, kind)
    VALUES (NEW.eppn, wanted_user_id, TG_ARGV[0])
    -- The no-op update makes PostgreSQL return the winning row even when this
    -- statement had to wait for a concurrent reservation that was invisible
    -- to its original READ COMMITTED snapshot.
    ON CONFLICT (eppn) DO UPDATE SET eppn = registry.eppn
    RETURNING * INTO existing;
    IF existing.user_id <> wanted_user_id OR existing.kind <> TG_ARGV[0] THEN
        RAISE unique_violation USING
            CONSTRAINT = 'user_eppn_registry_pkey',
            MESSAGE = format(
                'EPPN %s is already the %s identity of user %s',
                NEW.eppn, existing.kind, existing.user_id
            );
    END IF;
    RETURN NEW;
END
$$;

CREATE OR REPLACE FUNCTION release_user_eppn()
RETURNS trigger
LANGUAGE plpgsql
AS $$
DECLARE
    old_user_id UUID;
    new_user_id UUID;
BEGIN
    IF TG_ARGV[0] = 'primary' THEN
        old_user_id := OLD.id;
    ELSE
        old_user_id := OLD.user_id;
    END IF;
    IF TG_OP = 'UPDATE' THEN
        IF TG_ARGV[0] = 'primary' THEN
            new_user_id := NEW.id;
        ELSE
            new_user_id := NEW.user_id;
        END IF;
        IF OLD.eppn = NEW.eppn AND old_user_id = new_user_id THEN
            RETURN NEW;
        END IF;
    END IF;

    DELETE FROM user_eppn_registry
    WHERE eppn = OLD.eppn AND user_id = old_user_id AND kind = TG_ARGV[0];
    RETURN COALESCE(NEW, OLD);
END
$$;

CREATE TRIGGER users_reserve_eppn
BEFORE INSERT OR UPDATE OF id, eppn ON users
FOR EACH ROW EXECUTE FUNCTION reserve_user_eppn('primary');
CREATE TRIGGER users_release_eppn
AFTER UPDATE OF id, eppn OR DELETE ON users
FOR EACH ROW EXECUTE FUNCTION release_user_eppn('primary');

CREATE TRIGGER user_eppn_aliases_reserve_eppn
BEFORE INSERT OR UPDATE OF user_id, eppn ON user_eppn_aliases
FOR EACH ROW EXECUTE FUNCTION reserve_user_eppn('alias');
CREATE TRIGGER user_eppn_aliases_release_eppn
AFTER UPDATE OF user_id, eppn OR DELETE ON user_eppn_aliases
FOR EACH ROW EXECUTE FUNCTION release_user_eppn('alias');

-- Guard the registry itself against accidental deletion or ownership changes
-- while a primary/alias row still references it.
ALTER TABLE users
    ADD CONSTRAINT users_eppn_registry_fkey
    FOREIGN KEY (eppn, id) REFERENCES user_eppn_registry (eppn, user_id)
    DEFERRABLE INITIALLY IMMEDIATE;
ALTER TABLE user_eppn_aliases
    ADD CONSTRAINT user_eppn_aliases_registry_fkey
    FOREIGN KEY (eppn, user_id) REFERENCES user_eppn_registry (eppn, user_id)
    DEFERRABLE INITIALLY IMMEDIATE;
