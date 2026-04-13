-- Per-course minimum cosine-similarity score for RAG chunk inclusion. Chunks
-- below this threshold are dropped before being added to the LLM context.
-- 0.0 = no filter (legacy behavior, top-k by max_chunks only). Constrained
-- to the non-negative range since the embedding models in use normalize
-- positives only and a negative threshold would be meaningless.
ALTER TABLE courses
    ADD COLUMN min_score REAL NOT NULL DEFAULT 0.0
        CHECK (min_score >= 0.0 AND min_score <= 1.0);
