CREATE TABLE IF NOT EXISTS sync_log (
    sync_id BIGSERIAL PRIMARY KEY,
    entity_type VARCHAR(50) NOT NULL,
    entity_id VARCHAR(100) NOT NULL,
    workspace_id VARCHAR(100) NOT NULL,
    action VARCHAR(10) NOT NULL,  -- 'insert', 'update', 'delete'
    data JSONB,                    -- Full entity snapshot, NULL for deletes
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_sync_log_workspace_id ON sync_log(workspace_id, sync_id);
CREATE INDEX idx_sync_log_created_at ON sync_log(created_at);
