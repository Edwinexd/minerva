-- Document classification becomes its own feature flag, `document_kinds`,
-- split out of `course_kg`. The knowledge graph and the extraction guard
-- both build on it; the guard no longer needs the graph.
--
-- Every row that has `course_kg` today gets a matching `document_kinds`
-- row, so those courses keep classifying. A course with only
-- `extraction_guard` is left as it is: its documents have never been
-- classified, and switching classification on holds every unclassified
-- document out of chat until a backfill has run, which is an admin's
-- call to make per course.

INSERT INTO feature_flags (id, flag, course_id, user_id, enabled)
SELECT gen_random_uuid(), 'document_kinds', course_id, user_id, enabled
FROM feature_flags
WHERE flag = 'course_kg';

-- `graded_solution`: a solution to graded work, withheld from chat.
-- `sample_solution` now means answers to practice material, which are
-- ordinary context. Which of the two a solution is was decided by the
-- graph (a `solution_of` edge onto graded work); it is now the
-- document's own kind, set by the classifier and overridable by the
-- teacher like any other.

ALTER TABLE documents
    DROP CONSTRAINT IF EXISTS documents_kind_valid;

ALTER TABLE documents
    ADD CONSTRAINT documents_kind_valid CHECK (
        kind IS NULL OR kind IN (
            'lecture',
            'lecture_transcript',
            'reading',
            'tutorial_exercise',
            'assignment_brief',
            'sample_solution',
            'graded_solution',
            'lab_brief',
            'exam',
            'old_exam',
            'syllabus',
            'unknown'
        )
    );

-- Carry the existing judgements over. A solution the graph pairs with
-- graded work is a graded solution. So is one a teacher locked as
-- `sample_solution`: that lock was made when the kind meant "never
-- shown", and it keeps meaning that. The rest were requeued by the
-- previous migration and are reclassified by the worker under the new
-- definitions.

UPDATE documents d
SET kind = 'graded_solution'
WHERE d.kind = 'sample_solution'
  AND (
      d.kind_locked_by_teacher
      OR EXISTS (
          SELECT 1
          FROM document_relations r
          JOIN documents dst ON dst.id = r.dst_doc_id
          WHERE r.src_doc_id = d.id
            AND r.relation = 'solution_of'
            AND r.rejected_by_teacher = FALSE
            AND dst.kind IN ('assignment_brief', 'lab_brief', 'exam')
      )
  );
