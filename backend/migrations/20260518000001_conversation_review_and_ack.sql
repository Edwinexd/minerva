-- Bidirectional unread + acknowledged signals between the student
-- chat surface and the teacher dashboard.
--
-- Adds three orthogonal mechanisms:
--
-- 1. `conversation_reviews` -- one row per conversation capturing
--    the most-recent teaching-team review. Course-shared (any
--    teacher / TA / owner / admin clears it for the team); fits
--    Edwin's "trust TAs but let me filter for not-yet-reviewed".
--    Together with the per-conversation latest user-message
--    timestamp it powers an "Unreviewed" tab: a conversation is
--    unreviewed iff it has never been reviewed OR a new student
--    turn arrived after `reviewed_at`. Viewing a conversation in
--    the teacher dashboard auto-upserts a row here (read == reviewed,
--    per the product call); explicit re-review is just re-opening.
--
-- 2. `conversations.student_last_viewed_at` -- per-owner read
--    marker. Cheap to colocate on `conversations` because the
--    owner is unambiguous (one student per conversation). The
--    student chat surface upserts NOW() on open; the conversation
--    list shows an unread dot when any teacher_note arrived after
--    this timestamp. Same primitive feeds the "My Courses" tile's
--    unread-count badge.
--
-- 3. `acknowledged_at` / `acknowledged_by` on `conversation_flags`
--    and `message_feedback` -- explicit "I've made a teaching
--    decision about this" channel. Today extraction-guard flags
--    are append-only and can never leave the "Needs Review" tab;
--    this is the fix. Downvotes get the same column for symmetry,
--    while keeping the existing implicit "leaving a note on a
--    downvoted message addresses it" shortcut intact (the list
--    query continues to OR the two clearing rules).
--
-- The three live in one migration because they're one product
-- change.

-- ── 1. conversation_reviews ───────────────────────────────────────

CREATE TABLE conversation_reviews (
    conversation_id UUID PRIMARY KEY REFERENCES conversations(id) ON DELETE CASCADE,
    -- Timestamp the most-recent teaching-team review happened.
    -- Upserted on every teacher open: the list query compares
    -- this to the latest user-message timestamp to decide
    -- "unreviewed" without needing per-teacher history.
    reviewed_at     TIMESTAMPTZ NOT NULL,
    -- The teacher / TA / owner / admin who most recently viewed.
    -- Surfaced verbatim in the dashboard ("reviewed by Edwin");
    -- pseudonymisation happens at the response layer for ext:
    -- viewers.
    reviewed_by     UUID NOT NULL REFERENCES users(id)
);

-- Backfill so day-one isn't a sea of "Unreviewed" rows. Seed
-- (conversation, course owner, NOW()) for every existing
-- conversation; if the owner is no longer a teacher on the course
-- they remain a valid users(id) reference (no FK violation), and
-- the next teacher view will upsert real data over the top.
INSERT INTO conversation_reviews (conversation_id, reviewed_at, reviewed_by)
SELECT c.id, NOW(), co.owner_id
FROM conversations c
JOIN courses co ON co.id = c.course_id;

-- ── 2. student_last_viewed_at ─────────────────────────────────────

ALTER TABLE conversations
    ADD COLUMN student_last_viewed_at TIMESTAMPTZ;

-- Same day-one-clean rationale: existing conversations get a NOW()
-- baseline so students don't see every old chat dotted as "new"
-- on rollout. Any teacher_note created after the migration timestamp
-- will correctly drive a fresh dot.
UPDATE conversations SET student_last_viewed_at = NOW();

-- ── 3. acknowledged_at / acknowledged_by ──────────────────────────

ALTER TABLE conversation_flags
    ADD COLUMN acknowledged_at TIMESTAMPTZ,
    ADD COLUMN acknowledged_by UUID REFERENCES users(id);

ALTER TABLE message_feedback
    ADD COLUMN acknowledged_at TIMESTAMPTZ,
    ADD COLUMN acknowledged_by UUID REFERENCES users(id);

-- Partial indexes targeting the hot "still needs attention" path:
-- list queries filter `acknowledged_at IS NULL` to compute "needs
-- review" counters and badge sets, and the partial index keeps the
-- index size bounded as historical acked rows accumulate.
CREATE INDEX conversation_flags_unacknowledged_idx
    ON conversation_flags (conversation_id)
    WHERE acknowledged_at IS NULL;

CREATE INDEX message_feedback_unacknowledged_idx
    ON message_feedback (message_id)
    WHERE acknowledged_at IS NULL AND rating = 'down';
