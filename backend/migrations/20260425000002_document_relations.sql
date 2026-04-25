-- Course knowledge graph V2: typed edges between documents.
--
-- The cross-doc linking pass populates this table after every doc in a
-- course has a `kind` set. Each row asserts a single typed relation
-- between two docs in the same course, with a confidence the linker
-- can use to filter at query time.
--
-- Edges are directional: `solution_of` points solution -> assignment;
-- `part_of_unit` is undirected in spirit but stored with the lower
-- doc id as `src` for dedup. The graph viewer renders both shapes.
--
-- Constraints:
--   * Both docs must live in the same course (enforced by the linker
--     query, not the schema -- a CHECK across two foreign rows would
--     need a trigger, and the application-side guard is sufficient).
--   * (src_doc_id, dst_doc_id, relation) is unique so re-running the
--     linker is idempotent: ON CONFLICT updates confidence/rationale.
--   * `relation` is constrained to the closed set the application
--     understands; new edge kinds need a migration so old code can't
--     silently ignore them.

CREATE TABLE document_relations (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    course_id UUID NOT NULL REFERENCES courses(id) ON DELETE CASCADE,
    src_doc_id UUID NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
    dst_doc_id UUID NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
    relation TEXT NOT NULL,
    confidence REAL NOT NULL,
    rationale TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CONSTRAINT document_relations_no_self_loop CHECK (src_doc_id <> dst_doc_id),
    CONSTRAINT document_relations_relation_valid CHECK (
        relation IN ('solution_of', 'part_of_unit')
    ),
    CONSTRAINT document_relations_confidence_range CHECK (
        confidence >= 0.0 AND confidence <= 1.0
    ),
    UNIQUE (src_doc_id, dst_doc_id, relation)
);

CREATE INDEX idx_document_relations_course ON document_relations (course_id);
CREATE INDEX idx_document_relations_src ON document_relations (src_doc_id);
CREATE INDEX idx_document_relations_dst ON document_relations (dst_doc_id);
