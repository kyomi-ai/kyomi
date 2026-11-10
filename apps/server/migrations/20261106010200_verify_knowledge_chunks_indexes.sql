-- KYO-960: fail loudly if a concurrent index build was interrupted.
--
-- `CREATE INDEX CONCURRENTLY IF NOT EXISTS` treats an existing INVALID index
-- (what a cancelled or crashed build leaves behind) as already done, and then
-- records its migration. It also accepts any relation with the expected name,
-- which is the exact name-collision bug this ticket fixes. This transactional
-- migration runs after both builds and refuses to let the chain finish while
-- either index is missing, invalid, or not on knowledge_chunks.
--
-- Recovery: drop the bad index with DROP INDEX CONCURRENTLY, delete the matching
-- create-migration row from _sqlx_migrations (20261106010000 for
-- idx_knowledge_chunks_embedding, 20261106010100 for
-- idx_knowledge_chunks_workspace), and restart so the create migration runs
-- again and rebuilds it. A plain DROP INDEX is sufficient for an invalid index
-- because queries cannot use it.
DO $$
DECLARE
    missing text;
    invalid text;
BEGIN
    SELECT string_agg(t.name, ', ') INTO missing
    FROM unnest(ARRAY[
        'idx_knowledge_chunks_embedding',
        'idx_knowledge_chunks_workspace'
    ]) AS t(name)
    WHERE NOT EXISTS (
        SELECT 1
        FROM pg_index i
        JOIN pg_class c ON c.oid = i.indexrelid
        WHERE c.relname = t.name
          AND i.indrelid = 'knowledge_chunks'::regclass
    );

    IF missing IS NOT NULL THEN
        RAISE EXCEPTION 'KYO-960: missing knowledge_chunks index(es): % (not present on knowledge_chunks); drop any same-named index on another table and, if the create migration is recorded, delete its _sqlx_migrations row and restart to rebuild', missing;
    END IF;

    SELECT string_agg(c.relname, ', ') INTO invalid
    FROM pg_index i
    JOIN pg_class c ON c.oid = i.indexrelid
    WHERE c.relname IN (
        'idx_knowledge_chunks_embedding',
        'idx_knowledge_chunks_workspace'
    )
      AND i.indrelid = 'knowledge_chunks'::regclass
      AND NOT i.indisvalid;

    IF invalid IS NOT NULL THEN
        RAISE EXCEPTION 'KYO-960: invalid knowledge_chunks index(es): %; drop each with DROP INDEX CONCURRENTLY, delete its create-migration row (20261106010000 for embedding, 20261106010100 for workspace), then restart to rebuild', invalid;
    END IF;
END $$;
