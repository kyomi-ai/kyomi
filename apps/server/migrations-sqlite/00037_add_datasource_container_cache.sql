-- KYO-622: container-liveness bookkeeping for the catalog-archive GC.
--
-- See the Postgres counterpart
-- (apps/server/migrations/20260905000000_add_datasource_container_cache.sql)
-- for the full KYO-614/KYO-622/KYO-619 background -- this file repeats only
-- what differs for SQLite: INTEGER in place of BIGINT (SQLite's dynamic
-- typing stores any integer the same way regardless of declared type), and
-- `datetime('now')` in place of `NOW()` for the defaults.
--
-- NO BACKFILL -- see the Postgres file for why an existing container
-- correctly starts at missed_runs = 0, same as any newly-seen one.
CREATE TABLE IF NOT EXISTS datasource_container_cache (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    workspace_id TEXT NOT NULL,
    datasource_config_id TEXT NOT NULL REFERENCES datasource_configs(id) ON DELETE CASCADE,
    project_id TEXT NOT NULL,
    dataset_id TEXT NOT NULL,
    last_seen_at TEXT,
    missed_runs INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE (workspace_id, datasource_config_id, project_id, dataset_id)
);

CREATE INDEX IF NOT EXISTS idx_datasource_container_cache_lookup
    ON datasource_container_cache (workspace_id, datasource_config_id);
