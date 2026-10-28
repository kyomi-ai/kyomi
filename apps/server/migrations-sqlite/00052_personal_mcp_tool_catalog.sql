-- Catalogs are owner/connection/workspace scoped and die with enablement.
-- Existing grants default to no destination consent and cannot authorize execution.
ALTER TABLE user_mcp_tool_grants ADD COLUMN catalog_id TEXT NOT NULL DEFAULT '';
ALTER TABLE user_mcp_tool_grants ADD COLUMN destination_consent BOOLEAN NOT NULL DEFAULT FALSE;
CREATE TABLE user_mcp_tool_catalogs (
    connection_id TEXT NOT NULL,
    workspace_id TEXT NOT NULL,
    generation BIGINT NOT NULL CHECK (generation > 0),
    catalog_id TEXT NOT NULL,
    expires_at BIGINT NOT NULL,
    catalog_json TEXT NOT NULL,
    PRIMARY KEY (connection_id, workspace_id),
    FOREIGN KEY (connection_id, workspace_id) REFERENCES user_mcp_workspace_enablements(connection_id, workspace_id) ON DELETE CASCADE
);
