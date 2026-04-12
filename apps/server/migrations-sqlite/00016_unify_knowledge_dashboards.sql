-- Unify knowledge files and dashboards into a single table.
-- See PostgreSQL migration 20260409000000 for full description.

-- 1a. Add columns to dashboards
ALTER TABLE dashboards ADD COLUMN doc_type TEXT NOT NULL DEFAULT 'dashboard';
ALTER TABLE dashboards ADD COLUMN content_hash TEXT;
ALTER TABLE dashboards ADD COLUMN created_by TEXT;
ALTER TABLE dashboards ADD COLUMN updated_by TEXT;

-- 1b. Migrate knowledge files into dashboards (files only, not folders)
INSERT INTO dashboards (dashboard_id, user_id, workspace_id, title, content, created_at, updated_at, doc_type, content_hash, created_by, updated_by)
SELECT id, COALESCE(created_by, 'system'), workspace_id, name, COALESCE(content, ''),
       created_at, updated_at, 'knowledge', content_hash, created_by, updated_by
FROM knowledge_files WHERE is_folder = 0;

-- 1d. Repoint knowledge_chunks (SQLite: create-copy-drop-rename)
CREATE TABLE knowledge_chunks_new (
    id TEXT NOT NULL PRIMARY KEY,
    dashboard_id TEXT NOT NULL REFERENCES dashboards(dashboard_id) ON DELETE CASCADE,
    workspace_id TEXT NOT NULL,
    content TEXT NOT NULL,
    chunk_index INTEGER NOT NULL,
    embedding BLOB NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);
INSERT INTO knowledge_chunks_new (id, dashboard_id, workspace_id, content, chunk_index, embedding, created_at)
SELECT id, file_id, workspace_id, content, chunk_index, embedding, created_at FROM knowledge_chunks;
DROP TABLE knowledge_chunks;
ALTER TABLE knowledge_chunks_new RENAME TO knowledge_chunks;

-- Repoint knowledge_file_tables (SQLite: create-copy-drop-rename)
CREATE TABLE knowledge_file_tables_new (
    dashboard_id TEXT NOT NULL REFERENCES dashboards(dashboard_id) ON DELETE CASCADE,
    workspace_id TEXT NOT NULL,
    table_full_name TEXT NOT NULL,
    PRIMARY KEY (dashboard_id, table_full_name)
);
INSERT INTO knowledge_file_tables_new (dashboard_id, workspace_id, table_full_name)
SELECT file_id, workspace_id, table_full_name FROM knowledge_file_tables;
DROP TABLE knowledge_file_tables;
ALTER TABLE knowledge_file_tables_new RENAME TO knowledge_file_tables;

-- Index for doc_type filtering (match PostgreSQL migration)
CREATE INDEX idx_dashboards_doc_type ON dashboards(workspace_id, doc_type);

-- NOTE: knowledge_files table is NOT dropped here. The Rust post-migration hook
-- reads folders to create collections, then drops the table.
