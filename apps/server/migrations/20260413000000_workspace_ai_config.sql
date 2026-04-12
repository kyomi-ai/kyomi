-- Workspace-level AI configuration for BYOK (Bring Your Own Key).
--
-- Every workspace has exactly one AI configuration. Admins choose between:
--   * 'kyomi'   — use Kyomi's server-side keys, debit ai_bundle_balance_usd
--   * 'anthropic' | 'openai' | 'gemini' — workspace-owned BYOK key
--
-- The API key (when present) is AES-GCM encrypted with WORKSPACE_SECRETS_KEY
-- before storage. ai_base_url is optional for proxies / compatible endpoints.
--
-- Existing workspaces default to 'kyomi' — no data migration needed.

ALTER TABLE workspaces ADD COLUMN ai_provider TEXT NOT NULL DEFAULT 'kyomi'
    CHECK (ai_provider IN ('kyomi', 'anthropic', 'openai', 'gemini'));
ALTER TABLE workspaces ADD COLUMN ai_api_key_encrypted TEXT;
ALTER TABLE workspaces ADD COLUMN ai_base_url TEXT;
