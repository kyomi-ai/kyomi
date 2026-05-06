CREATE TABLE IF NOT EXISTS knowledge_files (
    id TEXT NOT NULL PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    parent_id TEXT REFERENCES knowledge_files(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    is_folder INTEGER NOT NULL DEFAULT 0,
    content TEXT,
    content_hash TEXT,
    sort_order INTEGER NOT NULL DEFAULT 0,
    created_by TEXT,
    updated_by TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE UNIQUE INDEX IF NOT EXISTS knowledge_files_root_name_unique
    ON knowledge_files (workspace_id, name)
    WHERE parent_id IS NULL;

CREATE UNIQUE INDEX IF NOT EXISTS knowledge_files_child_name_unique
    ON knowledge_files (workspace_id, parent_id, name)
    WHERE parent_id IS NOT NULL;

CREATE TABLE IF NOT EXISTS knowledge_chunks (
    id TEXT NOT NULL PRIMARY KEY,
    file_id TEXT NOT NULL REFERENCES knowledge_files(id) ON DELETE CASCADE,
    workspace_id TEXT NOT NULL,
    content TEXT NOT NULL,
    chunk_index INTEGER NOT NULL,
    embedding BLOB NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS knowledge_file_tables (
    file_id TEXT NOT NULL REFERENCES knowledge_files(id) ON DELETE CASCADE,
    workspace_id TEXT NOT NULL,
    table_full_name TEXT NOT NULL,
    PRIMARY KEY (file_id, table_full_name)
);
