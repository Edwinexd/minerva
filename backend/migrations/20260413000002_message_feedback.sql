-- Per-user thumbs up/down feedback on assistant messages. Thumbs down may
-- carry an optional category and free-text comment so teachers can triage
-- recurring failure modes (hallucination, off-topic, etc).
CREATE TABLE message_feedback (
    id UUID PRIMARY KEY,
    message_id UUID NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    rating TEXT NOT NULL CHECK (rating IN ('up', 'down')),
    category TEXT,
    comment TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (message_id, user_id)
);

CREATE INDEX idx_message_feedback_message ON message_feedback(message_id);
