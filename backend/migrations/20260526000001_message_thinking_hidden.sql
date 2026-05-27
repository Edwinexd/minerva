-- Per-message flag indicating the extraction guard's constraint was
-- active for this turn's research phase. The read-time gate on the
-- conversation-detail route uses it to suppress `thinking_transcript`
-- and `tool_events` for the conversation owner; the source columns
-- themselves stay populated so the teacher dashboard retains the audit
-- trail of what the research agent actually emitted on a guarded turn.
--
-- Frontend rule once this column propagates to MessageResponse:
--   thinking_ms IS NOT NULL && thinking_transcript IS NULL
--     -> "[Reasoning hidden under integrity guard for this turn]"
-- Naturally subsumes the read-time REWROTE_FLAG suppression already
-- shipped in commit 48cf9ce (rewritten turns also set this flag at
-- persist time going forward; the REWROTE_FLAG-based path remains as
-- the fallback for messages persisted before this column existed).
--
-- Default FALSE so historical rows are unaffected. New writes from
-- `common::finalize` set it from the strategy's GuardDecision.

ALTER TABLE messages
  ADD COLUMN thinking_hidden BOOLEAN NOT NULL DEFAULT FALSE;
