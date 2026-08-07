-- KYO-267: move catalog refresh state from workspace scope to datasource scope.
--
-- `workspaces.catalog_refresh_status`/`catalog_refresh_progress` are shared
-- by every datasource in a workspace. Two different datasources in the same
-- workspace can refresh concurrently (`index_started_within` in
-- `kyomi-auth/src/catalog/helpers.rs` keys off `datasource_config_id`, not
-- `workspace_id`), so datasource B finishing successfully writes `'idle'`
-- over datasource A's `'failed'` + reason. There is no history table --
-- nothing records that A ever failed, and A's Catalog tab shows clean. This
-- is the KYO-126 silent-success bug reintroduced by a different mechanism
-- (see `update_workspace_status`'s now-removed "Known limitation" doc
-- comment for the full history).
--
-- Same names/types/default as the workspace originals, so
-- `impl_sqlx_varchar_enum!(CatalogRefreshStatus)` (crates/kyomi-core/src/enums.rs)
-- keeps working unchanged against the new column.
ALTER TABLE datasource_configs
    ADD COLUMN catalog_refresh_status VARCHAR(50) DEFAULT 'idle';

COMMENT ON COLUMN datasource_configs.catalog_refresh_status IS 'Current status of this datasource''s catalog refresh: idle, running, failed';

ALTER TABLE datasource_configs
    ADD COLUMN catalog_refresh_progress json;

COMMENT ON COLUMN datasource_configs.catalog_refresh_progress IS 'JSON object tracking refresh progress: {datasource_config_id, updated_at, progress, error}';

CREATE INDEX IF NOT EXISTS idx_datasource_configs_catalog_status ON datasource_configs USING btree (catalog_refresh_status);

-- Targeted backfill, not a blanket copy: only the one datasource named
-- inside the workspace's existing progress envelope receives the
-- workspace's current status/progress. Copying to every datasource in the
-- workspace would fabricate a failure on datasources that never failed --
-- exactly the confusion this migration removes. This is what preserves a
-- currently-displayed failure alert across the deploy; without it, an
-- admin looking at a live failure would silently stop seeing it until the
-- next refresh runs.
--
-- `dc.id = w.catalog_refresh_progress->>'datasource_config_id'` naturally
-- no-ops for workspaces with a NULL envelope (comparison against NULL is
-- unknown) and for any envelope whose `datasource_config_id` doesn't match
-- a live row (e.g. a since-deleted datasource) -- no explicit guard needed.
UPDATE datasource_configs dc
   SET catalog_refresh_status = w.catalog_refresh_status,
       catalog_refresh_progress = w.catalog_refresh_progress
  FROM workspaces w
 WHERE dc.workspace_id = w.workspace_id
   AND dc.id = w.catalog_refresh_progress ->> 'datasource_config_id';

-- Drop the old workspace-scoped columns now that every live envelope has
-- been copied to its owning datasource. Dropping the index explicitly
-- first rather than relying on cascade -- keeps behavior identical to the
-- SQLite side, which has no automatic cascade for this.
DROP INDEX IF EXISTS idx_workspaces_catalog_status;

ALTER TABLE workspaces DROP COLUMN IF EXISTS catalog_refresh_status;
ALTER TABLE workspaces DROP COLUMN IF EXISTS catalog_refresh_progress;
