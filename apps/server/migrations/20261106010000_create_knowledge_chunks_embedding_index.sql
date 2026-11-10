-- no-transaction
-- KYO-960: restore the live knowledge_chunks embedding index.
--
-- The legacy workspace_knowledge_chunks table owned
-- `idx_knowledge_chunks_embedding` until KYO-956 dropped it, so
-- 20260412000000's `CREATE INDEX IF NOT EXISTS` on the live table was a silent
-- no-op. Semantic retrieval in crates/kyomi-agent/src/tools/knowledge.rs orders
-- by `embedding <=> $2`; without this index it sequentially scans on Postgres.
--
-- CONCURRENTLY avoids blocking reads/writes on the live table: it takes only a
-- SHARE UPDATE EXCLUSIVE lock, so existing pods keep serving. The build runs
-- inside DbPool::connect before the HTTP listener binds, so the pod's startup
-- probe budget must cover it. sqlx runs a file whose first line is
-- `-- no-transaction` outside a transaction (sqlx-core's no_tx path); inside
-- the normal migration transaction PostgreSQL rejects CONCURRENTLY. Keep this
-- file to one statement: a multi-statement simple query is an implicit
-- transaction block and would also reject CONCURRENTLY.
--
-- On a brand-new database this index is built before any chunks exist, so
-- pgvector has no samples to train the IVFFlat lists. After the table has data
-- (and after large backfills), run
-- `REINDEX INDEX CONCURRENTLY idx_knowledge_chunks_embedding` to retrain it.
--
-- If the build is interrupted it can leave an INVALID index behind;
-- 20261106010200_verify_knowledge_chunks_indexes.sql fails loudly in that case
-- instead of letting IF NOT EXISTS mask it on the next startup. These versions
-- are recorded, so a pre-KYO-960 binary fails at startup with `VersionMissing`
-- once they apply.
CREATE INDEX CONCURRENTLY IF NOT EXISTS idx_knowledge_chunks_embedding
    ON knowledge_chunks
    USING ivfflat (embedding vector_cosine_ops)
    WITH (lists = 10);
