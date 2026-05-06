-- Analytics sites table

CREATE TABLE IF NOT EXISTS analytics_sites (
    id TEXT NOT NULL PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    site_id TEXT NOT NULL UNIQUE,
    allowed_domains TEXT NOT NULL DEFAULT '[]',
    signed_key TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_analytics_sites_workspace ON analytics_sites(workspace_id);
CREATE INDEX IF NOT EXISTS idx_analytics_sites_site_id ON analytics_sites(site_id);
