ALTER TABLE courses ALTER COLUMN embedding_provider SET DEFAULT 'local';
ALTER TABLE courses ALTER COLUMN embedding_model SET DEFAULT 'sentence-transformers/all-MiniLM-L6-v2';
UPDATE courses SET embedding_provider = 'local' WHERE embedding_provider = 'qdrant';
