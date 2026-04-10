-- Add bundle balance fields for the single Cloud tier billing model.
-- AI token bundles and analytics event bundles are purchased separately
-- and tracked as workspace-level balances.

ALTER TABLE workspaces ADD COLUMN ai_bundle_balance_usd DOUBLE PRECISION NOT NULL DEFAULT 0.0;
ALTER TABLE workspaces ADD COLUMN analytics_bundle_events BIGINT NOT NULL DEFAULT 0;
