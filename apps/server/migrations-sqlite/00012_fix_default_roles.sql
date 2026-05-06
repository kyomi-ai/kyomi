-- Fix default role values to use full workspace_user format
-- (Cannot ALTER DEFAULT in SQLite, so recreate the affected tables)

-- 1. workspace_invitations: change default role from 'user' to 'workspace_user'
CREATE TABLE IF NOT EXISTS workspace_invitations_new (
    invitation_id TEXT NOT NULL PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id),
    email TEXT NOT NULL,
    role TEXT NOT NULL DEFAULT 'workspace_user',
    invited_by_user_id TEXT NOT NULL REFERENCES users(user_id),
    status TEXT NOT NULL DEFAULT 'pending',
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    expires_at TEXT NOT NULL,
    accepted_at TEXT,
    accepted_by_user_id TEXT REFERENCES users(user_id)
);

INSERT OR IGNORE INTO workspace_invitations_new
    SELECT * FROM workspace_invitations;

DROP TABLE workspace_invitations;
ALTER TABLE workspace_invitations_new RENAME TO workspace_invitations;

CREATE INDEX IF NOT EXISTS idx_workspace_invitations_email ON workspace_invitations(email);
CREATE INDEX IF NOT EXISTS idx_workspace_invitations_status ON workspace_invitations(status);
CREATE INDEX IF NOT EXISTS idx_workspace_invitations_workspace ON workspace_invitations(workspace_id);

-- 2. workspace_users: change default role from 'user' to 'workspace_user'
CREATE TABLE IF NOT EXISTS workspace_users_new (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id),
    user_id TEXT NOT NULL REFERENCES users(user_id),
    role TEXT NOT NULL DEFAULT 'workspace_user',
    active INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    last_active TEXT,
    extra_metadata TEXT,
    UNIQUE(workspace_id, user_id)
);

INSERT OR IGNORE INTO workspace_users_new
    SELECT * FROM workspace_users;

DROP TABLE workspace_users;
ALTER TABLE workspace_users_new RENAME TO workspace_users;

CREATE INDEX IF NOT EXISTS idx_workspace_users_role ON workspace_users(role);
CREATE INDEX IF NOT EXISTS idx_workspace_users_user ON workspace_users(user_id);
CREATE INDEX IF NOT EXISTS idx_workspace_users_workspace ON workspace_users(workspace_id);
