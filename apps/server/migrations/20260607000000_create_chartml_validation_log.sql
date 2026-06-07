-- SPDX-License-Identifier: AGPL-3.0-or-later
-- Tracks ChartML validation failures for prompt tuning and observability.
-- Each row represents one validation failure, with optional retry outcome.

CREATE TABLE chartml_validation_log (
    id SERIAL PRIMARY KEY,
    session_id TEXT NOT NULL,
    workspace_id TEXT NOT NULL,
    user_id TEXT NOT NULL,
    raw_response TEXT NOT NULL,
    error_message TEXT NOT NULL,
    error_type TEXT NOT NULL,
    retry_attempt INTEGER NOT NULL DEFAULT 0,
    retry_succeeded BOOLEAN,
    component TEXT NOT NULL DEFAULT 'chat',
    model TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_chartml_validation_log_workspace
    ON chartml_validation_log (workspace_id, created_at);
CREATE INDEX idx_chartml_validation_log_error_type
    ON chartml_validation_log (error_type, created_at);
