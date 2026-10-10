-- Public committed read projection survives journal retention. The common atomic
-- event writer maintains it; private provider/candidate records never enter it.
CREATE TABLE conversation_read_projection (
    event_id TEXT NOT NULL PRIMARY KEY,
    session_id TEXT NOT NULL REFERENCES chat_sessions(session_id) ON DELETE CASCADE,
    run_id TEXT NOT NULL,
    sequence BIGINT NOT NULL CHECK (sequence > 0),
    version INTEGER NOT NULL,
    encrypted_payload TEXT NOT NULL,
    detail_id TEXT UNIQUE,
    encrypted_detail TEXT,
    UNIQUE (session_id, sequence),
    FOREIGN KEY (session_id, run_id) REFERENCES conversation_runs(session_id, run_id) ON DELETE CASCADE,
    CHECK ((detail_id IS NULL) = (encrypted_detail IS NULL))
);
CREATE INDEX conversation_read_projection_run ON conversation_read_projection(session_id, run_id, sequence);
-- Copy ciphertext without decrypting in a migration. Conflict protection also
-- makes backfill safe to repeat as a maintenance operation.
INSERT INTO conversation_read_projection(event_id,session_id,run_id,sequence,version,encrypted_payload,detail_id,encrypted_detail)
SELECT e.event_id,e.session_id,e.run_id,e.sequence,e.version,e.encrypted_payload,d.detail_id,d.encrypted_content
FROM conversation_events e LEFT JOIN conversation_event_details d ON d.event_id=e.event_id AND d.session_id=e.session_id AND d.public=true
WHERE e.public=true
ON CONFLICT(event_id) DO NOTHING;
