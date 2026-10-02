ALTER TABLE dashboards ADD COLUMN write_revision INTEGER NOT NULL DEFAULT 0;

CREATE TABLE IF NOT EXISTS copilot_mutation_receipts (
    receipt_id TEXT PRIMARY KEY,
    dashboard_id TEXT NOT NULL REFERENCES dashboards(dashboard_id) ON DELETE CASCADE,
    workspace_id TEXT NOT NULL,
    user_id TEXT NOT NULL,
    session_id TEXT NOT NULL,
    pre_version_number INTEGER NOT NULL,
    pre_title TEXT NOT NULL,
    pre_content TEXT NOT NULL,
    saved_title TEXT NOT NULL,
    saved_content TEXT NOT NULL,
    saved_updated_at TEXT NOT NULL,
    saved_revision INTEGER NOT NULL,
    change_summary TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_copilot_mutation_receipts_session
    ON copilot_mutation_receipts(session_id, created_at DESC);

CREATE INDEX IF NOT EXISTS idx_copilot_mutation_receipts_dashboard
    ON copilot_mutation_receipts(dashboard_id, created_at DESC);
