-- Aegis live-iteration history. Captures every debounced analyze
-- call the frontend fires while the participant is editing a draft.
-- This is the meaningful behavioural data for an Aegis evaluation:
-- the prompt_analyses table only persists the at-send verdict (one
-- row per submitted message), so without this table the iteration
-- loop "Aegis suggested X, participant edited, Aegis re-analysed,
-- participant kept editing, ..." is invisible after the fact.
--
-- Application gate: rows are only inserted when the conversation's
-- course has the `study_mode` feature flag on (see
-- `routes::chat::analyze_prompt_for_user`). For regular Aegis-on
-- courses the analyzer still runs in-flight but no per-keystroke
-- draft text gets persisted; users in those courses haven't
-- consented to that level of capture.
--
-- One row per analyze call. `draft_text` is the exact content the
-- frontend POSTed; `suggestions` is the JSONB array Aegis returned
-- (same shape as `prompt_analyses.suggestions`). `created_at` is
-- the wall-clock; combined with conversation_id it gives the full
-- editing trace per task.

CREATE TABLE aegis_iterations (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    conversation_id UUID NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    draft_text      TEXT NOT NULL,
    suggestions     JSONB NOT NULL,
    mode            TEXT NOT NULL CHECK (mode IN ('beginner', 'expert')),
    model_used      TEXT NOT NULL,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_aegis_iterations_conversation
    ON aegis_iterations(conversation_id, created_at);
