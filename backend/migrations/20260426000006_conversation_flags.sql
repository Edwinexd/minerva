-- Reusable per-conversation observability surface for chat-time
-- guards. Started for the extraction_guard rollout, but the schema
-- is intentionally generic: any future per-turn or per-conversation
-- judgement (plagiarism suspicion, off-topic, abuse signals, etc.)
-- can use the same table with a different `flag` value rather than
-- adding a new boolean column per concern.
--
-- The teacher dashboard joins on `conversation_id` to render badges
-- and the per-turn breakdown; nothing in the schema is specific
-- to the extraction case.
--
-- `metadata JSONB` is the wiggle room: each flag kind owns its own
-- payload shape (matched assignment doc ids, classifier rationale,
-- output-check verdict, etc.). We deliberately do NOT type this
-- across flag kinds; the application layer is the single reader/
-- writer per flag.

CREATE TABLE conversation_flags (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    conversation_id UUID NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
   ; Stable string identifier. The application picks from a small
   ; set of known values (currently just 'extraction_attempt'); a
   ; future flag adds a new const, no schema change required.
    flag            TEXT NOT NULL,
   ; 1-based turn index this flag is attached to. Nullable for
   ; conversation-level flags (e.g. "this conversation is tagged
   ; as suspicious overall"). We don't enforce a FK to
   ; conversation_messages because messages are sometimes
   ; deleted / regenerated and we want flags to outlive that.
    turn_index      INT,
   ; Short human-readable explanation, intended for the teacher
   ; dashboard hover.
    rationale       TEXT,
   ; Flag-specific structured data. For extraction_attempt:
   ;   { "matched_assignment_doc_ids": [...], "intent_verdict": ...,
   ;     "output_check_verdict": ..., "rewrote_response": bool }
    metadata        JSONB,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX conversation_flags_conversation_idx
    ON conversation_flags (conversation_id, created_at DESC);

CREATE INDEX conversation_flags_flag_idx
    ON conversation_flags (flag, created_at DESC);

-- KG-driven chat state per conversation: sliding window of recent
-- turns, which assignments have been "near" the conversation, and
-- whether the extraction guard's hard constraint is currently
-- active (and waiting for engagement to lift).
--
-- JSONB rather than separate columns because the shape evolves as
-- the guard's heuristics mature, and re-migrating per change
-- doesn't make sense. Schema (defined+enforced in application
-- layer):
--   {
--     "constraint_active": bool,
--     "constraint_assignment_doc_ids": ["uuid", ...],
--     "constraint_lifted_at_turn": int | null,
--     "recent_turns": [
--         { "turn_idx": int, "assignments_near": ["uuid", ...] },
--         ...  // last N turns, default N=5
--     ]
--   }
-- An empty `{}` is the default for new conversations and existing
-- ones (read accessors treat missing keys as defaults).

ALTER TABLE conversations
    ADD COLUMN kg_state JSONB NOT NULL DEFAULT '{}'::jsonb;
