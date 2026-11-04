-- Structured credit transfer assessments, and the plan facts they compare.
--
-- `syllabus_versions` gains what the scheduler reads out of each downloaded
-- plan PDF: its text with page markers (`[Page 2]`), so an assessment can
-- cite pages without opening files; the level code (G1N, G2F, A1N, ...);
-- and the modules the course is examined in, each with its credits. The
-- assessment compares level and credits against these instead of trusting
-- the model to read them, and a partial-credit suggestion names a module
-- from this list. All stay NULL until `facts_parsed_at` is set, so
-- existing downloads are backfilled.
ALTER TABLE syllabus_versions
    ADD COLUMN text TEXT,
    ADD COLUMN level TEXT,
    ADD COLUMN modules JSONB,
    ADD COLUMN page_count INTEGER,
    ADD COLUMN facts_parsed_at TIMESTAMPTZ;

CREATE INDEX idx_syllabus_versions_facts_pending
    ON syllabus_versions (code)
    WHERE content_hash IS NOT NULL AND facts_parsed_at IS NULL;

-- One structured assessment per assistant message that produced one. The
-- message itself carries a plain-text rendering of the same assessment, so
-- follow-up turns, exports and every other reader of `messages` still work;
-- this row is what the case view draws its cards from.
--
-- `best_code` / `best_verdict` are the strongest candidate, denormalised
-- for the case list.
CREATE TABLE case_assessments (
    message_id UUID PRIMARY KEY REFERENCES messages(id) ON DELETE CASCADE,
    conversation_id UUID NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    assessment JSONB NOT NULL,
    best_code TEXT,
    best_verdict TEXT
        CONSTRAINT case_assessments_best_verdict_check
        CHECK (best_verdict IN ('full', 'partial', 'none')),
    candidate_count INTEGER NOT NULL DEFAULT 0,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_case_assessments_conversation
    ON case_assessments (conversation_id, created_at DESC);
