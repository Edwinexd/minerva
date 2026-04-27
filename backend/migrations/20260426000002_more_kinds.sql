-- Two more kinds the V1 enum was missing once we tested against real DSV
-- courses:
--
-- * `tutorial_exercise`; Swedish "övning". Practice/exercise material
--   that students work through but is NOT graded. Distinct from
--   `assignment_brief` (graded mandatory work). The chat path treats
--   tutorials as regular context (lecture-like) since there's no
--   academic-integrity reason to refuse to discuss them, but the
--   classification preserves the semantic distinction for teachers
--   browsing the graph and deciding what to expose.
--
-- * `lecture_transcript`; auto-generated transcript from a lecture
--   recording (play.dsv.su.se via the transcript pipeline). Lecture
--   content semantically, but messy / unstructured / lower-quality
--   text than slides or notes. Worth distinguishing so a teacher
--   reviewing kinds knows why retrieval might surface an awkward
--   block of speech-to-text.
--
-- We rebuild the CHECK constraint atomically; existing rows with valid
-- old values stay valid since the new set is a superset.

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
            'lab_brief',
            'exam',
            'syllabus',
            'unknown'
        )
    );
