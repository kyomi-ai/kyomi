-- KYO-973: optional API key for a watch-specific LLM provider, AES-GCM
-- encrypted with WORKSPACE_SECRETS_KEY exactly like ai_api_key_encrypted.
-- Kept in its own column (not settings JSON) so the ciphertext is never part
-- of the workspace settings snapshot synced to clients.
ALTER TABLE workspaces ADD COLUMN watch_ai_api_key_encrypted TEXT;
