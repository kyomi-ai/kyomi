-- SPDX-License-Identifier: AGPL-3.0-or-later
-- Add created_by to collections for document visibility (KYO-101).
-- SQLite does not support ALTER COLUMN ... SET NOT NULL or named FK constraints,
-- so we add the column with NOT NULL DEFAULT '' and backfill immediately.

ALTER TABLE collections ADD COLUMN created_by TEXT NOT NULL DEFAULT '';

UPDATE collections SET created_by = (
    SELECT wu.user_id FROM workspace_users wu
    WHERE wu.workspace_id = collections.workspace_id
    ORDER BY wu.created_at ASC
    LIMIT 1
) WHERE created_by = '';

CREATE INDEX IF NOT EXISTS idx_collections_created_by ON collections(created_by);
