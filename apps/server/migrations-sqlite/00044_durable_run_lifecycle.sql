-- Durable acceptance and ownership metadata. Old journal rows remain unmanaged.
ALTER TABLE conversation_runs ADD COLUMN request_id TEXT;
ALTER TABLE conversation_runs ADD COLUMN actor_id TEXT REFERENCES users(user_id);
ALTER TABLE conversation_runs ADD COLUMN workspace_id TEXT REFERENCES workspaces(workspace_id);
ALTER TABLE conversation_runs ADD COLUMN user_message_id TEXT REFERENCES chat_messages(message_id) ON DELETE SET NULL;
ALTER TABLE conversation_runs ADD COLUMN assistant_message_id TEXT REFERENCES chat_messages(message_id) ON DELETE SET NULL;
ALTER TABLE conversation_runs ADD COLUMN encrypted_submission TEXT;
ALTER TABLE conversation_runs ADD COLUMN fence BIGINT NOT NULL DEFAULT 0 CHECK (fence >= 0);
ALTER TABLE conversation_runs ADD COLUMN lease_owner TEXT;
ALTER TABLE conversation_runs ADD COLUMN lease_expires_at BIGINT;
ALTER TABLE conversation_runs ADD COLUMN cancellation_requested BOOLEAN NOT NULL DEFAULT false;
ALTER TABLE conversation_runs ADD COLUMN queued_at BIGINT;
ALTER TABLE conversation_runs ADD COLUMN started_at BIGINT;
ALTER TABLE conversation_runs ADD COLUMN terminal_at BIGINT;
CREATE UNIQUE INDEX conversation_runs_request ON conversation_runs(workspace_id,actor_id,request_id) WHERE request_id IS NOT NULL;
CREATE UNIQUE INDEX conversation_runs_active ON conversation_runs(session_id) WHERE (state = 'running') AND (request_id IS NOT NULL);
CREATE INDEX conversation_runs_queue ON conversation_runs(state,queued_at) WHERE request_id IS NOT NULL;
