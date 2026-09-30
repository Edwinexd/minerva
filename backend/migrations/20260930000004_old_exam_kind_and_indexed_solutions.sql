-- Split the `exam` kind in two, because the chat path treats them
-- oppositely:
--
--   * `old_exam`: a past or mock exam published for practice, with or
--     without its answers. Ordinary course material: its text is
--     retrieved into context and students can be walked through it.
--   * `exam`: an examining exam the student is sitting now (take-home
--     exam). Joins `assignment_brief` and `lab_brief` as examining: its
--     text never enters context and a full solution is never given.
--
-- Until now `exam` was defined as "past exams, mock exams", so every
-- existing row carries the practice meaning and moves to `old_exam`,
-- teacher-locked ones included: the lock recorded a judgement made
-- under the old definition. A take-home exam is rare enough that a
-- teacher marks it by hand.

ALTER TABLE documents
    DROP CONSTRAINT IF EXISTS documents_kind_valid;

UPDATE documents SET kind = 'old_exam' WHERE kind = 'exam';

ALTER TABLE documents
    ADD CONSTRAINT documents_kind_valid CHECK (
        kind IS NULL OR kind IN (
            'lecture',
            'lecture_transcript',
            'reading',
            'tutorial_exercise',
            'assignment_brief',
            'sample_solution',
            'lab_brief',
            'exam',
            'old_exam',
            'syllabus',
            'unknown'
        )
    );

-- Sample solutions are now indexed like any other document, and are
-- withheld at chat time only when they solve examining material (a
-- `solution_of` edge onto an examining kind). The pipeline used to skip
-- their Qdrant upsert entirely, so send the existing ones through the
-- worker again. Re-ingest replaces a document's points, so the ones
-- that already had vectors are not duplicated.

UPDATE documents
SET status = 'pending', retry_after = NULL
WHERE kind = 'sample_solution'
  AND status = 'ready'
  AND orphaned_at IS NULL;
