-- Workspace-level AI configuration for BYOK (Bring Your Own Key).
-- See PostgreSQL migration 20260413000000 for full description.
-- SQLite does not support CHECK constraints in ALTER TABLE ADD COLUMN;
-- provider validation is enforced at the application layer
-- (WorkspaceAiProvider::from_str in kyomi-auth).
ALTER TABLE workspaces ADD COLUMN ai_provider TEXT NOT NULL DEFAULT 'kyomi';
ALTER TABLE workspaces ADD COLUMN ai_api_key_encrypted TEXT;
ALTER TABLE workspaces ADD COLUMN ai_base_url TEXT;
