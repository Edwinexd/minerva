-- Per-course cross-encoder re-ranker model.
--
-- RAG retrieval always re-ranks: every embedding candidate pool is
-- scored by a cross-encoder before the top-k chunks reach the LLM. This
-- column selects WHICH re-ranker the course uses, mirroring the
-- `embedding_model` column. It is gated by the admin-managed
-- `reranker_models` catalog (only enabled models are pickable by
-- teachers) and snapshotted from the catalog default at course-create
-- time.
--
-- Independent of the embedding model: changing the re-ranker needs no
-- re-embed (the cross-encoder reads chunk text, not vectors), so there
-- is no embedding_version-style rotation column here.
--
-- Default matches the reranker_models seed default. Existing courses
-- pick up the multilingual default automatically; teachers can switch
-- per course afterwards.
ALTER TABLE courses
    ADD COLUMN reranker_model TEXT NOT NULL
        DEFAULT 'jinaai/jina-reranker-v2-base-multilingual';
