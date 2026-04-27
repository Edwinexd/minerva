-- Course knowledge graph: per-document embedding vectors used by the
-- cross-doc linker for embedding-based candidate generation.
--
-- The pooled embedding is the mean of all chunk embeddings for the
-- document under whatever embedding model the course is configured
-- with. Stored as REAL[] (postgres array of floats); pgvector is
-- not installed in this deployment and we don't need DB-side
-- similarity search; the linker computes pairwise cosine in memory.
--
-- Population:
--   * Filled at ingest time by the pipeline once chunks are embedded.
--   * Lazily backfilled by the linker on first run for docs whose
--     pooled_embedding is NULL (it scrolls Qdrant for the doc's
--     chunks and mean-pools).
--   * Kept up-to-date by re-ingest (reset_for_resync clears it).
--   * For `sample_solution` docs, we still compute the pooled
--     embedding so the linker can find their `solution_of` partner
--     even though the chunks themselves are not stored in Qdrant.

ALTER TABLE documents
    ADD COLUMN pooled_embedding REAL[];

-- No CHECK on length: different courses use different embedding
-- models with different dimensions (OpenAI text-embedding-3-small
-- is 1536, BGE-base is 768, MiniLM is 384). Validating dimension
-- consistency happens at the application layer where we know the
-- expected dim from the course config.
