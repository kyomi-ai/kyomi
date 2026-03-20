CREATE TABLE IF NOT EXISTS analytics_sites (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    workspace_id    VARCHAR(50) NOT NULL REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    name            VARCHAR(255) NOT NULL,
    site_id         VARCHAR(16) NOT NULL UNIQUE,
    allowed_domains TEXT[] NOT NULL DEFAULT '{}',
    signed_key      TEXT NOT NULL,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_analytics_sites_workspace ON analytics_sites(workspace_id);
CREATE INDEX IF NOT EXISTS idx_analytics_sites_site_id ON analytics_sites(site_id);
