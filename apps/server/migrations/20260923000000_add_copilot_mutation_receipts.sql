ALTER TABLE public.dashboards ADD COLUMN write_revision bigint NOT NULL DEFAULT 0;

CREATE TABLE IF NOT EXISTS public.copilot_mutation_receipts (
    receipt_id text PRIMARY KEY,
    dashboard_id varchar(50) NOT NULL REFERENCES public.dashboards(dashboard_id) ON DELETE CASCADE,
    workspace_id varchar(50) NOT NULL,
    user_id varchar(50) NOT NULL,
    session_id varchar(50) NOT NULL,
    pre_version_number integer NOT NULL,
    pre_title text NOT NULL,
    pre_content text NOT NULL,
    saved_title text NOT NULL,
    saved_content text NOT NULL,
    saved_updated_at timestamptz NOT NULL,
    saved_revision bigint NOT NULL,
    change_summary text NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_copilot_mutation_receipts_session
    ON public.copilot_mutation_receipts(session_id, created_at DESC);

CREATE INDEX IF NOT EXISTS idx_copilot_mutation_receipts_dashboard
    ON public.copilot_mutation_receipts(dashboard_id, created_at DESC);
