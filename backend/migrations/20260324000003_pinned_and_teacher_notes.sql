-- Add pinned flag to conversations
ALTER TABLE conversations ADD COLUMN pinned BOOLEAN NOT NULL DEFAULT FALSE;

-- Teacher notes on conversations or individual messages
CREATE TABLE teacher_notes (
    id UUID PRIMARY KEY,
    conversation_id UUID NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    message_id UUID REFERENCES messages(id) ON DELETE CASCADE,
    author_id UUID NOT NULL REFERENCES users(id),
    content TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_conversations_course_pinned ON conversations(course_id, pinned) WHERE pinned = true;
CREATE INDEX idx_teacher_notes_conversation ON teacher_notes(conversation_id);
CREATE INDEX idx_teacher_notes_message ON teacher_notes(message_id) WHERE message_id IS NOT NULL;
