-- Courses become one kind of workspace.
--
-- Minerva started course-native: every corpus, conversation, member and
-- spend row hung off `courses`. Staff tools (credit transfer first) need
-- the same machinery without being a course: no term, no Daisy offering,
-- no LMS. This is class-table inheritance:
--
--   workspaces  the old `courses` table: name, owner, model / retrieval
--               config, caps. `kind` says which subtype row exists.
--   courses     1:1 subtype keyed by `workspace_id`, holding what only a
--               course has (term, course code, Daisy auto-management).
--
-- `(workspace_id, kind)` references `workspaces (id, kind)` with a CHECK
-- pinning the subtype's kind, so a workspace cannot carry the wrong
-- subtype row. Generic tables rename `course_id` to `workspace_id`;
-- course-only tables (Daisy, Play, LTI, Canvas, Moodle keys, join links)
-- keep `course_id` and now reference the subtype.
--
-- Not expand/contract: every rename breaks the previous build's queries.
-- Pods of the previous build that are still running when this applies
-- fail their requests until they exit (a few minutes of errors, no data
-- damage: the migration is one transaction and old writes fail outright).
-- Stop them first to avoid that.
--
-- Qdrant point payloads keep their "course_id" key; it is a stored format
-- and renaming it would mean re-embedding every collection.

-- 1. The table every corpus hangs off becomes `workspaces`.
ALTER TABLE courses RENAME TO workspaces;
ALTER TABLE workspaces RENAME CONSTRAINT courses_pkey TO workspaces_pkey;
ALTER TABLE workspaces RENAME CONSTRAINT courses_owner_id_fkey TO workspaces_owner_id_fkey;
ALTER TABLE workspaces RENAME CONSTRAINT courses_min_score_check TO workspaces_min_score_check;
ALTER TABLE workspaces RENAME CONSTRAINT courses_conversation_token_limits_non_negative
    TO workspaces_conversation_token_limits_non_negative;
ALTER TABLE workspaces RENAME CONSTRAINT courses_conversation_hard_limit_above_soft
    TO workspaces_conversation_hard_limit_above_soft;

ALTER TABLE workspaces
    ADD COLUMN kind TEXT NOT NULL DEFAULT 'course'
        CONSTRAINT workspaces_kind_check CHECK (kind IN ('course', 'credit_transfer'));
-- Target for the subtype tables' (workspace_id, kind) foreign keys.
ALTER TABLE workspaces ADD CONSTRAINT workspaces_id_kind_key UNIQUE (id, kind);

-- 1b. The course subtype takes the columns only a course has.
CREATE TABLE courses (
    workspace_id UUID PRIMARY KEY,
    kind TEXT NOT NULL DEFAULT 'course' CONSTRAINT courses_kind_check CHECK (kind = 'course'),
    -- 'VT2026' / 'HT2025'; NULL for ad-hoc courses.
    semester_label TEXT,
    -- Daisy designation (e.g. 'PROG2') for Daisy-linked courses.
    course_code TEXT,
    -- Created by the Daisy import; membership sync stays additive.
    auto_managed BOOLEAN NOT NULL DEFAULT FALSE,
    CONSTRAINT courses_workspace_fkey FOREIGN KEY (workspace_id, kind)
        REFERENCES workspaces (id, kind) ON DELETE CASCADE
);

INSERT INTO courses (workspace_id, semester_label, course_code, auto_managed)
SELECT id, semester_label, course_code, auto_managed FROM workspaces;

ALTER TABLE workspaces
    DROP COLUMN semester_label,
    DROP COLUMN course_code,
    DROP COLUMN auto_managed;

CREATE INDEX idx_courses_semester_label ON courses (semester_label)
    WHERE semester_label IS NOT NULL;
CREATE INDEX idx_courses_course_code ON courses (course_code)
    WHERE course_code IS NOT NULL;

-- 2. Course-only tables: point their foreign keys at the course subtype.
--    Same column name and delete action; only the referenced table moves.
ALTER TABLE api_keys DROP CONSTRAINT api_keys_course_id_fkey;
ALTER TABLE api_keys ADD CONSTRAINT api_keys_course_id_fkey FOREIGN KEY (course_id)
    REFERENCES courses(workspace_id) ON DELETE CASCADE;
ALTER TABLE canvas_connections DROP CONSTRAINT canvas_connections_course_id_fkey;
ALTER TABLE canvas_connections ADD CONSTRAINT canvas_connections_course_id_fkey FOREIGN KEY (course_id)
    REFERENCES courses(workspace_id) ON DELETE CASCADE;
