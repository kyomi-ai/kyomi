-- Materialized learning-to-entity references
-- Replaces FalkorDB edges: MENTIONS_TABLE, MENTIONS_COLUMN, DEFINED_BY, APPLIES_TO
CREATE TABLE IF NOT EXISTS learning_references (
    id SERIAL PRIMARY KEY,
    learning_id UUID NOT NULL REFERENCES agent_learnings(learning_id) ON DELETE CASCADE,
    workspace_id TEXT NOT NULL,
    ref_type TEXT NOT NULL,        -- 'table', 'column', 'metric'
    ref_name TEXT NOT NULL,        -- resolved full name (e.g., 'public.orders', 'public.orders#customer_id')
    UNIQUE(learning_id, ref_type, ref_name)
);

CREATE INDEX IF NOT EXISTS idx_learning_refs_workspace ON learning_references(workspace_id);
CREATE INDEX IF NOT EXISTS idx_learning_refs_lookup ON learning_references(ref_type, ref_name, workspace_id);
CREATE INDEX IF NOT EXISTS idx_learning_refs_learning ON learning_references(learning_id);

-- Episodic conversation tracking
-- Replaces FalkorDB Conversation nodes and DISCUSSED edges
CREATE TABLE IF NOT EXISTS conversation_discussed (
    id SERIAL PRIMARY KEY,
    session_id TEXT NOT NULL,
    workspace_id TEXT NOT NULL,
    user_id TEXT NOT NULL,
    entity_type TEXT NOT NULL,     -- 'table', 'learning', 'metric'
    entity_id TEXT NOT NULL,       -- full_name, UUID, or metric name
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE(session_id, entity_type, entity_id)
);

CREATE INDEX IF NOT EXISTS idx_conv_discussed_session ON conversation_discussed(session_id);
CREATE INDEX IF NOT EXISTS idx_conv_discussed_workspace ON conversation_discussed(workspace_id);
CREATE INDEX IF NOT EXISTS idx_conv_discussed_entity ON conversation_discussed(entity_type, entity_id, workspace_id);
