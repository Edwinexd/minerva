ALTER TABLE courses ADD COLUMN embedding_provider TEXT NOT NULL DEFAULT 'openai';
ALTER TABLE courses ADD COLUMN embedding_model TEXT NOT NULL DEFAULT 'text-embedding-3-small';
