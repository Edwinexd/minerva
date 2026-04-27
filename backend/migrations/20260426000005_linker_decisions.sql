-- Per-pair LLM decision cache for the cross-doc linker.
--
-- Without this, every relink re-asks gpt-oss-120b about every
-- candidate pair in the course, even when neither endpoint has
-- changed since last evaluation; expensive in tokens and
-- pointless (the answer doesn't change without input changing).
--
-- This table records what the model decided for every pair we've
-- ever asked about, INCLUDING "none" (no relation); which the
-- positive-only `document_relations` table has no way to
-- represent. The linker reads cached decisions BEFORE deciding
-- whether to call the LLM:
--
--   * cache hit, both endpoints' classified_at <= a/b_classified_at:
--       skip the LLM call, reuse the existing edge in
--       document_relations (or skip entirely for "none")
--   * cache miss OR an endpoint has been re-classified since:
--       call the LLM, upsert the decision here, then upsert/
--       delete the edge in document_relations
--
-- Schema notes:
--   * (a_doc_id, b_doc_id) is the primary key, normalised to
--     a_doc_id < b_doc_id so each unordered pair has exactly one
--     decision row regardless of how the linker sees it. The
--     decision's `relation` column captures the canonical relation
--     (or NULL = "none"); for directional relations (solution_of,
--     applied_in, prerequisite_of) the actual src/dst lives on the
--     document_relations row, not here; this table is purely a
--     "did we ask about this pair, what was the answer" log.
--   * `a_classified_at` / `b_classified_at` are snapshots of the
--     two docs' classified_at at decision time. The cache is
--     invalid when either has moved forward.
--   * Cascading deletes: removing a doc cascades to remove its
--     decisions (FK ON DELETE CASCADE), so a doc-delete naturally
--     re-triggers LLM evaluation for any new pairs that involved
--     it; which is fine because new candidates won't include
--     that doc anyway.

CREATE TABLE linker_decisions (
    course_id        UUID NOT NULL REFERENCES courses(id)   ON DELETE CASCADE,
    a_doc_id         UUID NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
    b_doc_id         UUID NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
    decided_at       TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    a_classified_at  TIMESTAMPTZ,
    b_classified_at  TIMESTAMPTZ,
   ; Canonical decision string. NULL means the model said "no
   ; relation" / "none"; recorded so we don't re-ask. Non-NULL
   ; values mirror the document_relations.relation enum.
    relation         TEXT,
   ; Confidence the model assigned. NULL when relation IS NULL.
    confidence       REAL,
    PRIMARY KEY (a_doc_id, b_doc_id),
    CHECK (a_doc_id < b_doc_id),
    CHECK (
        relation IS NULL
        OR relation IN (
            'solution_of',
            'part_of_unit',
            'prerequisite_of',
            'applied_in'
        )
    ),
    CHECK (confidence IS NULL OR (confidence >= 0.0 AND confidence <= 1.0))
);

CREATE INDEX linker_decisions_course_idx ON linker_decisions (course_id);
