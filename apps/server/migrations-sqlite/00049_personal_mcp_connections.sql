-- Personal MCP ownership is global; workspace rows only authorize execution.
CREATE TABLE user_mcp_connections (
    connection_id TEXT NOT NULL PRIMARY KEY,
    owner_user_id TEXT NOT NULL REFERENCES users(user_id) ON DELETE CASCADE,
    name TEXT NOT NULL CHECK (length(name) BETWEEN 1 AND 100),
    endpoint TEXT NOT NULL,
    auth_kind TEXT NOT NULL CHECK (auth_kind IN ('none', 'bearer', 'oauth')),
    status TEXT NOT NULL CHECK (status IN ('ready', 'needs_auth', 'disabled')),
    generation BIGINT NOT NULL DEFAULT 1 CHECK (generation > 0),
    UNIQUE (owner_user_id, name),
    UNIQUE (connection_id, owner_user_id)
);
CREATE INDEX idx_user_mcp_connections_owner ON user_mcp_connections(owner_user_id, connection_id);
CREATE TABLE user_mcp_secrets (
    connection_id TEXT NOT NULL PRIMARY KEY REFERENCES user_mcp_connections(connection_id) ON DELETE CASCADE,
    encrypted_secret TEXT NOT NULL
);
CREATE TABLE user_mcp_workspace_enablements (
    connection_id TEXT NOT NULL,
    owner_user_id TEXT NOT NULL,
    workspace_id TEXT NOT NULL,
    PRIMARY KEY (connection_id, workspace_id),
    FOREIGN KEY (connection_id, owner_user_id) REFERENCES user_mcp_connections(connection_id, owner_user_id) ON DELETE CASCADE,
    FOREIGN KEY (workspace_id, owner_user_id) REFERENCES workspace_users(workspace_id, user_id) ON DELETE CASCADE
);
CREATE TABLE user_mcp_tool_grants (
    connection_id TEXT NOT NULL REFERENCES user_mcp_connections(connection_id) ON DELETE CASCADE,
    workspace_id TEXT NOT NULL,
    tool_name TEXT NOT NULL,
    revision TEXT NOT NULL,
    schema_hash TEXT NOT NULL,
    generation BIGINT NOT NULL CHECK (generation > 0),
    allow_automation BOOLEAN NOT NULL DEFAULT FALSE,
    PRIMARY KEY (connection_id, workspace_id, tool_name),
    FOREIGN KEY (connection_id, workspace_id) REFERENCES user_mcp_workspace_enablements(connection_id, workspace_id) ON DELETE CASCADE
);
CREATE TABLE user_mcp_auth_transactions (
    state_hash TEXT NOT NULL PRIMARY KEY,
    connection_id TEXT NOT NULL REFERENCES user_mcp_connections(connection_id) ON DELETE CASCADE,
    generation BIGINT NOT NULL CHECK (generation > 0),
    endpoint TEXT NOT NULL,
    issuer TEXT NOT NULL,
    resource TEXT NOT NULL,
    redirect_uri TEXT NOT NULL,
    encrypted_payload TEXT NOT NULL,
    expires_at BIGINT NOT NULL
);
CREATE INDEX idx_user_mcp_auth_transactions_connection ON user_mcp_auth_transactions(connection_id);
CREATE INDEX idx_user_mcp_auth_transactions_expiry ON user_mcp_auth_transactions(expires_at);
