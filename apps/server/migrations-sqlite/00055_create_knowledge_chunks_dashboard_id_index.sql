-- KYO-961: add the live knowledge_chunks dashboard_id index.
--
-- SQLite has no CONCURRENTLY; this transactional migration runs before the app
-- serves and supports the same delete/cascade paths. The index is non-unique,
-- so schema parity does not compare it; the parity test asserts it explicitly.
CREATE INDEX IF NOT EXISTS idx_knowledge_chunks_dashboard_id
    ON knowledge_chunks (dashboard_id);
