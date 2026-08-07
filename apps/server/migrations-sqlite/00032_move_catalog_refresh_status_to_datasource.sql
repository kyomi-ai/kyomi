-- KYO-267: move catalog refresh state from workspace scope to datasource scope.
--
-- See migrations/20260807000000_move_catalog_refresh_status_to_datasource.sql
-- for the full rationale (KYO-126 silent-success bug reintroduced by
-- workspace-scoped concurrency).
--
-- SQLite stores the JSON envelope as TEXT (via sqlx), same as every other
-- JSON-shaped column in this schema.
ALTER TABLE datasource_configs
    ADD COLUMN catalog_refresh_status TEXT DEFAULT 'idle';

ALTER TABLE datasource_configs
    ADD COLUMN catalog_refresh_progress TEXT;

CREATE INDEX IF NOT EXISTS idx_datasource_configs_catalog_status ON datasource_configs(catalog_refresh_status);

-- Targeted backfill (see the Postgres migration for the full rationale):
-- only the one datasource named inside the workspace's existing progress
-- envelope receives the workspace's current status/progress. `UPDATE ...
-- FROM` requires SQLite >= 3.33 (this project bundles a much newer
-- libsqlite3-sys via sqlx). json_extract on a NULL envelope returns NULL,
-- which never equals `datasource_configs.id`, so workspaces with no
-- envelope (or one naming a since-deleted datasource) naturally no-op.
UPDATE datasource_configs
   SET catalog_refresh_status = w.catalog_refresh_status,
       catalog_refresh_progress = w.catalog_refresh_progress
  FROM workspaces w
 WHERE datasource_configs.workspace_id = w.workspace_id
   AND datasource_configs.id = json_extract(w.catalog_refresh_progress, '$.datasource_config_id');

-- Drop the old workspace-scoped columns and index. SQLite's ALTER TABLE
-- DROP COLUMN refuses to run while a dependent index still references the
-- column (verified directly against this project's bundled SQLite: "error
-- in index idx_workspaces_catalog_status after drop column: no such
-- column"), so the index must be dropped first -- there is no automatic
-- cascade the way Postgres has for a plain (non-constraint) index.
DROP INDEX IF EXISTS idx_workspaces_catalog_status;

ALTER TABLE workspaces DROP COLUMN catalog_refresh_status;
ALTER TABLE workspaces DROP COLUMN catalog_refresh_progress;
