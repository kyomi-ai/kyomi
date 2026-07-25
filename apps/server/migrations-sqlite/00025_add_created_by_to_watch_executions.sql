-- Denormalize created_by onto watch_executions so alert ownership survives
-- watch deletion. watch_executions.watch_id is ON DELETE SET NULL (the table
-- already carries watch_name/mode/workspace_id snapshot columns for the same
-- reason), so a JOIN back to watches for ownership silently drops rows once
-- the parent watch is deleted. Filtering directly on this column avoids that.

ALTER TABLE watch_executions ADD COLUMN created_by TEXT;

-- Backfill rows whose parent watch still exists. Rows whose watch_id is
-- already NULL (parent watch deleted before this migration ran) have no way
-- to recover ownership and are left with created_by = NULL — an unavoidable
-- gap for pre-existing orphaned rows, not something to work around.
UPDATE watch_executions
   SET created_by = (
       SELECT created_by FROM watches WHERE watches.watch_id = watch_executions.watch_id
   )
 WHERE watch_id IS NOT NULL;

-- Index for the alerts queries, which filter on (workspace_id, created_by).
CREATE INDEX IF NOT EXISTS idx_watch_executions_created_by ON watch_executions(created_by);
