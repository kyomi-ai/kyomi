-- no-transaction
-- KYO-960: restore the live knowledge_chunks workspace index.
--
-- The retrieval query filters by `workspace_id` (crates/kyomi-agent/src/tools/
-- knowledge.rs), but the live table had no index on it: the legacy
-- workspace_knowledge_chunks table owned this name until KYO-956 dropped it.
--
-- CONCURRENTLY and the one-statement rule are explained in
-- 20261106010000_create_knowledge_chunks_embedding_index.sql. The following
-- verify migration checks both indexes for an interrupted build.
CREATE INDEX CONCURRENTLY IF NOT EXISTS idx_knowledge_chunks_workspace
    ON knowledge_chunks (workspace_id);
