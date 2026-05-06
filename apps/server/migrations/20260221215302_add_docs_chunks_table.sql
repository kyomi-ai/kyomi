-- Product documentation chunks for agent search_docs tool.
-- Global table (not per-workspace) — product docs are the same for all users.
-- Indexed at docs deploy time by scripts/index-docs.py.

CREATE TABLE docs_chunks (
    id SERIAL PRIMARY KEY,
    file_path TEXT NOT NULL,
    chunk_index INTEGER NOT NULL,
    chunk_text TEXT NOT NULL,
    embedding VECTOR(384) NOT NULL,
    file_hash TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE(file_path, chunk_index)
);

CREATE INDEX idx_docs_chunks_embedding ON docs_chunks
    USING ivfflat (embedding vector_cosine_ops) WITH (lists = 10);
