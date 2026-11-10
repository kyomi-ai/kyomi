-- KYO-960: add the live knowledge_chunks workspace index.
--
-- SQLite has no pgvector/IVFFlat index: the search path loads a workspace's
-- chunks and computes cosine similarity in Rust, so the workspace filter is
-- the useful index there. The legacy workspace_knowledge_chunks table owned
-- this name until KYO-956 dropped it.
CREATE INDEX IF NOT EXISTS idx_knowledge_chunks_workspace
    ON knowledge_chunks (workspace_id);
