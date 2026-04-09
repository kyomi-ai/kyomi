-- Unify knowledge files and dashboards into a single table.
--
-- Knowledge files (markdown documents in a folder tree) are migrated into the
-- dashboards table with doc_type = 'knowledge'. Folders are converted to
-- collections by application-code migration (see kyomi_knowledge::unify module).
--
-- This migration handles steps 1a, 1b, and 1d. Step 1c (folder-to-collection
-- conversion) runs as a Rust post-migration hook. Step 1e (DROP knowledge_files)
-- also runs in the Rust hook, after folders have been processed.

-- 1a. Add columns to dashboards
ALTER TABLE dashboards ADD COLUMN doc_type TEXT NOT NULL DEFAULT 'dashboard';
ALTER TABLE dashboards ADD COLUMN content_hash TEXT;
ALTER TABLE dashboards ADD COLUMN created_by TEXT;
ALTER TABLE dashboards ADD COLUMN updated_by TEXT;
CREATE INDEX idx_dashboards_doc_type ON dashboards(workspace_id, doc_type);

-- 1b. Migrate knowledge files into dashboards (files only, not folders)
INSERT INTO dashboards (dashboard_id, user_id, workspace_id, title, content, created_at, updated_at, doc_type, content_hash, created_by, updated_by)
SELECT id, COALESCE(created_by, 'system'), workspace_id, name, COALESCE(content, ''),
       created_at, updated_at, 'knowledge', content_hash, created_by, updated_by
FROM knowledge_files WHERE is_folder = false;

-- 1d. Repoint knowledge_chunks FK from knowledge_files to dashboards
ALTER TABLE knowledge_chunks DROP CONSTRAINT IF EXISTS knowledge_chunks_file_id_fkey;
ALTER TABLE knowledge_chunks RENAME COLUMN file_id TO dashboard_id;
ALTER TABLE knowledge_chunks ADD CONSTRAINT knowledge_chunks_dashboard_id_fkey
    FOREIGN KEY (dashboard_id) REFERENCES dashboards(dashboard_id) ON DELETE CASCADE;

-- Repoint knowledge_file_tables FK
ALTER TABLE knowledge_file_tables DROP CONSTRAINT IF EXISTS knowledge_file_tables_file_id_fkey;
ALTER TABLE knowledge_file_tables RENAME COLUMN file_id TO dashboard_id;
ALTER TABLE knowledge_file_tables ADD CONSTRAINT knowledge_file_tables_dashboard_id_fkey
    FOREIGN KEY (dashboard_id) REFERENCES dashboards(dashboard_id) ON DELETE CASCADE;

-- NOTE: knowledge_files table is NOT dropped here. The Rust post-migration hook
-- (step 1c) needs to read folders from it to create collections. After that,
-- the Rust hook drops the table.
