-- Course knowledge graph V1: per-document kind classification.
--
-- `kind` is set by an LLM classifier at ingest time (or backfill), and may
-- be overridden by a teacher. When `kind_locked_by_teacher = TRUE`, the
-- classifier never overwrites it; both the ingest hook and the
-- `documents::set_classification` query enforce this.
--
-- Behavior wired up in the application layer:
--   * `sample_solution`            -> never embedded into Qdrant; chunks
--                                     purged on reclassification.
--   * `assignment_brief|lab_brief|exam`
--                                  -> embedded for similarity matching but
--                                     NEVER injected into the prompt
--                                     context (used as a refusal trigger).
--   * `lecture|reading|syllabus`   -> normal RAG context.
--   * `unknown` or NULL classified_at
--                                  -> excluded from prompt context this
--                                     turn (defensive; classification is
--                                     usually fast enough that this only
--                                     bites during the brief race window
--                                     between upload and classify).

ALTER TABLE documents
    ADD COLUMN kind TEXT,
    ADD COLUMN kind_confidence REAL,
    ADD COLUMN kind_rationale TEXT,
    ADD COLUMN kind_locked_by_teacher BOOLEAN NOT NULL DEFAULT FALSE,
    ADD COLUMN classified_at TIMESTAMPTZ;

ALTER TABLE documents
    ADD CONSTRAINT documents_kind_valid CHECK (
        kind IS NULL OR kind IN (
            'lecture',
            'reading',
            'assignment_brief',
            'sample_solution',
            'lab_brief',
            'exam',
            'syllabus',
            'unknown'
        )
    );

ALTER TABLE documents
    ADD CONSTRAINT documents_kind_confidence_range CHECK (
        kind_confidence IS NULL
        OR (kind_confidence >= 0.0 AND kind_confidence <= 1.0)
    );

CREATE INDEX idx_documents_course_kind ON documents (course_id, kind);
