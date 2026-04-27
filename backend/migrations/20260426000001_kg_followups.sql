-- KG follow-ups: persist the relink dirty queue and add per-edge teacher
-- rejections.
--
-- Why a real table for the relink queue: previously the dirty set was an
-- in-memory HashMap, which (a) silently lost queued courses on restart,
-- and (b) had no max-defer cap so a long Moodle sync could push the run
-- time forward indefinitely and the linker would never fire. With this
-- table we persist `first_marked_at` (= when the course first became
-- dirty since its last relink) and cap the wait to first_marked_at +
-- MAX_PENDING_AGE in the application layer.
--
-- One row per course; mark = INSERT ... ON CONFLICT update; take_due =
-- SELECT due_at <= NOW() then DELETE.

CREATE TABLE IF NOT EXISTS relink_queue (
    course_id        UUID PRIMARY KEY REFERENCES courses(id) ON DELETE CASCADE,
   ; Wall-clock instant the course was first marked dirty since its
   ; last successful relink. Used to cap how far the debounce pushes
   ; due_at into the future.
    first_marked_at  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
   ; Earliest moment the linker may run for this course. Pushed back
   ; by mark_dirty up to a hard cap of first_marked_at + MAX_PENDING_AGE
   ; so a sustained burst doesn't starve the linker.
    due_at           TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS relink_queue_due_at_idx ON relink_queue (due_at);

-- Per-edge teacher rejection. The linker re-runs idempotently and
-- replaces the edge set on every pass, so we'd lose teacher veto
-- signals across reruns without persisting them somewhere stable.
--
-- Two parts:
--   1. `document_relations.rejected_by_teacher`; keeps a row visible
--      to admins/audit even after the linker would have cleaned it up,
--      and tells the graph viewer to hide it.
--   2. `rejected_edge_pairs`; a separate table keyed by the
--      DIRECTIONAL pair + relation, so the next linker pass can SKIP
--      proposing the pair entirely. Directional matters for solution_of
--      (src and dst aren't interchangeable) but for part_of_unit we
--      always normalize src < dst at write time, mirroring the linker.
ALTER TABLE document_relations
    ADD COLUMN IF NOT EXISTS rejected_by_teacher BOOLEAN NOT NULL DEFAULT FALSE,
    ADD COLUMN IF NOT EXISTS rejected_at         TIMESTAMPTZ,
    ADD COLUMN IF NOT EXISTS rejected_by         UUID REFERENCES users(id) ON DELETE SET NULL;

CREATE INDEX IF NOT EXISTS document_relations_rejected_idx
    ON document_relations (course_id, rejected_by_teacher)
    WHERE rejected_by_teacher = TRUE;

CREATE TABLE IF NOT EXISTS rejected_edge_pairs (
    course_id   UUID NOT NULL REFERENCES courses(id) ON DELETE CASCADE,
    src_doc_id  UUID NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
    dst_doc_id  UUID NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
    relation    TEXT NOT NULL CHECK (relation IN ('solution_of', 'part_of_unit')),
    rejected_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    rejected_by UUID REFERENCES users(id) ON DELETE SET NULL,
    PRIMARY KEY (src_doc_id, dst_doc_id, relation)
);

CREATE INDEX IF NOT EXISTS rejected_edge_pairs_course_idx
    ON rejected_edge_pairs (course_id);
