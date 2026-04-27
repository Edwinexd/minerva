-- Two more relation kinds for the course knowledge graph.
--
-- The original V1 had only `solution_of` (solution -> assessment) and
-- `part_of_unit` (same-unit cluster). On real DSV courses that turns
-- out to be too thin: most pairs the LLM finds related don't fit
-- either bucket cleanly, so they all get squeezed into "part_of_unit"
-- (27 of 30 edges in a recent test) regardless of whether they're
-- actually unit-mates.
--
-- New relations:
--
--   * `prerequisite_of(src, dst)`; src introduces concepts dst
--     builds on. Directional. Use for foundational lecture/reading
--     that a later lecture/exercise/assessment relies on.
--
--   * `applied_in(src, dst)`; theoretical content (lecture,
--     reading, lecture_transcript) is APPLIED in a practical doc
--     (tutorial_exercise, assignment_brief, lab_brief, exam).
--     Directional from theory to practice. Lets the chat path
--     surface "the lab where this concept is exercised" when a
--     student asks about a topic, and lets teachers see the
--     theory-to-practice flow at a glance.
--
-- Both are directional. The linker derives direction from the LLM's
-- per-pair `a_*` / `b_*` discriminator (see `classify_one_pair`).
--
-- We rebuild the constraint atomically. Existing rows keep validity
-- since the new set is a superset of the old.

ALTER TABLE document_relations
    DROP CONSTRAINT IF EXISTS document_relations_relation_valid;

ALTER TABLE document_relations
    ADD CONSTRAINT document_relations_relation_valid CHECK (
        relation IN (
            'solution_of',
            'part_of_unit',
            'prerequisite_of',
            'applied_in'
        )
    );

ALTER TABLE rejected_edge_pairs
    DROP CONSTRAINT IF EXISTS rejected_edge_pairs_relation_check;

-- The original CHECK was defined inline (no name), so the drop above
-- targets the conventional name `pg` would generate. If it doesn't
-- exist (older schemas), DROP IF EXISTS is a no-op. Re-add with the
-- expanded set explicitly named so future migrations can find it.
ALTER TABLE rejected_edge_pairs
    DROP CONSTRAINT IF EXISTS rejected_edge_pairs_relation_check1;

-- Postgres auto-named the original inline check `rejected_edge_pairs_relation_check`
-- (since the column is `relation`); guard against both possibilities by
-- dropping the inline form too and re-adding under a stable name.
DO $$
DECLARE
    cname text;
BEGIN
    FOR cname IN
        SELECT conname FROM pg_constraint
         WHERE conrelid = 'rejected_edge_pairs'::regclass
           AND contype = 'c'
           AND pg_get_constraintdef(oid) ILIKE '%relation%IN%'
    LOOP
        EXECUTE format('ALTER TABLE rejected_edge_pairs DROP CONSTRAINT %I', cname);
    END LOOP;
END $$;

ALTER TABLE rejected_edge_pairs
    ADD CONSTRAINT rejected_edge_pairs_relation_valid CHECK (
        relation IN (
            'solution_of',
            'part_of_unit',
            'prerequisite_of',
            'applied_in'
        )
    );