ALTER TABLE course_daisy_offerings DROP CONSTRAINT course_daisy_offerings_course_id_fkey;
ALTER TABLE course_daisy_offerings ADD CONSTRAINT course_daisy_offerings_course_id_fkey FOREIGN KEY (course_id)
    REFERENCES courses(workspace_id) ON DELETE CASCADE;
ALTER TABLE course_member_role_suggestions DROP CONSTRAINT course_member_role_suggestions_course_id_fkey;
ALTER TABLE course_member_role_suggestions ADD CONSTRAINT course_member_role_suggestions_course_id_fkey FOREIGN KEY (course_id)
    REFERENCES courses(workspace_id) ON DELETE CASCADE;
ALTER TABLE daisy_pending_imports DROP CONSTRAINT daisy_pending_imports_existing_course_id_fkey;
ALTER TABLE daisy_pending_imports ADD CONSTRAINT daisy_pending_imports_existing_course_id_fkey FOREIGN KEY (existing_course_id)
    REFERENCES courses(workspace_id) ON DELETE SET NULL;
ALTER TABLE lti_course_bindings DROP CONSTRAINT lti_course_bindings_course_id_fkey;
ALTER TABLE lti_course_bindings ADD CONSTRAINT lti_course_bindings_course_id_fkey FOREIGN KEY (course_id)
    REFERENCES courses(workspace_id) ON DELETE CASCADE;
ALTER TABLE lti_nrps_contexts DROP CONSTRAINT lti_nrps_contexts_course_id_fkey;
ALTER TABLE lti_nrps_contexts ADD CONSTRAINT lti_nrps_contexts_course_id_fkey FOREIGN KEY (course_id)
    REFERENCES courses(workspace_id) ON DELETE CASCADE;
ALTER TABLE lti_registrations DROP CONSTRAINT lti_registrations_course_id_fkey;
ALTER TABLE lti_registrations ADD CONSTRAINT lti_registrations_course_id_fkey FOREIGN KEY (course_id)
    REFERENCES courses(workspace_id) ON DELETE CASCADE;
ALTER TABLE play_designations DROP CONSTRAINT play_designations_course_id_fkey;
ALTER TABLE play_designations ADD CONSTRAINT play_designations_course_id_fkey FOREIGN KEY (course_id)
    REFERENCES courses(workspace_id) ON DELETE CASCADE;
ALTER TABLE signed_urls DROP CONSTRAINT signed_urls_course_id_fkey;
ALTER TABLE signed_urls ADD CONSTRAINT signed_urls_course_id_fkey FOREIGN KEY (course_id)
    REFERENCES courses(workspace_id) ON DELETE NO ACTION;

-- 3. Generic tables: course_id becomes workspace_id; course_* tables
--    become workspace_*. Foreign keys follow the parent rename on their own.
ALTER TABLE course_members RENAME COLUMN course_id TO workspace_id;
ALTER TABLE conversations RENAME COLUMN course_id TO workspace_id;
ALTER TABLE course_conversation_topics RENAME COLUMN course_id TO workspace_id;
ALTER TABLE course_suggested_questions RENAME COLUMN course_id TO workspace_id;
ALTER TABLE course_token_usage RENAME COLUMN course_id TO workspace_id;
ALTER TABLE document_figures RENAME COLUMN course_id TO workspace_id;
ALTER TABLE document_relations RENAME COLUMN course_id TO workspace_id;
ALTER TABLE documents RENAME COLUMN course_id TO workspace_id;
ALTER TABLE feature_flags RENAME COLUMN course_id TO workspace_id;
ALTER TABLE linker_decisions RENAME COLUMN course_id TO workspace_id;
ALTER TABLE rejected_edge_pairs RENAME COLUMN course_id TO workspace_id;
ALTER TABLE relink_queue RENAME COLUMN course_id TO workspace_id;
ALTER TABLE usage_daily RENAME COLUMN course_id TO workspace_id;
ALTER TABLE visual_extraction_jobs RENAME COLUMN course_id TO workspace_id;
ALTER TABLE course_members RENAME TO workspace_members;
ALTER TABLE course_token_usage RENAME TO workspace_token_usage;
ALTER TABLE course_conversation_topics RENAME TO workspace_conversation_topics;
ALTER TABLE course_suggested_questions RENAME TO workspace_suggested_questions;

