-- Add dual embedding columns to datasource_table_cache
ALTER TABLE datasource_table_cache
    ADD COLUMN name_embedding BLOB;

ALTER TABLE datasource_table_cache
    ADD COLUMN desc_embedding BLOB;

-- Create column_embeddings table for per-column embeddings
CREATE TABLE IF NOT EXISTS column_embeddings (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    table_cache_id INTEGER NOT NULL REFERENCES datasource_table_cache(id) ON DELETE CASCADE,
    workspace_id TEXT NOT NULL,
    column_name TEXT NOT NULL,
    data_type TEXT NOT NULL,
    description TEXT,
    name_embedding BLOB,
    desc_embedding BLOB,
    UNIQUE(table_cache_id, column_name)
);

CREATE INDEX IF NOT EXISTS idx_column_embeddings_workspace ON column_embeddings(workspace_id);

-- Note: HNSW/IVFFLAT vector indexes are skipped for SQLite.
-- Vector similarity search will use brute-force scan or application-layer indexing.
