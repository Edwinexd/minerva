-- Files a user attaches to one conversation, e.g. the external syllabus a
-- credit transfer case is about. Unlike documents they belong to the
-- conversation, not the workspace: never chunked or embedded into the
-- shared index, only visible to the conversation's owner, and given to the
-- model as full text on every turn of that conversation.
--
-- The bytes live at {docs_path}/{workspace_id}/attachments/{id}.{ext}; the
-- text is extracted once at upload. Deleting the conversation (by hand or
-- by a workspace's retention sweep) cascades here, and the sweep removes
-- the files first.

CREATE TABLE conversation_attachments (
    id UUID PRIMARY KEY,
    conversation_id UUID NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    uploaded_by UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    filename TEXT NOT NULL,
    mime_type TEXT NOT NULL,
    size_bytes BIGINT NOT NULL,
    content_hash TEXT NOT NULL,
    extracted_text TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_conversation_attachments_conversation
    ON conversation_attachments (conversation_id, created_at);
