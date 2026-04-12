-- Fix knowledge_chunks.embedding column type on Postgres.
--
-- Migration 20260315100000 originally created the column as BYTEA, which
-- matches the SQLite write path (f32 LE bytes) but is incompatible with the
-- Postgres write path in dashboard_service::rechunk_document, which binds a
-- pgvector::Vector via $N::vector. Every Postgres write to knowledge_chunks
-- failed with `column "embedding" is of type bytea but expression is of type
-- vector`, so the table is guaranteed empty on every Postgres deployment —
-- nothing ever successfully inserted. That makes DROP+ADD safe (a direct
-- ALTER TYPE cast from bytea to vector is not supported by pgvector).
--
-- Idempotent: if the column has already been migrated (e.g. on a fresh dev
-- DB seeded by an out-of-band fix), this is a no-op.

DO $$
BEGIN
    IF EXISTS (
        SELECT 1
        FROM information_schema.columns
        WHERE table_name = 'knowledge_chunks'
          AND column_name = 'embedding'
          AND udt_name = 'bytea'
    ) THEN
        ALTER TABLE knowledge_chunks DROP COLUMN embedding;
        ALTER TABLE knowledge_chunks ADD COLUMN embedding vector(384) NOT NULL;
    END IF;
END $$;

CREATE INDEX IF NOT EXISTS idx_knowledge_chunks_embedding
    ON knowledge_chunks
    USING ivfflat (embedding vector_cosine_ops)
    WITH (lists = 10);
