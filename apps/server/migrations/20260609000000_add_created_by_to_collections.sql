-- Add created_by to collections so we can track who created each collection.
-- This is part of the document visibility feature (KYO-101).
--
-- The column is added nullable first so existing rows can be backfilled, then
-- converted to NOT NULL. New rows will always have the value set by the service.
--
-- Backfill strategy: use the earliest workspace_users member (by created_at)
-- as a best-effort proxy for the workspace owner on existing collections.

ALTER TABLE collections ADD COLUMN IF NOT EXISTS created_by TEXT;

UPDATE collections SET created_by = COALESCE(
    (SELECT wu.user_id FROM workspace_users wu
     WHERE wu.workspace_id = collections.workspace_id
     ORDER BY wu.created_at ASC
     LIMIT 1),
    (SELECT user_id FROM users ORDER BY created_at ASC LIMIT 1)
) WHERE created_by IS NULL;

-- Safety check: fail loudly if any NULLs remain (e.g. zero users in the system).
DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM collections WHERE created_by IS NULL) THEN
        RAISE EXCEPTION 'collections.created_by backfill incomplete: NULL values remain';
    END IF;
END $$;

-- Add NOT NULL constraint now that all rows have been backfilled.
ALTER TABLE collections ALTER COLUMN created_by SET NOT NULL;

-- Add FK constraint to ensure referential integrity.
ALTER TABLE collections ADD CONSTRAINT collections_created_by_fkey
    FOREIGN KEY (created_by) REFERENCES users(user_id);

-- Index for fast lookups by creator.
CREATE INDEX IF NOT EXISTS idx_collections_created_by ON collections(created_by);
