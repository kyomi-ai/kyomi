-- KYO-973: provider/model/mode on each watch execution, and the durable state
-- a batch-mode run needs to survive restarts and multiple instances.
--
-- llm_mode            'realtime' | 'batch' (what the run actually used)
-- batch_ids           JSON array text of the vendor batch ids this run submitted
-- batch_fallback_reason  why a batch-configured run (or part of one) ran realtime
-- batch_deadline_at   latest time the run may still wait on a batch
-- usage_billed_at     set once when the run's usage was written to api_usage_log
-- lease_owner / lease_expires_at  which instance is driving a batch-mode run;
--                     an expired lease lets any instance resume it
ALTER TABLE watch_executions ADD COLUMN llm_provider varchar(50);
ALTER TABLE watch_executions ADD COLUMN llm_model varchar(200);
ALTER TABLE watch_executions ADD COLUMN llm_mode varchar(20);
ALTER TABLE watch_executions ADD COLUMN batch_ids text;
ALTER TABLE watch_executions ADD COLUMN batch_fallback_reason text;
ALTER TABLE watch_executions ADD COLUMN batch_deadline_at timestamp with time zone;
ALTER TABLE watch_executions ADD COLUMN lease_owner varchar(200);
ALTER TABLE watch_executions ADD COLUMN lease_expires_at timestamp with time zone;
ALTER TABLE watch_executions ADD COLUMN usage_billed_at timestamp with time zone;
ALTER TABLE watch_executions ADD COLUMN llm_workspace_billed boolean NOT NULL DEFAULT false;

-- Resume scan: unfinished batch-mode runs by lease expiry.
CREATE INDEX IF NOT EXISTS idx_watch_executions_batch_lease
    ON watch_executions (lease_expires_at)
    WHERE llm_mode = 'batch' AND completed_at IS NULL;

-- One row per LLM call a batch-mode run makes. (execution_id, call_index) is
-- the replay key: a resumed run replays from the start and its nth model call is
-- answered by the nth row (the finished response, the vendor batch still being
-- polled, or the recorded outcome), so a call is never resubmitted even when the
-- replayed tool results differ. request_hash is kept as a drift check only.
CREATE TABLE IF NOT EXISTS watch_batch_calls (
    call_id varchar(50) PRIMARY KEY,
    execution_id integer NOT NULL REFERENCES watch_executions(id) ON DELETE CASCADE,
    call_index integer NOT NULL,
    request_hash varchar(64) NOT NULL,
    provider varchar(50) NOT NULL,
    model varchar(200) NOT NULL,
    vendor_batch_id varchar(200),
    state varchar(20) NOT NULL,
    response_encrypted text,
    error_message text,
    input_tokens integer NOT NULL DEFAULT 0,
    output_tokens integer NOT NULL DEFAULT 0,
    cost_usd double precision,
    submitted_at timestamp with time zone NOT NULL DEFAULT now(),
    completed_at timestamp with time zone,
    UNIQUE (execution_id, call_index)
);