-- 4. Index and constraint names on generic tables (cosmetic, no code
--    matches on them).
ALTER TABLE workspace_members RENAME CONSTRAINT course_members_pkey TO workspace_members_pkey;
ALTER TABLE workspace_token_usage RENAME CONSTRAINT course_token_usage_pkey TO workspace_token_usage_pkey;
ALTER TABLE workspace_suggested_questions RENAME CONSTRAINT course_suggested_questions_pkey TO workspace_suggested_questions_pkey;
ALTER TABLE usage_daily RENAME CONSTRAINT usage_daily_user_id_course_id_date_model_key TO usage_daily_user_id_workspace_id_date_model_key;
ALTER TABLE workspace_conversation_topics RENAME CONSTRAINT course_conversation_topics_pkey TO workspace_conversation_topics_pkey;
ALTER TABLE workspace_members RENAME CONSTRAINT course_members_course_id_fkey TO workspace_members_workspace_id_fkey;
ALTER TABLE workspace_members RENAME CONSTRAINT course_members_user_id_fkey TO workspace_members_user_id_fkey;
ALTER TABLE documents RENAME CONSTRAINT documents_course_id_fkey TO documents_workspace_id_fkey;
ALTER TABLE conversations RENAME CONSTRAINT conversations_course_id_fkey TO conversations_workspace_id_fkey;
ALTER TABLE usage_daily RENAME CONSTRAINT usage_daily_course_id_fkey TO usage_daily_workspace_id_fkey;
ALTER TABLE document_relations RENAME CONSTRAINT document_relations_course_id_fkey TO document_relations_workspace_id_fkey;
ALTER TABLE relink_queue RENAME CONSTRAINT relink_queue_course_id_fkey TO relink_queue_workspace_id_fkey;
ALTER TABLE rejected_edge_pairs RENAME CONSTRAINT rejected_edge_pairs_course_id_fkey TO rejected_edge_pairs_workspace_id_fkey;
ALTER TABLE feature_flags RENAME CONSTRAINT feature_flags_course_id_fkey TO feature_flags_workspace_id_fkey;
ALTER TABLE linker_decisions RENAME CONSTRAINT linker_decisions_course_id_fkey TO linker_decisions_workspace_id_fkey;
ALTER TABLE workspace_token_usage RENAME CONSTRAINT course_token_usage_course_id_fkey TO workspace_token_usage_workspace_id_fkey;
ALTER TABLE workspace_suggested_questions RENAME CONSTRAINT course_suggested_questions_course_id_fkey TO workspace_suggested_questions_workspace_id_fkey;
ALTER TABLE document_figures RENAME CONSTRAINT document_figures_course_id_fkey TO document_figures_workspace_id_fkey;
ALTER TABLE workspace_conversation_topics RENAME CONSTRAINT course_conversation_topics_course_id_fkey TO workspace_conversation_topics_workspace_id_fkey;
ALTER TABLE visual_extraction_jobs RENAME CONSTRAINT visual_extraction_jobs_course_id_fkey TO visual_extraction_jobs_workspace_id_fkey;
ALTER INDEX idx_documents_course RENAME TO idx_documents_workspace;
ALTER INDEX idx_conversations_course_user RENAME TO idx_conversations_workspace_user;
ALTER INDEX idx_usage_daily_course_date RENAME TO idx_usage_daily_workspace_date;
ALTER INDEX idx_conversations_course_pinned RENAME TO idx_conversations_workspace_pinned;
ALTER INDEX idx_documents_course_source_url RENAME TO idx_documents_workspace_source_url;
ALTER INDEX idx_documents_course_kind RENAME TO idx_documents_workspace_kind;
ALTER INDEX idx_document_relations_course RENAME TO idx_document_relations_workspace;
ALTER INDEX rejected_edge_pairs_course_idx RENAME TO rejected_edge_pairs_workspace_idx;
ALTER INDEX feature_flags_course_idx RENAME TO feature_flags_workspace_idx;
ALTER INDEX linker_decisions_course_idx RENAME TO linker_decisions_workspace_idx;
ALTER INDEX idx_course_token_usage_course_created RENAME TO idx_workspace_token_usage_workspace_created;
ALTER INDEX idx_course_token_usage_course_category_created RENAME TO idx_workspace_token_usage_workspace_category_created;
ALTER INDEX idx_documents_course_source_active RENAME TO idx_documents_workspace_source_active;
ALTER INDEX idx_documents_course_source_system RENAME TO idx_documents_workspace_source_system;
ALTER INDEX idx_documents_course_content_hash_active RENAME TO idx_documents_workspace_content_hash_active;
ALTER INDEX idx_document_figures_course RENAME TO idx_document_figures_workspace;
