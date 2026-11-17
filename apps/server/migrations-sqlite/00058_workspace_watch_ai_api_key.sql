-- KYO-973: see the PostgreSQL migration 20261116000000.
ALTER TABLE workspaces ADD COLUMN watch_ai_api_key_encrypted TEXT;
