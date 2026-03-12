-- Platform integration tables for messaging platform abstraction (Phase 12)
-- These generic tables replace the direct slack_* columns on workspaces/workspace_users/watches/chat_sessions.
-- Old columns are NOT dropped yet -- that happens in Task 6 after all code is migrated.

-- 1. Platform user links (maps platform identity -> Kyomi user)
CREATE TABLE IF NOT EXISTS platform_user_links (
    id TEXT NOT NULL PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    user_id TEXT NOT NULL REFERENCES users(user_id) ON DELETE CASCADE,
    platform_type TEXT NOT NULL,
    platform_user_id TEXT NOT NULL,
    platform_username TEXT,
    connected_at TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE (workspace_id, platform_type, platform_user_id)
);

-- 2. Workspace integrations (workspace-level platform installation)
CREATE TABLE IF NOT EXISTS workspace_integrations (
    id TEXT NOT NULL PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    platform_type TEXT NOT NULL,
    config TEXT NOT NULL DEFAULT '{}',
    installed_by TEXT REFERENCES users(user_id),
    installed_at TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE (workspace_id, platform_type)
);

-- 3. Workspace user integrations (per-user platform credentials)
CREATE TABLE IF NOT EXISTS workspace_user_integrations (
    id TEXT NOT NULL PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    user_id TEXT NOT NULL REFERENCES users(user_id) ON DELETE CASCADE,
    platform_type TEXT NOT NULL,
    config TEXT NOT NULL DEFAULT '{}',
    connected_at TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE (workspace_id, user_id, platform_type)
);

-- 4. Watch alert channels (which platforms a watch sends alerts to)
CREATE TABLE IF NOT EXISTS watch_alert_channels (
    id TEXT NOT NULL PRIMARY KEY,
    watch_id TEXT NOT NULL REFERENCES watches(watch_id) ON DELETE CASCADE,
    channel_type TEXT NOT NULL,
    channel_id TEXT NOT NULL,
    channel_name TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE (watch_id, channel_type)
);

-- 5. Add generic platform columns to chat_sessions
ALTER TABLE chat_sessions ADD COLUMN platform_type TEXT;
ALTER TABLE chat_sessions ADD COLUMN platform_thread_key TEXT;

CREATE UNIQUE INDEX IF NOT EXISTS idx_chat_sessions_platform_thread
    ON chat_sessions (platform_type, platform_thread_key)
    WHERE platform_type IS NOT NULL;

-- Indexes for common lookups
CREATE INDEX IF NOT EXISTS idx_platform_user_links_user ON platform_user_links(user_id);
CREATE INDEX IF NOT EXISTS idx_platform_user_links_workspace ON platform_user_links(workspace_id);
CREATE INDEX IF NOT EXISTS idx_platform_user_links_platform ON platform_user_links(platform_type, platform_user_id);

CREATE INDEX IF NOT EXISTS idx_workspace_integrations_workspace ON workspace_integrations(workspace_id);

CREATE INDEX IF NOT EXISTS idx_workspace_user_integrations_user ON workspace_user_integrations(user_id);
CREATE INDEX IF NOT EXISTS idx_workspace_user_integrations_workspace ON workspace_user_integrations(workspace_id);

CREATE INDEX IF NOT EXISTS idx_watch_alert_channels_watch ON watch_alert_channels(watch_id);
