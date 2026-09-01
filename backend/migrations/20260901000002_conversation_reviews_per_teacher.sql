-- A review is personal to the teacher who opened the conversation.
--
-- Keep the existing rows (they remain useful historical reviews), but allow
-- each teacher to have an independent current review marker. This makes a
-- colleague's read available as an optional triage signal instead of
-- automatically clearing every teacher's unread state.
ALTER TABLE conversation_reviews
    DROP CONSTRAINT conversation_reviews_pkey,
    ADD PRIMARY KEY (conversation_id, reviewed_by);
