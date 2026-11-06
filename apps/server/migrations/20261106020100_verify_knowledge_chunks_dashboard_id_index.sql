-- KYO-961: fail loudly if the concurrent dashboard_id index build was interrupted.
--
-- `CREATE INDEX CONCURRENTLY IF NOT EXISTS` treats an existing INVALID index
-- (what a cancelled or crashed build leaves behind) as already done, and then
-- records its migration. It also accepts any relation with the expected name.
-- This transactional migration runs after the build and refuses to let the
-- chain finish while the index is missing, invalid, or not on knowledge_chunks.
--
-- Recovery: stop the new replicas (so they cannot restart-loop mid-repair),
-- drop the bad index with DROP INDEX CONCURRENTLY (a plain DROP INDEX takes
-- ACCESS EXCLUSIVE on the live table), delete the matching create-migration row
-- from _sqlx_migrations (20261106020000 for idx_knowledge_chunks_dashboard_id),
-- and restart so the create migration runs again and rebuilds it.
DO $$
DECLARE
    missing text;
    invalid text;
BEGIN
    SELECT string_agg(t.name, ', ') INTO missing
    FROM unnest(ARRAY['idx_knowledge_chunks_dashboard_id']) AS t(name)
    WHERE NOT EXISTS (
        SELECT 1
        FROM pg_index i
        JOIN pg_class c ON c.oid = i.indexrelid
        WHERE c.relname = t.name
          AND i.indrelid = 'knowledge_chunks'::regclass
    );

    IF missing IS NOT NULL THEN
        RAISE EXCEPTION 'KYO-961: missing knowledge_chunks index(es): % (not present on knowledge_chunks); stop the new replicas, drop any same-named index on another table and, if the create migration is recorded, delete its _sqlx_migrations row, then restart to rebuild', missing;
    END IF;

    SELECT string_agg(c.relname, ', ') INTO invalid
    FROM pg_index i
    JOIN pg_class c ON c.oid = i.indexrelid
    WHERE c.relname IN ('idx_knowledge_chunks_dashboard_id')
      AND i.indrelid = 'knowledge_chunks'::regclass
      AND NOT i.indisvalid;

    IF invalid IS NOT NULL THEN
        RAISE EXCEPTION 'KYO-961: invalid knowledge_chunks index(es): %; stop the new replicas, drop each with DROP INDEX CONCURRENTLY, delete its create-migration row (20261106020000 for dashboard_id), then restart to rebuild', invalid;
    END IF;
END $$;
