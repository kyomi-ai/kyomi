-- Add bundle balance fields for single Cloud tier billing.
ALTER TABLE workspaces ADD COLUMN ai_bundle_balance_usd REAL NOT NULL DEFAULT 0.0;
ALTER TABLE workspaces ADD COLUMN analytics_bundle_events INTEGER NOT NULL DEFAULT 0;
