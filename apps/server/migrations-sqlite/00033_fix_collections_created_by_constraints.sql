-- SPDX-License-Identifier: AGPL-3.0-or-later
-- Fix collections.created_by to match Postgres (KYO-293).
--
-- 00022_add_created_by_to_collections.sql declared the column
-- `NOT NULL DEFAULT ''` and never added a foreign key, because SQLite can't
-- `ALTER COLUMN ... SET NOT NULL`/drop a default or add a named FK to an
-- existing column. That left two divergences from
-- 20260609000000_add_created_by_to_collections.sql (Postgres):
--   1. An INSERT omitting created_by silently stored '' instead of failing.
--   2. A created_by naming a nonexistent user was never rejected — no FK.
--
-- Both require a full table rebuild (create-copy-drop-rename), since that's
-- the only way SQLite can drop a column default or add a REFERENCES clause.
-- That rebuild is dangerous on its own: collection_dashboards has
-- `FOREIGN KEY (collection_id) REFERENCES collections(id) ON DELETE CASCADE`,
-- and SQLite fires that cascade during the DROP TABLE's implicit delete —
-- wiping every collection_dashboards row with no error. So its rows are
-- backed up to a temp table first and restored after the rename, all inside
-- this migration's transaction (no `-- no-transaction`: a partial rebuild
-- left uncommitted on failure is worse than the whole thing rolling back).
--
-- Backfill mirrors Postgres's own migration exactly: earliest
-- workspace_users member by created_at, COALESCEd to the earliest user
-- overall if the workspace has no members. 00022 only did the first half,
-- with no COALESCE fallback and no safety check — its own
-- `UPDATE ... SET created_by = (subquery)` yields NULL (rejected by its own
-- NOT NULL) for a memberless workspace, a second, unticketed divergence
-- fixed here. If a row still can't resolve after both COALESCE steps (which
-- requires zero users in the system — a database that could not have
-- legitimately created a collection in the first place), it stays '' and
-- the FK below rejects it when copying into the rebuilt table, aborting the
-- migration loudly. No sentinel user is invented and no rows are deleted.

-- 1. Preserve collection_dashboards rows across the collections rebuild.
CREATE TEMP TABLE cd_backup_00033 AS SELECT * FROM collection_dashboards;

-- 2. Backfill any '' rows left by 00022, using the same two-step COALESCE
--    Postgres used, before the rebuild enforces NOT NULL + FK below.
UPDATE collections SET created_by = COALESCE(
    (SELECT wu.user_id FROM workspace_users wu
     WHERE wu.workspace_id = collections.workspace_id
     ORDER BY wu.created_at ASC
     LIMIT 1),
    (SELECT user_id FROM users ORDER BY created_at ASC LIMIT 1)
) WHERE created_by = '';

-- 3. Rebuild collections: created_by is now NOT NULL with no default, and
--    references users(user_id) — matching Postgres's
--    collections_created_by_fkey. Every other column, the workspace_id FK,
--    UNIQUE(workspace_id, name), and column order are unchanged.
CREATE TABLE collections_new (
    id TEXT NOT NULL PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id),
    name TEXT NOT NULL,
    description TEXT,
    color TEXT,
    is_public INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now')),
    created_by TEXT NOT NULL REFERENCES users(user_id),
    doc_type TEXT NOT NULL DEFAULT 'dashboard',
    UNIQUE(workspace_id, name)
);

-- This INSERT is itself FK-validated against users(user_id) — a free
-- integrity check on the backfill above, and the mechanism that aborts the
-- migration loudly if any row still can't resolve a real user.
INSERT INTO collections_new (id, workspace_id, name, description, color, is_public, created_at, updated_at, created_by, doc_type)
SELECT id, workspace_id, name, description, color, is_public, created_at, updated_at, created_by, doc_type
FROM collections;

DROP TABLE collections;

ALTER TABLE collections_new RENAME TO collections;

-- 4. Recreate every index DROP TABLE removed (from 00001_baseline.sql,
--    00022_add_created_by_to_collections.sql, and
--    00026_collection_doc_type.sql).
CREATE INDEX IF NOT EXISTS idx_collections_is_public ON collections(is_public);
CREATE INDEX IF NOT EXISTS idx_collections_workspace_id ON collections(workspace_id);
CREATE INDEX IF NOT EXISTS idx_collections_created_by ON collections(created_by);
CREATE INDEX IF NOT EXISTS idx_collections_doc_type ON collections(workspace_id, doc_type);

-- 5. Restore collection_dashboards rows — also FK-validated against the
--    rebuilt collections table.
INSERT INTO collection_dashboards SELECT * FROM cd_backup_00033;

DROP TABLE cd_backup_00033;
