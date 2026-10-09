-- Durable journal foundation; existing sessions/messages remain compatibility history.
ALTER TABLE chat_sessions ADD COLUMN event_sequence BIGINT NOT NULL DEFAULT 0 CHECK (event_sequence >= 0);
CREATE TABLE conversation_runs (
    run_id TEXT NOT NULL PRIMARY KEY,
    session_id TEXT NOT NULL REFERENCES chat_sessions(session_id) ON DELETE CASCADE,
    state TEXT NOT NULL CHECK (state IN ('queued','running','completed','failed','cancelled','interrupted')),
    UNIQUE (session_id, run_id)
);
CREATE TABLE conversation_events (
    event_id TEXT NOT NULL PRIMARY KEY,
    session_id TEXT NOT NULL REFERENCES chat_sessions(session_id) ON DELETE CASCADE,
    run_id TEXT NOT NULL,
    sequence BIGINT NOT NULL CHECK (sequence > 0),
    version INTEGER NOT NULL,
    idempotency_key TEXT NOT NULL,
    public BOOLEAN NOT NULL,
    encrypted_payload TEXT NOT NULL,
    encrypted_command TEXT NOT NULL,
    UNIQUE (session_id, sequence),
    UNIQUE (session_id, idempotency_key),
    UNIQUE (session_id, event_id),
    FOREIGN KEY (session_id, run_id) REFERENCES conversation_runs(session_id, run_id) ON DELETE CASCADE
);
-- Receipt deduplication also reserves every acknowledged event/key alias. A later
-- conflicting retry cannot reuse an alias to produce another logical event.
CREATE TABLE conversation_event_aliases (
    session_id TEXT NOT NULL,
    idempotency_key TEXT NOT NULL,
    alias_event_id TEXT NOT NULL UNIQUE,
    event_id TEXT NOT NULL,
    encrypted_command TEXT NOT NULL,
    PRIMARY KEY (session_id, idempotency_key),
    FOREIGN KEY (session_id, event_id) REFERENCES conversation_events(session_id, event_id) ON DELETE CASCADE
);
CREATE TABLE conversation_event_details (
    detail_id TEXT NOT NULL PRIMARY KEY,
    session_id TEXT NOT NULL,
    event_id TEXT NOT NULL,
    public BOOLEAN NOT NULL,
    encrypted_content TEXT NOT NULL,
    FOREIGN KEY (session_id, event_id) REFERENCES conversation_events(session_id, event_id) ON DELETE CASCADE
);
CREATE TABLE conversation_tool_receipts (
    session_id TEXT NOT NULL,
    run_id TEXT NOT NULL,
    tool_call_id TEXT NOT NULL,
    event_id TEXT NOT NULL,
    succeeded BOOLEAN NOT NULL,
    PRIMARY KEY (session_id, run_id, tool_call_id),
    FOREIGN KEY (session_id, event_id) REFERENCES conversation_events(session_id, event_id) ON DELETE CASCADE,
    FOREIGN KEY (session_id, run_id) REFERENCES conversation_runs(session_id, run_id) ON DELETE CASCADE
);
CREATE TABLE conversation_usage_receipts (
    session_id TEXT NOT NULL,
    run_id TEXT NOT NULL,
    model_call_id TEXT NOT NULL,
    event_id TEXT NOT NULL,
    PRIMARY KEY (session_id, run_id, model_call_id),
    FOREIGN KEY (session_id, event_id) REFERENCES conversation_events(session_id, event_id) ON DELETE CASCADE,
    FOREIGN KEY (session_id, run_id) REFERENCES conversation_runs(session_id, run_id) ON DELETE CASCADE
);
CREATE INDEX conversation_events_run ON conversation_events(session_id, run_id, sequence);
CREATE INDEX conversation_event_details_session ON conversation_event_details(session_id);
