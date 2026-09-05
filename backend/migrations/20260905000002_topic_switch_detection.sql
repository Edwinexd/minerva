-- Two-layer "you seem to have switched topic" detection on user turns.
--
-- The per-conversation token ceilings (migration 20260905000001) fire on
-- cumulative spend, which is the operator's concern at an arbitrary
-- moment. This fires when starting a fresh chat is actually the right
-- move, which is a rule a student can recognise as true.
--
-- Layer 1 is a cosine comparison of the new user turn against the
-- conversation's earlier user turns; layer 2 is a small utility-model
-- call that only runs when layer 1 trips. Layer 1 alone is not good
-- enough to accuse a student with: measured against 867 real user turns
-- from 72 prod conversations on the production embedding model, a
-- cosine threshold holding per-turn false alarms at 5% still put a
-- spurious nudge in 36% of entirely on-topic conversations. Loosening
-- layer 1 to catch ~89% of switches and letting the LLM adjudicate the
-- ~33% of turns that trip is what makes the signal usable.

-- Cached query embedding of a user message. Written once, on insert of
-- the turn, so a conversation embeds each of its turns exactly once
-- instead of re-embedding the whole tail every turn (which is quadratic
-- in conversation length, on the one code path whose whole purpose is
-- to stop quadratic growth).
--
-- REAL[] rather than a pgvector column: there is no pgvector extension
-- here (document vectors live in Qdrant), we never index or ANN-search
-- these, and the only operation is a dot product against at most a
-- handful of same-conversation rows. A native float4 array is ~3 KB for
-- the 768-dim production model and needs no extension.
--
-- The model id is stored beside the vector because embeddings are only
-- comparable within one model. A course that rotates its embedding
-- model leaves rows whose vectors mean nothing next to new ones; the
-- reader compares this column and treats a mismatch as "no cached
-- vector" rather than silently computing a garbage similarity.
ALTER TABLE messages
    ADD COLUMN topic_embedding REAL[],
    ADD COLUMN topic_embedding_model TEXT;

ALTER TABLE messages
    ADD CONSTRAINT messages_topic_embedding_paired
        CHECK ((topic_embedding IS NULL) = (topic_embedding_model IS NULL));

-- Outcome of the two-layer check for this user turn.
--
--   NULL          not evaluated: historical rows, the feature flag off
--                 for the course, too few earlier turns to compare
--                 against, or the embedder was unavailable.
--   'on_topic'    layer 1 cleared it. No LLM call was made.
--   'confirmed'   layer 1 tripped and the model agreed it is a new topic.
--                 The only value that surfaces a nudge.
--   'rejected'    layer 1 tripped and the model disagreed.
--   'undetermined' layer 1 tripped but layer 2 was unavailable. Treated
--                 as "no nudge": a summarizer outage must never produce
--                 an accusation.
--
-- Text + CHECK rather than a bool, because the interesting distinction
-- is three-way and a NULL/TRUE/FALSE bool cannot express it. The
-- 'rejected' rows are the operationally valuable ones: they measure
-- layer 1's precision on live traffic, so the cosine threshold can be
-- retuned against real disagreements rather than a proxy.
ALTER TABLE messages
    ADD COLUMN topic_shift TEXT;

ALTER TABLE messages
    ADD CONSTRAINT messages_topic_shift_valid
        CHECK (topic_shift IS NULL
               OR topic_shift IN ('on_topic', 'confirmed', 'rejected', 'undetermined'));

-- The reader wants the last few user turns of one conversation with
-- their cached vectors. Covered by the existing
-- idx_messages_conversation, but that index is not ordered, and this
-- path runs on every chat turn.
CREATE INDEX idx_messages_conversation_role_created
    ON messages (conversation_id, role, created_at DESC);
