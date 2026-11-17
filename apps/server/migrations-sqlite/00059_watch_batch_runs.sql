-- KYO-973: see the PostgreSQL migration 20261118000100.
ALTER TABLE watch_executions ADD COLUMN llm_provider TEXT;
ALTER TABLE watch_executions ADD COLUMN llm_model TEXT;
ALTER TABLE watch_executions ADD COLUMN llm_mode TEXT;
ALTER TABLE watch_executions ADD COLUMN batch_ids TEXT;
ALTER TABLE watch_executions ADD COLUMN batch_fallback_reason TEXT;
ALTER TABLE watch_executions ADD COLUMN batch_deadline_at TEXT;
ALTER TABLE watch_executions ADD COLUMN lease_owner TEXT;
ALTER TABLE watch_executions ADD COLUMN lease_expires_at TEXT;
ALTER TABLE watch_executions ADD COLUMN usage_billed_at TEXT;
ALTER TABLE watch_executions ADD COLUMN llm_workspace_billed INTEGER NOT NULL DEFAULT 0;

CREATE INDEX IF NOT EXISTS idx_watch_executions_batch_lease
    ON watch_executions (lease_expires_at)
    WHERE llm_mode = 'batch' AND completed_at IS NULL;

CREATE TABLE IF NOT EXISTS watch_batch_calls (
    call_id TEXT NOT NULL PRIMARY KEY,
    execution_id INTEGER NOT NULL REFERENCES watch_executions(id) ON DELETE CASCADE,
    call_index INTEGER NOT NULL,
    request_hash TEXT NOT NULL,
    provider TEXT NOT NULL,
    model TEXT NOT NULL,
    vendor_batch_id TEXT,
    state TEXT NOT NULL,
    response_encrypted TEXT,
    error_message TEXT,
    input_tokens INTEGER NOT NULL DEFAULT 0,
    output_tokens INTEGER NOT NULL DEFAULT 0,
    cost_usd REAL,
    submitted_at TEXT NOT NULL DEFAULT (datetime('now')),
    completed_at TEXT,
    UNIQUE (execution_id, call_index)
);
