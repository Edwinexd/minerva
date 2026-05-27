-- Fix off-by-one in extraction_* flag rows' `turn_index` column.
--
-- `compute_turn_index` (strategy/extraction_guard.rs) used to return
-- `prior_user_count + 1`, with the docstring asserting that history
-- excluded the current turn's user message. Production never honoured
-- that precondition: routes/chat.rs::run_chat_message persists the
-- user row BEFORE loading history and passing it to the strategy, so
-- the count already included the current user and the function
-- returned `real_turn + 1`. Every extraction_* flag row written since
-- the guard shipped therefore carries `turn_index = real_turn + 1`.
--
-- Off-by-one consequences this migration corrects:
--   1. The new read-time owner-suppression gate in
--      routes/chat.rs::get_conversation + routes/embed.rs (and its
--      REWROTE_FLAG-based historical fallback) walks persisted
--      messages assigning turn N to the assistant after user N. With
--      the off-by-one the fallback never matched, so historical
--      conversations whose only suppression signal was the
--      REWROTE_FLAG row leaked thinking_transcript / tool_events /
--      chunks_used to the conversation owner on reload.
--   2. The teacher dashboard
--      (frontend/.../teacher/conversations-page.tsx) uses the same
--      walker convention for its flag-to-message join; every guard
--      badge (intent_detected, constraint_activated, rewrote,
--      engagement_refused, constraint_lifted) was attached to the
--      assistant of the NEXT turn, or nowhere when the flag fired
--      on the final turn of a conversation.
--
-- Paired with the code fix that drops the `+1` in compute_turn_index
-- and removes the duplicate `.push(current_user_content)` in
-- `recent_user_messages`. After this migration runs and the new
-- code deploys, both historical and going-forward flag rows align to
-- the canonical convention (turn N = Nth user message in history).
--
-- Notes:
--   * `turn_index > 0` guard is paranoia. The buggy writer always
--     returned >= 2 (prior_user_count = 1 on first turn + 1), so no
--     legacy row should be at 0 or below. The clamp prevents writing
--     a negative value if some seed/test fixture managed to slip a
--     0-stamped row through.
--   * Only `extraction_*` flags are touched. Other flag kinds that
--     happen to use turn_index in the future have to manage their
--     own alignment.
--   * kg_state.recent_turns[].turn_idx (JSONB on conversations) also
--     carries the off-by-one, but is read only for sliding-window
--     proximity (which counts assignments across turns, not turn_idx
--     directly) and for dashboard display; leaving it alone avoids
--     a JSONB rewrite for cosmetic alignment.

UPDATE conversation_flags
SET turn_index = turn_index - 1
WHERE flag IN (
    'extraction_intent_detected',
    'extraction_constraint_activated',
    'extraction_rewrote',
    'extraction_constraint_lifted',
    'extraction_engagement_refused'
)
AND turn_index IS NOT NULL
AND turn_index > 0;
