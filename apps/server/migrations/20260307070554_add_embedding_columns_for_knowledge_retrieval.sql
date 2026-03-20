-- Enable pgvector if not already enabled
CREATE EXTENSION IF NOT EXISTS vector;

-- Add dual embedding columns to datasource_table_cache
ALTER TABLE datasource_table_cache
    ADD COLUMN IF NOT EXISTS name_embedding vector(384),
    ADD COLUMN IF NOT EXISTS desc_embedding vector(384);

-- Create column_embeddings table for per-column embeddings
CREATE TABLE IF NOT EXISTS column_embeddings (
    id SERIAL PRIMARY KEY,
    table_cache_id INTEGER NOT NULL REFERENCES datasource_table_cache(id) ON DELETE CASCADE,
    workspace_id TEXT NOT NULL,
    column_name TEXT NOT NULL,
    data_type TEXT NOT NULL,
    description TEXT,
    name_embedding vector(384),
    desc_embedding vector(384),
    UNIQUE(table_cache_id, column_name)
);

CREATE INDEX IF NOT EXISTS idx_column_embeddings_workspace ON column_embeddings(workspace_id);

-- Ensure agent_learnings.embedding column exists (may already exist from baseline)
ALTER TABLE agent_learnings
    ADD COLUMN IF NOT EXISTS embedding vector(384);

-- Create HNSW indexes for fast vector search
-- Using cosine distance operator (<=>) with HNSW for approximate nearest neighbor
CREATE INDEX IF NOT EXISTS idx_table_cache_name_emb ON datasource_table_cache
    USING hnsw (name_embedding vector_cosine_ops);
CREATE INDEX IF NOT EXISTS idx_table_cache_desc_emb ON datasource_table_cache
    USING hnsw (desc_embedding vector_cosine_ops);

CREATE INDEX IF NOT EXISTS idx_col_emb_name ON column_embeddings
    USING hnsw (name_embedding vector_cosine_ops);
CREATE INDEX IF NOT EXISTS idx_col_emb_desc ON column_embeddings
    USING hnsw (desc_embedding vector_cosine_ops);

CREATE INDEX IF NOT EXISTS idx_learnings_embedding ON agent_learnings
    USING hnsw (embedding vector_cosine_ops);
