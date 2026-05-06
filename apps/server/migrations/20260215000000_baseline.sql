-- Kyomi baseline migration
-- Generated from production schema on 2026-02-15
-- Idempotent: safe to run on both empty and populated databases
--
-- All CREATE TABLE/INDEX/SEQUENCE use IF NOT EXISTS
-- All ADD CONSTRAINT wrapped in DO blocks with duplicate_object exception
-- All CREATE FUNCTION use CREATE OR REPLACE
-- All CREATE TRIGGER use DROP IF EXISTS + CREATE
-- alembic_version table intentionally excluded

--
-- PostgreSQL database dump
--


-- Dumped from database version 15.14 (Debian 15.14-1.pgdg12+1)
-- Dumped by pg_dump version 18.0


--
-- Name: pg_trgm; Type: EXTENSION; Schema: -; Owner: -
--

CREATE EXTENSION IF NOT EXISTS pg_trgm WITH SCHEMA public;

--
-- Name: EXTENSION pg_trgm; Type: COMMENT; Schema: -; Owner: -
--

COMMENT ON EXTENSION pg_trgm IS 'text similarity measurement and index searching based on trigrams';

--
-- Name: vector; Type: EXTENSION; Schema: -; Owner: -
--

CREATE EXTENSION IF NOT EXISTS vector WITH SCHEMA public;

--
-- Name: EXTENSION vector; Type: COMMENT; Schema: -; Owner: -
--

COMMENT ON EXTENSION vector IS 'vector data type and ivfflat and hnsw access methods';

--
-- Name: learning_scope; Type: TYPE; Schema: public; Owner: -
--

DO $$ BEGIN
    CREATE TYPE public.learning_scope AS ENUM (
    'workspace',
    'user'
);
EXCEPTION WHEN duplicate_object THEN NULL;
END $$;

--
-- Name: agent_learnings_search_vector_update(); Type: FUNCTION; Schema: public; Owner: -
--

CREATE OR REPLACE FUNCTION public.agent_learnings_search_vector_update() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
  NEW.search_vector :=
    setweight(to_tsvector('english', COALESCE(NEW.insight, '')), 'A') ||
    setweight(to_tsvector('english', COALESCE(NEW.context, '')), 'B');
  RETURN NEW;
END;
$$;

--
-- Name: chat_messages_content_tsv_trigger(); Type: FUNCTION; Schema: public; Owner: -
--

CREATE OR REPLACE FUNCTION public.chat_messages_content_tsv_trigger() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
begin
  new.content_tsv :=
     to_tsvector('english', coalesce(new.content,''));
  return new;
end
$$;

--
-- Name: dashboards_search_vector_update(); Type: FUNCTION; Schema: public; Owner: -
--

CREATE OR REPLACE FUNCTION public.dashboards_search_vector_update() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
        DECLARE
            summary_text TEXT;
            clean_content TEXT;
        BEGIN
            -- Extract summary from HTML comment: <!-- dashboard-summary: ... -->
            summary_text := substring(NEW.content FROM '<!-- dashboard-summary: (.+?) -->');

            -- Strip the summary comment from content for indexing
            clean_content := regexp_replace(
                COALESCE(NEW.content, ''),
                '^<!-- dashboard-summary: .+? -->\n?',
                ''
            );
            -- Truncate content to first 2000 chars for indexing
            clean_content := left(clean_content, 2000);

            NEW.search_vector :=
                setweight(to_tsvector('english', COALESCE(NEW.title, '')), 'A') ||
                setweight(to_tsvector('english', COALESCE(summary_text, '')), 'B') ||
                setweight(to_tsvector('english', COALESCE(clean_content, '')), 'C');
            RETURN NEW;
        END;
        $$;

--
-- Name: update_collections_updated_at(); Type: FUNCTION; Schema: public; Owner: -
--

CREATE OR REPLACE FUNCTION public.update_collections_updated_at() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    NEW.updated_at = CURRENT_TIMESTAMP;
    RETURN NEW;
END;
$$;

--
-- Name: agent_learnings; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE IF NOT EXISTS public.agent_learnings (
    learning_id uuid DEFAULT gen_random_uuid() NOT NULL,
    workspace_id character varying(50) NOT NULL,
    insight text NOT NULL,
    context text,
    embedding public.vector(384),
    enabled boolean DEFAULT true,
    learned_from_session character varying(50),
    learned_from_user character varying(50),
    created_at timestamp with time zone DEFAULT now(),
    times_used integer DEFAULT 0,
    last_used_at timestamp with time zone,
    scope public.learning_scope DEFAULT 'workspace'::public.learning_scope NOT NULL,
    superseded_by uuid,
    superseded_at timestamp without time zone,
    search_vector tsvector,
    datasource_config_id character varying(50),
    learning_type character varying(20) DEFAULT 'learning'::character varying NOT NULL,
    reference_queries jsonb,
    structured_metadata jsonb
);

--
-- Name: TABLE agent_learnings; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON TABLE public.agent_learnings IS 'Stores workspace-scoped learnings automatically captured by the AI agent. Retrieved via semantic search and injected into agent context.';

--
-- Name: COLUMN agent_learnings.insight; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.agent_learnings.insight IS 'The learning in natural language, written by the LLM as advice to its future self.';

--
-- Name: COLUMN agent_learnings.context; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.agent_learnings.context IS 'Optional explanation of how/why this was learned (e.g., "User corrected table choice").';

--
-- Name: COLUMN agent_learnings.enabled; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.agent_learnings.enabled IS 'Admin can toggle learnings off without deleting them.';

--
-- Name: COLUMN agent_learnings.times_used; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.agent_learnings.times_used IS 'Counter incremented each time learning is retrieved for a conversation.';

--
-- Name: COLUMN agent_learnings.scope; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.agent_learnings.scope IS 'Scope of learning: workspace (all users) or user (only creator)';

--
-- Name: COLUMN agent_learnings.superseded_by; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.agent_learnings.superseded_by IS 'UUID of the learning that supersedes this one (if outdated)';

--
-- Name: COLUMN agent_learnings.superseded_at; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.agent_learnings.superseded_at IS 'Timestamp when this learning was marked as superseded';

--
-- Name: COLUMN agent_learnings.datasource_config_id; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.agent_learnings.datasource_config_id IS 'Optional datasource scope. NULL = global learning (all datasources). UUID = datasource-specific learning (e.g., "Revenue is in sales.transactions table").';

--
-- Name: api_tokens; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE IF NOT EXISTS public.api_tokens (
    token_id character varying(100) NOT NULL,
    user_id character varying(50) NOT NULL,
    name character varying(255) NOT NULL,
    token_hash character varying(255) NOT NULL,
    active boolean DEFAULT true NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    expires_at timestamp with time zone,
    last_used timestamp with time zone,
    revoked_at timestamp with time zone,
    created_by character varying(255),
    revoked_by character varying(255)
);

--
-- Name: api_usage_log; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE IF NOT EXISTS public.api_usage_log (
    id integer NOT NULL,
    user_id character varying(50) NOT NULL,
    workspace_id character varying(50) NOT NULL,
    session_id character varying(100),
    "timestamp" timestamp with time zone NOT NULL,
    provider character varying(50) NOT NULL,
    model character varying(100) NOT NULL,
    input_tokens integer NOT NULL,
    output_tokens integer NOT NULL,
    total_tokens integer NOT NULL,
    cache_creation_input_tokens integer DEFAULT 0 NOT NULL,
    cache_read_input_tokens integer DEFAULT 0 NOT NULL,
    cost_estimate double precision,
    component character varying(100),
    request_id character varying(100),
    extra_metadata json
);

--
-- Name: COLUMN api_usage_log.cache_creation_input_tokens; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.api_usage_log.cache_creation_input_tokens IS 'Tokens written to cache (1.25x input token cost)';

--
-- Name: COLUMN api_usage_log.cache_read_input_tokens; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.api_usage_log.cache_read_input_tokens IS 'Tokens read from cache (0.1x input token cost - 90% savings)';

--
-- Name: api_usage_log_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE IF NOT EXISTS public.api_usage_log_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;

--
-- Name: api_usage_log_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.api_usage_log_id_seq OWNED BY public.api_usage_log.id;

--
-- Name: datasource_search_embeddings; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE IF NOT EXISTS public.datasource_search_embeddings (
    id integer NOT NULL,
    table_cache_id integer NOT NULL,
    workspace_id character varying(50) NOT NULL,
    project_id character varying(255) NOT NULL,
    dataset_id character varying(255) NOT NULL,
    table_id character varying(255) NOT NULL,
    entry_type character varying(50) NOT NULL,
    text text NOT NULL,
    weight double precision NOT NULL,
    column_name character varying(255),
    embedding public.vector(384) NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    datasource_config_id character varying(50)
);

--
-- Name: bigquery_search_embeddings_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE IF NOT EXISTS public.bigquery_search_embeddings_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;

--
-- Name: bigquery_search_embeddings_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.bigquery_search_embeddings_id_seq OWNED BY public.datasource_search_embeddings.id;

--
-- Name: datasource_table_cache; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE IF NOT EXISTS public.datasource_table_cache (
    id integer NOT NULL,
    workspace_id character varying(50) NOT NULL,
    project_id character varying(255) NOT NULL,
    dataset_id character varying(255) NOT NULL,
    table_id character varying(255) NOT NULL,
    table_metadata json NOT NULL,
    column_descriptions json,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    structure_refreshed_at timestamp with time zone,
    descriptions_refreshed_at timestamp with time zone,
    is_archived boolean DEFAULT false NOT NULL,
    last_verified timestamp with time zone,
    datasource_config_id character varying(50)
);

--
-- Name: COLUMN datasource_table_cache.is_archived; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.datasource_table_cache.is_archived IS 'True if table no longer exists in BigQuery (soft delete)';

--
-- Name: COLUMN datasource_table_cache.last_verified; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.datasource_table_cache.last_verified IS 'Last time we confirmed table still exists in BigQuery';

--
-- Name: bigquery_table_cache_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE IF NOT EXISTS public.bigquery_table_cache_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;

--
-- Name: bigquery_table_cache_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.bigquery_table_cache_id_seq OWNED BY public.datasource_table_cache.id;

--
-- Name: charts; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE IF NOT EXISTS public.charts (
    chart_id uuid NOT NULL,
    message_id character varying(50) NOT NULL,
    chart_data json NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

--
-- Name: chat_messages; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE IF NOT EXISTS public.chat_messages (
    message_id character varying(50) NOT NULL,
    session_id character varying(50) NOT NULL,
    role character varying(20) NOT NULL,
    content text NOT NULL,
    pinned boolean DEFAULT false NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    extra_metadata text,
    content_tsv tsvector,
    current_time_user_tz character varying(50),
    sent_by_user_id character varying(50),
    tool_call_id character varying(100),
    tool_name character varying(100),
    tool_calls jsonb
);

--
-- Name: COLUMN chat_messages.current_time_user_tz; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.chat_messages.current_time_user_tz IS 'User''s current time in their local timezone (ISO format with offset, no timezone name). Used by agent for relative time queries';

--
-- Name: COLUMN chat_messages.sent_by_user_id; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.chat_messages.sent_by_user_id IS 'User who sent this message (for shared conversations). NULL for assistant messages. Equals session.user_id for private conversations.';

--
-- Name: chat_sessions; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE IF NOT EXISTS public.chat_sessions (
    session_id character varying(50) NOT NULL,
    user_id character varying(50) NOT NULL,
    workspace_id character varying(50) NOT NULL,
    title character varying(255),
    model character varying(100),
    session_type character varying(50) DEFAULT 'chat'::character varying NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    config json,
    slack_channel_id character varying(50),
    slack_thread_ts character varying(50),
    shared boolean DEFAULT false,
    shared_at timestamp with time zone
);

--
-- Name: COLUMN chat_sessions.shared; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.chat_sessions.shared IS 'Whether this conversation is shared with workspace members';

--
-- Name: COLUMN chat_sessions.shared_at; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.chat_sessions.shared_at IS 'When the conversation was shared (NULL if never shared or unshared)';

--
-- Name: collection_dashboards; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE IF NOT EXISTS public.collection_dashboards (
    collection_id uuid NOT NULL,
    dashboard_id character varying(50) NOT NULL,
    "position" integer DEFAULT 0 NOT NULL,
    added_at timestamp with time zone DEFAULT now() NOT NULL
);

--
-- Name: collections; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE IF NOT EXISTS public.collections (
    id uuid NOT NULL,
    workspace_id character varying(50) NOT NULL,
    name character varying(255) NOT NULL,
    description text,
    color character varying(7),
    is_public boolean DEFAULT false NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

--
-- Name: conversation_read_status; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE IF NOT EXISTS public.conversation_read_status (
    id integer NOT NULL,
    session_id character varying(50) NOT NULL,
    user_id character varying(50) NOT NULL,
    last_read_at timestamp with time zone DEFAULT now() NOT NULL,
    last_read_message_id character varying(50)
);

--
-- Name: TABLE conversation_read_status; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON TABLE public.conversation_read_status IS 'Track read/unread state per user per conversation for shared conversations';

--
-- Name: COLUMN conversation_read_status.last_read_at; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.conversation_read_status.last_read_at IS 'When the user last marked this conversation as read';

--
-- Name: COLUMN conversation_read_status.last_read_message_id; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.conversation_read_status.last_read_message_id IS 'ID of the last message the user has read';

--
-- Name: conversation_read_status_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE IF NOT EXISTS public.conversation_read_status_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;

--
-- Name: conversation_read_status_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.conversation_read_status_id_seq OWNED BY public.conversation_read_status.id;

--
-- Name: dashboard_versions; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE IF NOT EXISTS public.dashboard_versions (
    version_id integer NOT NULL,
    dashboard_id character varying(50) NOT NULL,
    version_number integer NOT NULL,
    content text NOT NULL,
    title character varying(255) NOT NULL,
    change_summary character varying(500),
    created_by character varying(50) NOT NULL,
    created_at timestamp with time zone DEFAULT now(),
    content_hash character varying(64),
    byte_size integer
);

--
-- Name: TABLE dashboard_versions; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON TABLE public.dashboard_versions IS 'Version history for dashboards. Every save creates a new version.';

--
-- Name: COLUMN dashboard_versions.version_number; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.dashboard_versions.version_number IS 'Sequential version number per dashboard (1, 2, 3...)';

--
-- Name: COLUMN dashboard_versions.content; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.dashboard_versions.content IS 'Full dashboard content (ChartML markdown) at this version';

--
-- Name: COLUMN dashboard_versions.change_summary; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.dashboard_versions.change_summary IS 'Brief description of changes, auto-generated or user-provided';

--
-- Name: COLUMN dashboard_versions.content_hash; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.dashboard_versions.content_hash IS 'SHA-256 hash of content for deduplication and change detection';

--
-- Name: dashboard_versions_version_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE IF NOT EXISTS public.dashboard_versions_version_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;

--
-- Name: dashboard_versions_version_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.dashboard_versions_version_id_seq OWNED BY public.dashboard_versions.version_id;

--
-- Name: dashboard_views; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE IF NOT EXISTS public.dashboard_views (
    view_id character varying(50) NOT NULL,
    dashboard_id character varying(50) NOT NULL,
    user_id character varying(50) NOT NULL,
    workspace_id character varying(50) NOT NULL,
    viewed_at timestamp with time zone DEFAULT now() NOT NULL
);

--
-- Name: dashboards; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE IF NOT EXISTS public.dashboards (
    dashboard_id character varying(50) NOT NULL,
    user_id character varying(50) NOT NULL,
    workspace_id character varying(50) NOT NULL,
    title character varying(255) NOT NULL,
    content text DEFAULT ''::text NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    last_change_summary character varying(500),
    embedding public.vector(384),
    search_vector tsvector
);

--
-- Name: COLUMN dashboards.last_change_summary; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.dashboards.last_change_summary IS 'Summary of the most recent changes, migrated to version history on next save';

--
-- Name: datasource_configs; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE IF NOT EXISTS public.datasource_configs (
    id character varying(50) DEFAULT ('ds-'::text || SUBSTRING(md5((random())::text) FROM 1 FOR 24)) NOT NULL,
    workspace_id character varying(50) NOT NULL,
    name character varying(255) NOT NULL,
    datasource_type character varying(50) NOT NULL,
    connection_config jsonb DEFAULT '{}'::json NOT NULL,
    active boolean DEFAULT true,
    created_at timestamp with time zone DEFAULT now(),
    updated_at timestamp with time zone DEFAULT now(),
    slug character varying(100) NOT NULL,
    last_catalog_refresh timestamp with time zone,
    auto_refresh_allowed boolean DEFAULT true
);

--
-- Name: TABLE datasource_configs; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON TABLE public.datasource_configs IS 'Workspace-level datasource configurations. Stores connection parameters shared across users.';

--
-- Name: COLUMN datasource_configs.connection_config; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.datasource_configs.connection_config IS 'Provider-specific connection parameters (e.g., host, port, project). May include workspace credentials for service account mode.';

--
-- Name: email_subscribers; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE IF NOT EXISTS public.email_subscribers (
    id integer NOT NULL,
    email character varying(255) NOT NULL,
    company_name character varying(255),
    company_size character varying(50),
    use_case character varying(100),
    marketing_consent boolean DEFAULT false,
    created_at timestamp without time zone DEFAULT CURRENT_TIMESTAMP,
    updated_at timestamp without time zone DEFAULT CURRENT_TIMESTAMP,
    source character varying(50) DEFAULT 'web'::character varying,
    notified boolean DEFAULT false,
    notified_at timestamp without time zone
);

--
-- Name: TABLE email_subscribers; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON TABLE public.email_subscribers IS 'Email subscribers for product updates, beta waitlist, and newsletters';

--
-- Name: COLUMN email_subscribers.source; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.email_subscribers.source IS 'Where the subscriber signed up from (web, marketing_site, etc.)';

--
-- Name: COLUMN email_subscribers.notified; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.email_subscribers.notified IS 'Whether we have notified them of launch/updates';

--
-- Name: email_subscribers_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE IF NOT EXISTS public.email_subscribers_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;

--
-- Name: email_subscribers_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.email_subscribers_id_seq OWNED BY public.email_subscribers.id;

--
-- Name: feedback; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE IF NOT EXISTS public.feedback (
    id character varying(50) NOT NULL,
    user_id character varying(50) NOT NULL,
    workspace_id character varying(50),
    type character varying(20) NOT NULL,
    description text NOT NULL,
    screenshot_url character varying(500),
    include_context boolean DEFAULT true NOT NULL,
    context json,
    status character varying(20) DEFAULT 'new'::character varying NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    resolved_at timestamp with time zone,
    resolution_notes text,
    resolved_by character varying(50)
);

--
-- Name: notifications; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE IF NOT EXISTS public.notifications (
    id character varying(50) NOT NULL,
    user_id character varying(50) NOT NULL,
    workspace_id character varying(50) NOT NULL,
    type character varying(50) NOT NULL,
    title character varying(255) NOT NULL,
    message text NOT NULL,
    data jsonb,
    source_watch_id character varying(50),
    read boolean DEFAULT false,
    dismissed boolean DEFAULT false,
    created_at timestamp with time zone DEFAULT now(),
    read_at timestamp with time zone
);

--
-- Name: TABLE notifications; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON TABLE public.notifications IS 'Persistent user notifications for watch alerts and system messages';

--
-- Name: oauth_clients; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE IF NOT EXISTS public.oauth_clients (
    id uuid NOT NULL,
    client_id character varying(255) NOT NULL,
    client_secret_hash character varying(255),
    name character varying(255) NOT NULL,
    redirect_uris jsonb DEFAULT '[]'::jsonb NOT NULL,
    scopes jsonb DEFAULT '[]'::jsonb NOT NULL,
    client_type character varying(50) DEFAULT 'public'::character varying NOT NULL,
    active boolean DEFAULT true NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);

--
-- Name: oauth_states; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE IF NOT EXISTS public.oauth_states (
    state character varying(64) NOT NULL,
    user_id character varying(64) NOT NULL,
    action character varying(32) NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);

--
-- Name: TABLE oauth_states; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON TABLE public.oauth_states IS 'Stores OAuth flow state parameters for CSRF protection across multiple workers';

--
-- Name: COLUMN oauth_states.state; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.oauth_states.state IS 'Unique state token for OAuth flow (URL-safe random string)';

--
-- Name: COLUMN oauth_states.user_id; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.oauth_states.user_id IS 'User ID for account linking flows';

--
-- Name: COLUMN oauth_states.action; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.oauth_states.action IS 'OAuth action type (e.g., link_account)';

--
-- Name: COLUMN oauth_states.created_at; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.oauth_states.created_at IS 'When the state was created (for TTL cleanup)';

--
-- Name: ownership_transfers; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE IF NOT EXISTS public.ownership_transfers (
    transfer_id character varying(50) NOT NULL,
    workspace_id character varying(50) NOT NULL,
    from_user_id character varying(50) NOT NULL,
    to_user_id character varying(50) NOT NULL,
    status character varying(20) DEFAULT 'pending'::character varying NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    expires_at timestamp with time zone DEFAULT (now() + '7 days'::interval) NOT NULL,
    completed_at timestamp with time zone
);

--
-- Name: query_cache; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE IF NOT EXISTS public.query_cache (
    query_id character varying(64) NOT NULL,
    sql text NOT NULL,
    last_accessed_at timestamp with time zone DEFAULT now() NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);

--
-- Name: refresh_tokens; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE IF NOT EXISTS public.refresh_tokens (
    token_id character varying(100) NOT NULL,
    user_id character varying(50) NOT NULL,
    token_hash character varying(255) NOT NULL,
    demo_token_value character varying(500),
    expires_at timestamp with time zone NOT NULL,
    is_active boolean DEFAULT true NOT NULL,
    revoked_at timestamp with time zone,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    last_used timestamp with time zone,
    user_agent character varying(500),
    ip_address character varying(45),
    oauth_client_id character varying(255),
    country_code character varying(10)
);

--
-- Name: COLUMN refresh_tokens.demo_token_value; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.refresh_tokens.demo_token_value IS 'DEMO mode only: stores unhashed refresh token for e2e testing. NULL in production.';

--
-- Name: sql_query_history; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE IF NOT EXISTS public.sql_query_history (
    query_id character varying(50) NOT NULL,
    workspace_id character varying(50) NOT NULL,
    user_id character varying(50) NOT NULL,
    query_text text NOT NULL,
    executed_at timestamp with time zone DEFAULT now() NOT NULL,
    execution_time_ms integer,
    bytes_processed bigint,
    row_count integer,
    status character varying(20) NOT NULL,
    error_message text,
    is_saved boolean DEFAULT false NOT NULL,
    query_name character varying(255),
    tags character varying(500),
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    datasource_config_id character varying(50)
);

--
-- Name: TABLE sql_query_history; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON TABLE public.sql_query_history IS 'SQL query execution history for query reuse and analytics';

--
-- Name: COLUMN sql_query_history.query_text; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.sql_query_history.query_text IS 'The SQL query that was executed';

--
-- Name: COLUMN sql_query_history.execution_time_ms; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.sql_query_history.execution_time_ms IS 'Query duration in milliseconds';

--
-- Name: COLUMN sql_query_history.bytes_processed; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.sql_query_history.bytes_processed IS 'BigQuery bytes scanned';

--
-- Name: COLUMN sql_query_history.row_count; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.sql_query_history.row_count IS 'Number of rows returned';

--
-- Name: COLUMN sql_query_history.status; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.sql_query_history.status IS 'Query execution status: success or error';

--
-- Name: COLUMN sql_query_history.is_saved; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.sql_query_history.is_saved IS 'Whether this query has been starred/saved by user';

--
-- Name: COLUMN sql_query_history.query_name; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.sql_query_history.query_name IS 'Optional custom name for saved queries';

--
-- Name: COLUMN sql_query_history.tags; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.sql_query_history.tags IS 'Comma-separated tags for organizing queries';

--
-- Name: sql_query_search_embeddings; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE IF NOT EXISTS public.sql_query_search_embeddings (
    id integer NOT NULL,
    query_id character varying(50) NOT NULL,
    workspace_id character varying(50) NOT NULL,
    user_id character varying(50) NOT NULL,
    search_text text NOT NULL,
    embedding public.vector(384) NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);

--
-- Name: TABLE sql_query_search_embeddings; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON TABLE public.sql_query_search_embeddings IS 'Semantic search embeddings for SQL query history using pgvector';

--
-- Name: sql_query_search_embeddings_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE IF NOT EXISTS public.sql_query_search_embeddings_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;

--
-- Name: sql_query_search_embeddings_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.sql_query_search_embeddings_id_seq OWNED BY public.sql_query_search_embeddings.id;

--
-- Name: user_auth_methods; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE IF NOT EXISTS public.user_auth_methods (
    id integer NOT NULL,
    user_id character varying(50) NOT NULL,
    auth_type character varying(50) NOT NULL,
    auth_data json NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    last_used timestamp with time zone,
    active boolean DEFAULT true NOT NULL
);

--
-- Name: user_auth_methods_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE IF NOT EXISTS public.user_auth_methods_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;

--
-- Name: user_auth_methods_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.user_auth_methods_id_seq OWNED BY public.user_auth_methods.id;

--
-- Name: user_datasource_credentials; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE IF NOT EXISTS public.user_datasource_credentials (
    id integer NOT NULL,
    user_id character varying(50) NOT NULL,
    datasource_config_id character varying(50) NOT NULL,
    workspace_id character varying(50) NOT NULL,
    credentials text NOT NULL,
    created_at timestamp with time zone DEFAULT now(),
    updated_at timestamp with time zone DEFAULT now(),
    enabled boolean DEFAULT true NOT NULL
);

--
-- Name: TABLE user_datasource_credentials; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON TABLE public.user_datasource_credentials IS 'Per-user credentials for datasource access. Enables personal credential mode where each user authenticates with their own identity.';

--
-- Name: COLUMN user_datasource_credentials.credentials; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.user_datasource_credentials.credentials IS 'Encrypted provider-specific credentials. For BigQuery includes billing_project. For ClickHouse includes username/password.';

--
-- Name: COLUMN user_datasource_credentials.enabled; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.user_datasource_credentials.enabled IS 'User preference: whether this datasource is enabled for the user. Allows users to disable datasources they do not want to use.';

--
-- Name: user_datasource_credentials_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE IF NOT EXISTS public.user_datasource_credentials_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;

--
-- Name: user_datasource_credentials_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.user_datasource_credentials_id_seq OWNED BY public.user_datasource_credentials.id;

--
-- Name: user_datasource_preferences; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE IF NOT EXISTS public.user_datasource_preferences (
    id integer NOT NULL,
    user_id character varying(50) NOT NULL,
    datasource_config_id character varying(50) NOT NULL,
    enabled boolean DEFAULT true NOT NULL,
    created_at timestamp with time zone DEFAULT now(),
    updated_at timestamp with time zone DEFAULT now()
);

--
-- Name: TABLE user_datasource_preferences; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON TABLE public.user_datasource_preferences IS 'User preferences for shared-auth datasources. For datasources using workspace credentials (service account mode), users can set their enabled preference here without needing personal credentials.';

--
-- Name: COLUMN user_datasource_preferences.enabled; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.user_datasource_preferences.enabled IS 'User preference: whether this datasource is enabled for the user.';

--
-- Name: user_datasource_preferences_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE IF NOT EXISTS public.user_datasource_preferences_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;

--
-- Name: user_datasource_preferences_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.user_datasource_preferences_id_seq OWNED BY public.user_datasource_preferences.id;

--
-- Name: users; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE IF NOT EXISTS public.users (
    user_id character varying(50) NOT NULL,
    email character varying(255) NOT NULL,
    name character varying(255),
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    last_login timestamp with time zone,
    active boolean DEFAULT true NOT NULL,
    verified boolean DEFAULT false NOT NULL,
    terms_accepted_at timestamp with time zone,
    terms_accepted_version character varying(50),
    marketing_consent boolean DEFAULT false NOT NULL,
    oauth_data text,
    extra_metadata json,
    chartml_config json,
    last_workspace_id character varying(50),
    knowledge text,
    billing_project character varying(255),
    default_project character varying(255),
    query_size_limit_gb integer DEFAULT 50 NOT NULL
);

--
-- Name: COLUMN users.terms_accepted_at; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.users.terms_accepted_at IS 'Timestamp when user accepted Terms of Service and Privacy Policy. NULL means user has not accepted (legacy users or incomplete signup).';

--
-- Name: COLUMN users.terms_accepted_version; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.users.terms_accepted_version IS 'Version of terms accepted (e.g., "2025-11-16"). Used to track which version of legal documents user agreed to.';

--
-- Name: COLUMN users.chartml_config; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.users.chartml_config IS 'User-level ChartML configuration (YAML/JSON). Contains type: config and type: source blocks for custom chart styling and data sources.';

--
-- Name: COLUMN users.last_workspace_id; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.users.last_workspace_id IS 'ID of the last workspace accessed by this user. Used to restore user to their last workspace on login.';

--
-- Name: COLUMN users.billing_project; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.users.billing_project IS 'BigQuery project ID to bill queries to (user-level preference)';

--
-- Name: COLUMN users.default_project; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.users.default_project IS 'Default BigQuery project ID for queries (user-level preference)';

--
-- Name: verification_tokens; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE IF NOT EXISTS public.verification_tokens (
    token_id character varying(100) NOT NULL,
    email character varying(255) NOT NULL,
    token_hash character varying(255) NOT NULL,
    token_type character varying(50) NOT NULL,
    expires_at timestamp with time zone NOT NULL,
    used boolean DEFAULT false NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    used_at timestamp with time zone
);

--
-- Name: watch_executions; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE IF NOT EXISTS public.watch_executions (
    id integer NOT NULL,
    watch_id character varying(50),
    started_at timestamp with time zone DEFAULT now(),
    completed_at timestamp with time zone,
    status character varying(20) NOT NULL,
    agent_response text,
    error_message text,
    input_tokens integer DEFAULT 0,
    output_tokens integer DEFAULT 0,
    cost_estimate double precision,
    execution_trace jsonb,
    alert_triggered boolean DEFAULT false,
    notification_id character varying(50),
    watch_name character varying(200),
    workspace_id character varying(50),
    deleted_at timestamp with time zone,
    deleted_by character varying(50),
    read_at timestamp with time zone,
    dismissed_at timestamp with time zone,
    dismissed_by character varying(50),
    mode character varying(20),
    session_id character varying(50)
);

--
-- Name: TABLE watch_executions; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON TABLE public.watch_executions IS 'Execution logs for watch runs including agent response and token usage';

--
-- Name: COLUMN watch_executions.status; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.watch_executions.status IS 'running, success, error, or no_alert';

--
-- Name: COLUMN watch_executions.watch_name; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.watch_executions.watch_name IS 'Snapshot of watch name at execution time. Preserved even when watch is deleted.';

--
-- Name: COLUMN watch_executions.workspace_id; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.watch_executions.workspace_id IS 'Workspace ID snapshot. Preserved for filtering after watch is deleted.';

--
-- Name: COLUMN watch_executions.deleted_at; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.watch_executions.deleted_at IS 'Timestamp when alert was deleted by user (purged after 30 days)';

--
-- Name: COLUMN watch_executions.deleted_by; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.watch_executions.deleted_by IS 'User ID who deleted the alert';

--
-- Name: COLUMN watch_executions.read_at; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.watch_executions.read_at IS 'Timestamp when alert was first opened/viewed by user';

--
-- Name: watch_executions_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE IF NOT EXISTS public.watch_executions_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;

--
-- Name: watch_executions_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.watch_executions_id_seq OWNED BY public.watch_executions.id;

--
-- Name: watches; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE IF NOT EXISTS public.watches (
    watch_id character varying(50) NOT NULL,
    workspace_id character varying(50) NOT NULL,
    created_by character varying(50) NOT NULL,
    name character varying(255) NOT NULL,
    prompt text NOT NULL,
    schedule character varying(100) NOT NULL,
    datasource_hints jsonb,
    enabled boolean DEFAULT true,
    last_run_at timestamp with time zone,
    last_run_status character varying(20),
    next_run_at timestamp with time zone,
    created_at timestamp with time zone DEFAULT now(),
    updated_at timestamp with time zone DEFAULT now(),
    slack_channel_id character varying(50),
    alert_emails_enabled boolean DEFAULT false NOT NULL,
    alert_emails character varying(500),
    mode character varying(20) DEFAULT 'alert'::character varying,
    queries jsonb
);

--
-- Name: TABLE watches; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON TABLE public.watches IS 'Data watches for proactive monitoring with scheduled AI agent runs';

--
-- Name: COLUMN watches.schedule; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.watches.schedule IS 'Cron expression in UTC (e.g., "0 9 * * *" for daily at 9 AM UTC)';

--
-- Name: COLUMN watches.slack_channel_id; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.watches.slack_channel_id IS 'Slack channel ID where alerts should be posted';

--
-- Name: COLUMN watches.alert_emails_enabled; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.watches.alert_emails_enabled IS 'Whether email alerts are enabled. Allows temporarily disabling without clearing the email list.';

--
-- Name: COLUMN watches.alert_emails; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.watches.alert_emails IS 'Comma-separated list of email addresses to send alerts to. If empty, no email alerts are sent.';

--
-- Name: COLUMN watches.queries; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.watches.queries IS 'Array of pre-determined queries [{comment: string, sql: string, datasource: string|null}] discovered by copilot. Optional - agent uses as reference. Datasource is optional slug of the target datasource.';

--
-- Name: workspace_invitations; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE IF NOT EXISTS public.workspace_invitations (
    invitation_id character varying(50) NOT NULL,
    workspace_id character varying(50) NOT NULL,
    email character varying(255) NOT NULL,
    role character varying(20) DEFAULT 'user'::character varying NOT NULL,
    invited_by_user_id character varying(50) NOT NULL,
    status character varying(20) DEFAULT 'pending'::character varying NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    expires_at timestamp with time zone DEFAULT (now() + '7 days'::interval) NOT NULL,
    accepted_at timestamp with time zone,
    accepted_by_user_id character varying(50)
);

--
-- Name: TABLE workspace_invitations; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON TABLE public.workspace_invitations IS 'Stores workspace invitation links for multi-user collaboration';

--
-- Name: COLUMN workspace_invitations.role; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.workspace_invitations.role IS 'Role assigned to user upon acceptance (admin or user)';

--
-- Name: COLUMN workspace_invitations.expires_at; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.workspace_invitations.expires_at IS 'Invitations expire after 7 days';

--
-- Name: workspace_knowledge_chunks; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE IF NOT EXISTS public.workspace_knowledge_chunks (
    chunk_id uuid DEFAULT gen_random_uuid() NOT NULL,
    workspace_id character varying(50) NOT NULL,
    chunk_text text NOT NULL,
    chunk_index integer NOT NULL,
    embedding public.vector(384),
    created_at timestamp with time zone DEFAULT now()
);

--
-- Name: TABLE workspace_knowledge_chunks; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON TABLE public.workspace_knowledge_chunks IS 'Stores chunked and embedded workspace knowledge for semantic search. Used by AI agent for context injection.';

--
-- Name: workspace_usage; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE IF NOT EXISTS public.workspace_usage (
    id integer NOT NULL,
    workspace_id character varying(50) NOT NULL,
    usage_date timestamp with time zone NOT NULL,
    api_calls integer DEFAULT 0 NOT NULL,
    tokens_used integer DEFAULT 0 NOT NULL,
    storage_bytes integer DEFAULT 0 NOT NULL,
    metrics json
);

--
-- Name: workspace_usage_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE IF NOT EXISTS public.workspace_usage_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;

--
-- Name: workspace_usage_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.workspace_usage_id_seq OWNED BY public.workspace_usage.id;

--
-- Name: workspace_users; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE IF NOT EXISTS public.workspace_users (
    id integer NOT NULL,
    workspace_id character varying(50) NOT NULL,
    user_id character varying(50) NOT NULL,
    role character varying(50) DEFAULT 'user'::character varying NOT NULL,
    active boolean DEFAULT true NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    last_active timestamp with time zone,
    extra_metadata json,
    slack_user_id character varying(50),
    slack_username character varying(255),
    slack_connected_at timestamp with time zone,
    slack_default_channel_id character varying(50),
    slack_default_channel_name character varying(255),
    slack_user_token text,
    slack_user_refresh_token text,
    slack_user_token_expires_at timestamp with time zone,
    slack_timezone character varying(100),
    slack_timezone_fetched_at timestamp with time zone
);

--
-- Name: COLUMN workspace_users.slack_timezone; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.workspace_users.slack_timezone IS 'User timezone from Slack (e.g., America/Los_Angeles), cached for 24 hours';

--
-- Name: COLUMN workspace_users.slack_timezone_fetched_at; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.workspace_users.slack_timezone_fetched_at IS 'When the timezone was last fetched from Slack API';

--
-- Name: workspace_users_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE IF NOT EXISTS public.workspace_users_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;

--
-- Name: workspace_users_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.workspace_users_id_seq OWNED BY public.workspace_users.id;

--
-- Name: workspaces; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE IF NOT EXISTS public.workspaces (
    workspace_id character varying(50) NOT NULL,
    name character varying(255),
    domain character varying(255),
    status character varying(50) DEFAULT 'trial'::character varying NOT NULL,
    admin_email character varying(255),
    owner_user_id character varying(50) NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    subscription_tier character varying(20) DEFAULT 'free'::character varying NOT NULL,
    subscription_status character varying(20) DEFAULT 'active'::character varying NOT NULL,
    billing_cycle character varying(10),
    subscription_period_start timestamp with time zone,
    subscription_period_end timestamp with time zone,
    trial_ends_at timestamp with time zone,
    ai_credits_used_usd double precision DEFAULT 0.0 NOT NULL,
    user_limit integer DEFAULT 1,
    stripe_customer_id character varying(100),
    stripe_subscription_id character varying(100),
    stripe_additional_users_item_id character varying(100),
    settings json,
    business_knowledge text DEFAULT ''::text,
    knowledge_updated_at timestamp with time zone,
    last_catalog_refresh timestamp with time zone,
    catalog_refresh_status character varying(50) DEFAULT 'idle'::character varying,
    catalog_refresh_progress json,
    catalog_onboarding_completed boolean DEFAULT false NOT NULL,
    catalog_indexed_projects json DEFAULT '[]'::json,
    slack_team_id character varying(50),
    slack_team_name character varying(255),
    slack_bot_token text,
    slack_bot_user_id character varying(50),
    slack_installed_by_user_id character varying(50),
    slack_installed_at timestamp with time zone
);

--
-- Name: COLUMN workspaces.subscription_tier; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.workspaces.subscription_tier IS 'Subscription tier: free, basic, pro, team, enterprise';

--
-- Name: COLUMN workspaces.subscription_status; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.workspaces.subscription_status IS 'Subscription status: trialing, active, past_due, cancelled';

--
-- Name: COLUMN workspaces.billing_cycle; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.workspaces.billing_cycle IS 'Billing cycle: annual or monthly (null for free tier)';

--
-- Name: COLUMN workspaces.subscription_period_start; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.workspaces.subscription_period_start IS 'Current billing period start date';

--
-- Name: COLUMN workspaces.subscription_period_end; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.workspaces.subscription_period_end IS 'Current billing period end date (for credit reset)';

--
-- Name: COLUMN workspaces.trial_ends_at; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.workspaces.trial_ends_at IS 'Timestamp when free tier trial expires (7 days from workspace creation)';

--
-- Name: COLUMN workspaces.ai_credits_used_usd; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.workspaces.ai_credits_used_usd IS 'AI dollars spent this billing period (resets on period_end)';

--
-- Name: COLUMN workspaces.user_limit; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.workspaces.user_limit IS 'Maximum active users allowed in workspace. Set by billing system. NULL defaults to 1.';

--
-- Name: COLUMN workspaces.stripe_customer_id; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.workspaces.stripe_customer_id IS 'Stripe customer ID for payment processing';

--
-- Name: COLUMN workspaces.stripe_subscription_id; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.workspaces.stripe_subscription_id IS 'Stripe subscription ID for subscription management';

--
-- Name: COLUMN workspaces.stripe_additional_users_item_id; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.workspaces.stripe_additional_users_item_id IS 'Stripe subscription item ID for additional users line item (Team tier only). Used to update quantity when purchasing more user seats.';

--
-- Name: COLUMN workspaces.business_knowledge; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.workspaces.business_knowledge IS 'Markdown document containing workspace-specific business context, metrics, and data notes.';

--
-- Name: COLUMN workspaces.last_catalog_refresh; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.workspaces.last_catalog_refresh IS 'Last time workspace catalog was refreshed from BigQuery';

--
-- Name: COLUMN workspaces.catalog_refresh_status; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.workspaces.catalog_refresh_status IS 'Current status of catalog refresh: idle, running, failed';

--
-- Name: COLUMN workspaces.catalog_refresh_progress; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.workspaces.catalog_refresh_progress IS 'JSON object tracking refresh progress: {total_tables, processed, status}';

--
-- Name: COLUMN workspaces.catalog_onboarding_completed; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.workspaces.catalog_onboarding_completed IS 'True if workspace owner has completed catalog onboarding (one-time setup)';

--
-- Name: COLUMN workspaces.catalog_indexed_projects; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.workspaces.catalog_indexed_projects IS 'JSON array of BigQuery project IDs selected for catalog indexing';

--
-- Name: api_usage_log id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.api_usage_log ALTER COLUMN id SET DEFAULT nextval('public.api_usage_log_id_seq'::regclass);

--
-- Name: conversation_read_status id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.conversation_read_status ALTER COLUMN id SET DEFAULT nextval('public.conversation_read_status_id_seq'::regclass);

--
-- Name: dashboard_versions version_id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dashboard_versions ALTER COLUMN version_id SET DEFAULT nextval('public.dashboard_versions_version_id_seq'::regclass);

--
-- Name: datasource_search_embeddings id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.datasource_search_embeddings ALTER COLUMN id SET DEFAULT nextval('public.bigquery_search_embeddings_id_seq'::regclass);

--
-- Name: datasource_table_cache id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.datasource_table_cache ALTER COLUMN id SET DEFAULT nextval('public.bigquery_table_cache_id_seq'::regclass);

--
-- Name: email_subscribers id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.email_subscribers ALTER COLUMN id SET DEFAULT nextval('public.email_subscribers_id_seq'::regclass);

--
-- Name: sql_query_search_embeddings id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.sql_query_search_embeddings ALTER COLUMN id SET DEFAULT nextval('public.sql_query_search_embeddings_id_seq'::regclass);

--
-- Name: user_auth_methods id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.user_auth_methods ALTER COLUMN id SET DEFAULT nextval('public.user_auth_methods_id_seq'::regclass);

--
-- Name: user_datasource_credentials id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.user_datasource_credentials ALTER COLUMN id SET DEFAULT nextval('public.user_datasource_credentials_id_seq'::regclass);

--
-- Name: user_datasource_preferences id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.user_datasource_preferences ALTER COLUMN id SET DEFAULT nextval('public.user_datasource_preferences_id_seq'::regclass);

--
-- Name: watch_executions id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.watch_executions ALTER COLUMN id SET DEFAULT nextval('public.watch_executions_id_seq'::regclass);

--
-- Name: workspace_usage id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.workspace_usage ALTER COLUMN id SET DEFAULT nextval('public.workspace_usage_id_seq'::regclass);

--
-- Name: workspace_users id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.workspace_users ALTER COLUMN id SET DEFAULT nextval('public.workspace_users_id_seq'::regclass);

--
-- Name: agent_learnings agent_learnings_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $$ BEGIN
    ALTER TABLE ONLY public.agent_learnings
    ADD CONSTRAINT agent_learnings_pkey PRIMARY KEY (learning_id);
EXCEPTION
    WHEN duplicate_object THEN NULL;
    WHEN duplicate_table THEN NULL;
    WHEN invalid_table_definition THEN NULL;
END $$;

--
-- Name: api_tokens api_tokens_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $$ BEGIN
    ALTER TABLE ONLY public.api_tokens
    ADD CONSTRAINT api_tokens_pkey PRIMARY KEY (token_id);
EXCEPTION
    WHEN duplicate_object THEN NULL;
    WHEN duplicate_table THEN NULL;
    WHEN invalid_table_definition THEN NULL;
END $$;

--
-- Name: api_usage_log api_usage_log_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $$ BEGIN
    ALTER TABLE ONLY public.api_usage_log
    ADD CONSTRAINT api_usage_log_pkey PRIMARY KEY (id);
EXCEPTION
    WHEN duplicate_object THEN NULL;
    WHEN duplicate_table THEN NULL;
    WHEN invalid_table_definition THEN NULL;
END $$;

--
-- Name: datasource_search_embeddings bigquery_search_embeddings_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $$ BEGIN
    ALTER TABLE ONLY public.datasource_search_embeddings
    ADD CONSTRAINT bigquery_search_embeddings_pkey PRIMARY KEY (id);
EXCEPTION
    WHEN duplicate_object THEN NULL;
    WHEN duplicate_table THEN NULL;
    WHEN invalid_table_definition THEN NULL;
END $$;

--
-- Name: datasource_table_cache bigquery_table_cache_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $$ BEGIN
    ALTER TABLE ONLY public.datasource_table_cache
    ADD CONSTRAINT bigquery_table_cache_pkey PRIMARY KEY (id);
EXCEPTION
    WHEN duplicate_object THEN NULL;
    WHEN duplicate_table THEN NULL;
    WHEN invalid_table_definition THEN NULL;
END $$;

--
-- Name: charts charts_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $$ BEGIN
    ALTER TABLE ONLY public.charts
    ADD CONSTRAINT charts_pkey PRIMARY KEY (chart_id);
EXCEPTION
    WHEN duplicate_object THEN NULL;
    WHEN duplicate_table THEN NULL;
    WHEN invalid_table_definition THEN NULL;
END $$;

--
-- Name: chat_messages chat_messages_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $$ BEGIN
    ALTER TABLE ONLY public.chat_messages
    ADD CONSTRAINT chat_messages_pkey PRIMARY KEY (message_id);
EXCEPTION
    WHEN duplicate_object THEN NULL;
    WHEN duplicate_table THEN NULL;
    WHEN invalid_table_definition THEN NULL;
END $$;

--
-- Name: chat_sessions chat_sessions_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $$ BEGIN
    ALTER TABLE ONLY public.chat_sessions
    ADD CONSTRAINT chat_sessions_pkey PRIMARY KEY (session_id);
EXCEPTION
    WHEN duplicate_object THEN NULL;
    WHEN duplicate_table THEN NULL;
    WHEN invalid_table_definition THEN NULL;
END $$;

--
-- Name: collection_dashboards collection_dashboards_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $$ BEGIN
    ALTER TABLE ONLY public.collection_dashboards
    ADD CONSTRAINT collection_dashboards_pkey PRIMARY KEY (collection_id, dashboard_id);
EXCEPTION
    WHEN duplicate_object THEN NULL;
    WHEN duplicate_table THEN NULL;
    WHEN invalid_table_definition THEN NULL;
END $$;

--
-- Name: collections collections_name_workspace_unique; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $$ BEGIN
    ALTER TABLE ONLY public.collections
    ADD CONSTRAINT collections_name_workspace_unique UNIQUE (workspace_id, name);
EXCEPTION
    WHEN duplicate_object THEN NULL;
    WHEN duplicate_table THEN NULL;
    WHEN invalid_table_definition THEN NULL;
END $$;

--
-- Name: collections collections_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $$ BEGIN
    ALTER TABLE ONLY public.collections
    ADD CONSTRAINT collections_pkey PRIMARY KEY (id);
EXCEPTION
    WHEN duplicate_object THEN NULL;
    WHEN duplicate_table THEN NULL;
    WHEN invalid_table_definition THEN NULL;
END $$;

--
-- Name: conversation_read_status conversation_read_status_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $$ BEGIN
    ALTER TABLE ONLY public.conversation_read_status
    ADD CONSTRAINT conversation_read_status_pkey PRIMARY KEY (id);
EXCEPTION
    WHEN duplicate_object THEN NULL;
    WHEN duplicate_table THEN NULL;
    WHEN invalid_table_definition THEN NULL;
END $$;

--
-- Name: dashboard_versions dashboard_versions_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $$ BEGIN
    ALTER TABLE ONLY public.dashboard_versions
    ADD CONSTRAINT dashboard_versions_pkey PRIMARY KEY (version_id);
EXCEPTION
    WHEN duplicate_object THEN NULL;
    WHEN duplicate_table THEN NULL;
    WHEN invalid_table_definition THEN NULL;
END $$;

--
-- Name: dashboard_views dashboard_views_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $$ BEGIN
    ALTER TABLE ONLY public.dashboard_views
    ADD CONSTRAINT dashboard_views_pkey PRIMARY KEY (view_id);
EXCEPTION
    WHEN duplicate_object THEN NULL;
    WHEN duplicate_table THEN NULL;
    WHEN invalid_table_definition THEN NULL;
END $$;

--
-- Name: dashboards dashboards_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $$ BEGIN
    ALTER TABLE ONLY public.dashboards
    ADD CONSTRAINT dashboards_pkey PRIMARY KEY (dashboard_id);
EXCEPTION
    WHEN duplicate_object THEN NULL;
    WHEN duplicate_table THEN NULL;
    WHEN invalid_table_definition THEN NULL;
END $$;

--
-- Name: datasource_configs datasource_configs_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $$ BEGIN
    ALTER TABLE ONLY public.datasource_configs
    ADD CONSTRAINT datasource_configs_pkey PRIMARY KEY (id);
EXCEPTION
    WHEN duplicate_object THEN NULL;
    WHEN duplicate_table THEN NULL;
    WHEN invalid_table_definition THEN NULL;
END $$;

--
-- Name: email_subscribers email_subscribers_email_key; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $$ BEGIN
    ALTER TABLE ONLY public.email_subscribers
    ADD CONSTRAINT email_subscribers_email_key UNIQUE (email);
EXCEPTION
    WHEN duplicate_object THEN NULL;
    WHEN duplicate_table THEN NULL;
    WHEN invalid_table_definition THEN NULL;
END $$;

--
-- Name: email_subscribers email_subscribers_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $$ BEGIN
    ALTER TABLE ONLY public.email_subscribers
    ADD CONSTRAINT email_subscribers_pkey PRIMARY KEY (id);
EXCEPTION
    WHEN duplicate_object THEN NULL;
    WHEN duplicate_table THEN NULL;
    WHEN invalid_table_definition THEN NULL;
END $$;

--
-- Name: feedback feedback_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $$ BEGIN
    ALTER TABLE ONLY public.feedback
    ADD CONSTRAINT feedback_pkey PRIMARY KEY (id);
EXCEPTION
    WHEN duplicate_object THEN NULL;
    WHEN duplicate_table THEN NULL;
    WHEN invalid_table_definition THEN NULL;
END $$;

--
-- Name: notifications notifications_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $$ BEGIN
    ALTER TABLE ONLY public.notifications
    ADD CONSTRAINT notifications_pkey PRIMARY KEY (id);
EXCEPTION
    WHEN duplicate_object THEN NULL;
    WHEN duplicate_table THEN NULL;
    WHEN invalid_table_definition THEN NULL;
END $$;

--
-- Name: oauth_clients oauth_clients_client_id_key; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $$ BEGIN
    ALTER TABLE ONLY public.oauth_clients
    ADD CONSTRAINT oauth_clients_client_id_key UNIQUE (client_id);
EXCEPTION
    WHEN duplicate_object THEN NULL;
    WHEN duplicate_table THEN NULL;
    WHEN invalid_table_definition THEN NULL;
END $$;

--
-- Name: oauth_clients oauth_clients_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $$ BEGIN
    ALTER TABLE ONLY public.oauth_clients
    ADD CONSTRAINT oauth_clients_pkey PRIMARY KEY (id);
EXCEPTION
    WHEN duplicate_object THEN NULL;
    WHEN duplicate_table THEN NULL;
    WHEN invalid_table_definition THEN NULL;
END $$;

--
-- Name: oauth_states oauth_states_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $$ BEGIN
    ALTER TABLE ONLY public.oauth_states
    ADD CONSTRAINT oauth_states_pkey PRIMARY KEY (state);
EXCEPTION
    WHEN duplicate_object THEN NULL;
    WHEN duplicate_table THEN NULL;
    WHEN invalid_table_definition THEN NULL;
END $$;

--
-- Name: ownership_transfers ownership_transfers_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $$ BEGIN
    ALTER TABLE ONLY public.ownership_transfers
    ADD CONSTRAINT ownership_transfers_pkey PRIMARY KEY (transfer_id);
EXCEPTION
    WHEN duplicate_object THEN NULL;
    WHEN duplicate_table THEN NULL;
    WHEN invalid_table_definition THEN NULL;
END $$;

--
-- Name: query_cache query_cache_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $$ BEGIN
    ALTER TABLE ONLY public.query_cache
    ADD CONSTRAINT query_cache_pkey PRIMARY KEY (query_id);
EXCEPTION
    WHEN duplicate_object THEN NULL;
    WHEN duplicate_table THEN NULL;
    WHEN invalid_table_definition THEN NULL;
END $$;

--
-- Name: refresh_tokens refresh_tokens_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $$ BEGIN
    ALTER TABLE ONLY public.refresh_tokens
    ADD CONSTRAINT refresh_tokens_pkey PRIMARY KEY (token_id);
EXCEPTION
    WHEN duplicate_object THEN NULL;
    WHEN duplicate_table THEN NULL;
    WHEN invalid_table_definition THEN NULL;
END $$;

--
-- Name: sql_query_history sql_query_history_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $$ BEGIN
    ALTER TABLE ONLY public.sql_query_history
    ADD CONSTRAINT sql_query_history_pkey PRIMARY KEY (query_id);
EXCEPTION
    WHEN duplicate_object THEN NULL;
    WHEN duplicate_table THEN NULL;
    WHEN invalid_table_definition THEN NULL;
END $$;

--
-- Name: sql_query_search_embeddings sql_query_search_embeddings_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $$ BEGIN
    ALTER TABLE ONLY public.sql_query_search_embeddings
    ADD CONSTRAINT sql_query_search_embeddings_pkey PRIMARY KEY (id);
EXCEPTION
    WHEN duplicate_object THEN NULL;
    WHEN duplicate_table THEN NULL;
    WHEN invalid_table_definition THEN NULL;
END $$;

--
-- Name: conversation_read_status uq_conversation_read_status; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $$ BEGIN
    ALTER TABLE ONLY public.conversation_read_status
    ADD CONSTRAINT uq_conversation_read_status UNIQUE (session_id, user_id);
EXCEPTION
    WHEN duplicate_object THEN NULL;
    WHEN duplicate_table THEN NULL;
    WHEN invalid_table_definition THEN NULL;
END $$;

--
-- Name: dashboard_versions uq_dashboard_version; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $$ BEGIN
    ALTER TABLE ONLY public.dashboard_versions
    ADD CONSTRAINT uq_dashboard_version UNIQUE (dashboard_id, version_number);
EXCEPTION
    WHEN duplicate_object THEN NULL;
    WHEN duplicate_table THEN NULL;
    WHEN invalid_table_definition THEN NULL;
END $$;

--
-- Name: datasource_configs uq_datasource_name_workspace; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $$ BEGIN
    ALTER TABLE ONLY public.datasource_configs
    ADD CONSTRAINT uq_datasource_name_workspace UNIQUE (workspace_id, name);
EXCEPTION
    WHEN duplicate_object THEN NULL;
    WHEN duplicate_table THEN NULL;
    WHEN invalid_table_definition THEN NULL;
END $$;

--
-- Name: datasource_configs uq_datasource_slug_workspace; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $$ BEGIN
    ALTER TABLE ONLY public.datasource_configs
    ADD CONSTRAINT uq_datasource_slug_workspace UNIQUE (workspace_id, slug);
EXCEPTION
    WHEN duplicate_object THEN NULL;
    WHEN duplicate_table THEN NULL;
    WHEN invalid_table_definition THEN NULL;
END $$;

--
-- Name: user_auth_methods uq_user_auth_type; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $$ BEGIN
    ALTER TABLE ONLY public.user_auth_methods
    ADD CONSTRAINT uq_user_auth_type UNIQUE (user_id, auth_type);
EXCEPTION
    WHEN duplicate_object THEN NULL;
    WHEN duplicate_table THEN NULL;
    WHEN invalid_table_definition THEN NULL;
END $$;

--
-- Name: user_datasource_credentials uq_user_datasource; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $$ BEGIN
    ALTER TABLE ONLY public.user_datasource_credentials
    ADD CONSTRAINT uq_user_datasource UNIQUE (user_id, datasource_config_id);
EXCEPTION
    WHEN duplicate_object THEN NULL;
    WHEN duplicate_table THEN NULL;
    WHEN invalid_table_definition THEN NULL;
END $$;

--
-- Name: user_datasource_preferences uq_user_datasource_preference; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $$ BEGIN
    ALTER TABLE ONLY public.user_datasource_preferences
    ADD CONSTRAINT uq_user_datasource_preference UNIQUE (user_id, datasource_config_id);
EXCEPTION
    WHEN duplicate_object THEN NULL;
    WHEN duplicate_table THEN NULL;
    WHEN invalid_table_definition THEN NULL;
END $$;

--
-- Name: workspace_usage uq_workspace_usage_date; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $$ BEGIN
    ALTER TABLE ONLY public.workspace_usage
    ADD CONSTRAINT uq_workspace_usage_date UNIQUE (workspace_id, usage_date);
EXCEPTION
    WHEN duplicate_object THEN NULL;
    WHEN duplicate_table THEN NULL;
    WHEN invalid_table_definition THEN NULL;
END $$;

--
-- Name: workspace_users uq_workspace_user; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $$ BEGIN
    ALTER TABLE ONLY public.workspace_users
    ADD CONSTRAINT uq_workspace_user UNIQUE (workspace_id, user_id);
EXCEPTION
    WHEN duplicate_object THEN NULL;
    WHEN duplicate_table THEN NULL;
    WHEN invalid_table_definition THEN NULL;
END $$;

--
-- Name: user_auth_methods user_auth_methods_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $$ BEGIN
    ALTER TABLE ONLY public.user_auth_methods
    ADD CONSTRAINT user_auth_methods_pkey PRIMARY KEY (id);
EXCEPTION
    WHEN duplicate_object THEN NULL;
    WHEN duplicate_table THEN NULL;
    WHEN invalid_table_definition THEN NULL;
END $$;

--
-- Name: user_datasource_credentials user_datasource_credentials_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $$ BEGIN
    ALTER TABLE ONLY public.user_datasource_credentials
    ADD CONSTRAINT user_datasource_credentials_pkey PRIMARY KEY (id);
EXCEPTION
    WHEN duplicate_object THEN NULL;
    WHEN duplicate_table THEN NULL;
    WHEN invalid_table_definition THEN NULL;
END $$;

--
-- Name: user_datasource_preferences user_datasource_preferences_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $$ BEGIN
    ALTER TABLE ONLY public.user_datasource_preferences
    ADD CONSTRAINT user_datasource_preferences_pkey PRIMARY KEY (id);
EXCEPTION
    WHEN duplicate_object THEN NULL;
    WHEN duplicate_table THEN NULL;
    WHEN invalid_table_definition THEN NULL;
END $$;

--
-- Name: users users_email_key; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $$ BEGIN
    ALTER TABLE ONLY public.users
    ADD CONSTRAINT users_email_key UNIQUE (email);
EXCEPTION
    WHEN duplicate_object THEN NULL;
    WHEN duplicate_table THEN NULL;
    WHEN invalid_table_definition THEN NULL;
END $$;

--
-- Name: users users_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $$ BEGIN
    ALTER TABLE ONLY public.users
    ADD CONSTRAINT users_pkey PRIMARY KEY (user_id);
EXCEPTION
    WHEN duplicate_object THEN NULL;
    WHEN duplicate_table THEN NULL;
    WHEN invalid_table_definition THEN NULL;
END $$;

--
-- Name: verification_tokens verification_tokens_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $$ BEGIN
    ALTER TABLE ONLY public.verification_tokens
    ADD CONSTRAINT verification_tokens_pkey PRIMARY KEY (token_id);
EXCEPTION
    WHEN duplicate_object THEN NULL;
    WHEN duplicate_table THEN NULL;
    WHEN invalid_table_definition THEN NULL;
END $$;

--
-- Name: watch_executions watch_executions_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $$ BEGIN
    ALTER TABLE ONLY public.watch_executions
    ADD CONSTRAINT watch_executions_pkey PRIMARY KEY (id);
EXCEPTION
    WHEN duplicate_object THEN NULL;
    WHEN duplicate_table THEN NULL;
    WHEN invalid_table_definition THEN NULL;
END $$;

--
-- Name: watches watches_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $$ BEGIN
    ALTER TABLE ONLY public.watches
    ADD CONSTRAINT watches_pkey PRIMARY KEY (watch_id);
EXCEPTION
    WHEN duplicate_object THEN NULL;
    WHEN duplicate_table THEN NULL;
    WHEN invalid_table_definition THEN NULL;
END $$;

--
-- Name: workspace_invitations workspace_invitations_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $$ BEGIN
    ALTER TABLE ONLY public.workspace_invitations
    ADD CONSTRAINT workspace_invitations_pkey PRIMARY KEY (invitation_id);
EXCEPTION
    WHEN duplicate_object THEN NULL;
    WHEN duplicate_table THEN NULL;
    WHEN invalid_table_definition THEN NULL;
END $$;

--
-- Name: workspace_knowledge_chunks workspace_knowledge_chunks_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $$ BEGIN
    ALTER TABLE ONLY public.workspace_knowledge_chunks
    ADD CONSTRAINT workspace_knowledge_chunks_pkey PRIMARY KEY (chunk_id);
EXCEPTION
    WHEN duplicate_object THEN NULL;
    WHEN duplicate_table THEN NULL;
    WHEN invalid_table_definition THEN NULL;
END $$;

--
-- Name: workspace_usage workspace_usage_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $$ BEGIN
    ALTER TABLE ONLY public.workspace_usage
    ADD CONSTRAINT workspace_usage_pkey PRIMARY KEY (id);
EXCEPTION
    WHEN duplicate_object THEN NULL;
    WHEN duplicate_table THEN NULL;
    WHEN invalid_table_definition THEN NULL;
END $$;

--
-- Name: workspace_users workspace_users_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $$ BEGIN
    ALTER TABLE ONLY public.workspace_users
    ADD CONSTRAINT workspace_users_pkey PRIMARY KEY (id);
EXCEPTION
    WHEN duplicate_object THEN NULL;
    WHEN duplicate_table THEN NULL;
    WHEN invalid_table_definition THEN NULL;
END $$;

--
-- Name: workspaces workspaces_domain_key; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $$ BEGIN
    ALTER TABLE ONLY public.workspaces
    ADD CONSTRAINT workspaces_domain_key UNIQUE (domain);
EXCEPTION
    WHEN duplicate_object THEN NULL;
    WHEN duplicate_table THEN NULL;
    WHEN invalid_table_definition THEN NULL;
END $$;

--
-- Name: workspaces workspaces_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $$ BEGIN
    ALTER TABLE ONLY public.workspaces
    ADD CONSTRAINT workspaces_pkey PRIMARY KEY (workspace_id);
EXCEPTION
    WHEN duplicate_object THEN NULL;
    WHEN duplicate_table THEN NULL;
    WHEN invalid_table_definition THEN NULL;
END $$;

--
-- Name: workspaces workspaces_slack_team_id_key; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $$ BEGIN
    ALTER TABLE ONLY public.workspaces
    ADD CONSTRAINT workspaces_slack_team_id_key UNIQUE (slack_team_id);
EXCEPTION
    WHEN duplicate_object THEN NULL;
    WHEN duplicate_table THEN NULL;
    WHEN invalid_table_definition THEN NULL;
END $$;

--
-- Name: idx_agent_learnings_superseded; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_agent_learnings_superseded ON public.agent_learnings USING btree (workspace_id, enabled, superseded_by) WHERE (superseded_by IS NULL);

--
-- Name: idx_api_tokens_active; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_api_tokens_active ON public.api_tokens USING btree (active);

--
-- Name: idx_api_tokens_expires; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_api_tokens_expires ON public.api_tokens USING btree (expires_at);

--
-- Name: idx_api_tokens_hash; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_api_tokens_hash ON public.api_tokens USING btree (token_hash);

--
-- Name: idx_api_tokens_user; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_api_tokens_user ON public.api_tokens USING btree (user_id);

--
-- Name: idx_api_usage_cache_tokens; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_api_usage_cache_tokens ON public.api_usage_log USING btree (cache_creation_input_tokens, cache_read_input_tokens) WHERE ((cache_creation_input_tokens > 0) OR (cache_read_input_tokens > 0));

--
-- Name: idx_api_usage_session; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_api_usage_session ON public.api_usage_log USING btree (session_id);

--
-- Name: idx_api_usage_timestamp; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_api_usage_timestamp ON public.api_usage_log USING btree ("timestamp");

--
-- Name: idx_api_usage_user; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_api_usage_user ON public.api_usage_log USING btree (user_id);

--
-- Name: idx_api_usage_workspace; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_api_usage_workspace ON public.api_usage_log USING btree (workspace_id);

--
-- Name: idx_auth_methods_type; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_auth_methods_type ON public.user_auth_methods USING btree (auth_type);

--
-- Name: idx_auth_methods_user; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_auth_methods_user ON public.user_auth_methods USING btree (user_id);

--
-- Name: idx_charts_created; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_charts_created ON public.charts USING btree (created_at);

--
-- Name: idx_charts_message; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_charts_message ON public.charts USING btree (message_id);

--
-- Name: idx_chat_messages_content_tsv; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_chat_messages_content_tsv ON public.chat_messages USING gin (content_tsv);

--
-- Name: idx_chat_messages_created; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_chat_messages_created ON public.chat_messages USING btree (created_at);

--
-- Name: idx_chat_messages_role; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_chat_messages_role ON public.chat_messages USING btree (role);

--
-- Name: idx_chat_messages_sent_by_user; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_chat_messages_sent_by_user ON public.chat_messages USING btree (sent_by_user_id);

--
-- Name: idx_chat_messages_session; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_chat_messages_session ON public.chat_messages USING btree (session_id);

--
-- Name: idx_chat_messages_tool_call_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_chat_messages_tool_call_id ON public.chat_messages USING btree (tool_call_id);

--
-- Name: idx_chat_sessions_created; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_chat_sessions_created ON public.chat_sessions USING btree (created_at);

--
-- Name: idx_chat_sessions_shared; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_chat_sessions_shared ON public.chat_sessions USING btree (workspace_id, shared) WHERE (shared = true);

--
-- Name: idx_chat_sessions_slack_thread; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX IF NOT EXISTS idx_chat_sessions_slack_thread ON public.chat_sessions USING btree (slack_channel_id, slack_thread_ts) WHERE (slack_channel_id IS NOT NULL);

--
-- Name: idx_chat_sessions_type; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_chat_sessions_type ON public.chat_sessions USING btree (session_type);

--
-- Name: idx_chat_sessions_user; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_chat_sessions_user ON public.chat_sessions USING btree (user_id);

--
-- Name: idx_chat_sessions_workspace; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_chat_sessions_workspace ON public.chat_sessions USING btree (workspace_id);

--
-- Name: idx_collection_dashboards_collection_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_collection_dashboards_collection_id ON public.collection_dashboards USING btree (collection_id);

--
-- Name: idx_collection_dashboards_dashboard_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_collection_dashboards_dashboard_id ON public.collection_dashboards USING btree (dashboard_id);

--
-- Name: idx_collection_dashboards_position; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_collection_dashboards_position ON public.collection_dashboards USING btree (collection_id, "position");

--
-- Name: idx_collections_is_public; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_collections_is_public ON public.collections USING btree (is_public);

--
-- Name: idx_collections_workspace_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_collections_workspace_id ON public.collections USING btree (workspace_id);

--
-- Name: idx_conversation_read_status_user; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_conversation_read_status_user ON public.conversation_read_status USING btree (user_id);

--
-- Name: idx_dashboard_versions_created; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_dashboard_versions_created ON public.dashboard_versions USING btree (created_at DESC);

--
-- Name: idx_dashboard_versions_dashboard; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_dashboard_versions_dashboard ON public.dashboard_versions USING btree (dashboard_id);

--
-- Name: idx_dashboard_versions_user; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_dashboard_versions_user ON public.dashboard_versions USING btree (created_by);

--
-- Name: idx_dashboard_views_dashboard; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_dashboard_views_dashboard ON public.dashboard_views USING btree (dashboard_id);

--
-- Name: idx_dashboard_views_viewed_at; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_dashboard_views_viewed_at ON public.dashboard_views USING btree (viewed_at);

--
-- Name: idx_dashboard_views_workspace; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_dashboard_views_workspace ON public.dashboard_views USING btree (workspace_id);

--
-- Name: idx_dashboards_created; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_dashboards_created ON public.dashboards USING btree (created_at);

--
-- Name: idx_dashboards_embedding; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_dashboards_embedding ON public.dashboards USING hnsw (embedding public.vector_cosine_ops) WITH (m='16', ef_construction='64');

--
-- Name: idx_dashboards_search_vector; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_dashboards_search_vector ON public.dashboards USING gin (search_vector);

--
-- Name: idx_dashboards_updated; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_dashboards_updated ON public.dashboards USING btree (updated_at);

--
-- Name: idx_dashboards_user; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_dashboards_user ON public.dashboards USING btree (user_id);

--
-- Name: idx_dashboards_workspace_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_dashboards_workspace_id ON public.dashboards USING btree (workspace_id);

--
-- Name: idx_datasource_cache_archived; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_datasource_cache_archived ON public.datasource_table_cache USING btree (is_archived);

--
-- Name: idx_datasource_cache_dataset; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_datasource_cache_dataset ON public.datasource_table_cache USING btree (dataset_id);

--
-- Name: idx_datasource_cache_descriptions_refresh; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_datasource_cache_descriptions_refresh ON public.datasource_table_cache USING btree (descriptions_refreshed_at);

--
-- Name: idx_datasource_cache_project; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_datasource_cache_project ON public.datasource_table_cache USING btree (project_id);

--
-- Name: idx_datasource_cache_structure_refresh; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_datasource_cache_structure_refresh ON public.datasource_table_cache USING btree (structure_refreshed_at);

--
-- Name: idx_datasource_cache_updated; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_datasource_cache_updated ON public.datasource_table_cache USING btree (updated_at);

--
-- Name: idx_datasource_cache_workspace; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_datasource_cache_workspace ON public.datasource_table_cache USING btree (workspace_id);

--
-- Name: idx_datasource_configs_active; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_datasource_configs_active ON public.datasource_configs USING btree (active);

--
-- Name: idx_datasource_configs_slug; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_datasource_configs_slug ON public.datasource_configs USING btree (workspace_id, slug);

--
-- Name: idx_datasource_configs_type; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_datasource_configs_type ON public.datasource_configs USING btree (datasource_type);

--
-- Name: idx_datasource_configs_workspace; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_datasource_configs_workspace ON public.datasource_configs USING btree (workspace_id);

--
-- Name: idx_email_subscribers_created_at; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_email_subscribers_created_at ON public.email_subscribers USING btree (created_at DESC);

--
-- Name: idx_email_subscribers_email; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_email_subscribers_email ON public.email_subscribers USING btree (email);

--
-- Name: idx_email_subscribers_marketing_consent; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_email_subscribers_marketing_consent ON public.email_subscribers USING btree (marketing_consent) WHERE (marketing_consent = true);

--
-- Name: idx_feedback_created; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_feedback_created ON public.feedback USING btree (created_at);

--
-- Name: idx_feedback_status; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_feedback_status ON public.feedback USING btree (status);

--
-- Name: idx_feedback_type; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_feedback_type ON public.feedback USING btree (type);

--
-- Name: idx_feedback_user; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_feedback_user ON public.feedback USING btree (user_id);

--
-- Name: idx_feedback_workspace; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_feedback_workspace ON public.feedback USING btree (workspace_id);

--
-- Name: idx_knowledge_chunks_embedding; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_knowledge_chunks_embedding ON public.workspace_knowledge_chunks USING ivfflat (embedding public.vector_cosine_ops);

--
-- Name: idx_knowledge_chunks_workspace; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_knowledge_chunks_workspace ON public.workspace_knowledge_chunks USING btree (workspace_id);

--
-- Name: idx_knowledge_chunks_workspace_index; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_knowledge_chunks_workspace_index ON public.workspace_knowledge_chunks USING btree (workspace_id, chunk_index);

--
-- Name: idx_learnings_datasource; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_learnings_datasource ON public.agent_learnings USING btree (datasource_config_id);

--
-- Name: idx_learnings_embedding; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_learnings_embedding ON public.agent_learnings USING hnsw (embedding public.vector_cosine_ops);

--
-- Name: idx_learnings_scope; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_learnings_scope ON public.agent_learnings USING btree (workspace_id, scope, enabled);

--
-- Name: idx_learnings_search_vector; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_learnings_search_vector ON public.agent_learnings USING gin (search_vector);

--
-- Name: idx_learnings_workspace; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_learnings_workspace ON public.agent_learnings USING btree (workspace_id);

--
-- Name: idx_learnings_workspace_enabled; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_learnings_workspace_enabled ON public.agent_learnings USING btree (workspace_id, enabled);

--
-- Name: idx_notifications_created; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_notifications_created ON public.notifications USING btree (created_at DESC);

--
-- Name: idx_notifications_source_watch; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_notifications_source_watch ON public.notifications USING btree (source_watch_id);

--
-- Name: idx_notifications_unread; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_notifications_unread ON public.notifications USING btree (user_id, read) WHERE (read = false);

--
-- Name: idx_notifications_user; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_notifications_user ON public.notifications USING btree (user_id);

--
-- Name: idx_notifications_workspace; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_notifications_workspace ON public.notifications USING btree (workspace_id);

--
-- Name: idx_oauth_clients_client_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_oauth_clients_client_id ON public.oauth_clients USING btree (client_id);

--
-- Name: idx_oauth_states_created_at; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_oauth_states_created_at ON public.oauth_states USING btree (created_at);

--
-- Name: idx_ownership_transfers_expires; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_ownership_transfers_expires ON public.ownership_transfers USING btree (expires_at);

--
-- Name: idx_ownership_transfers_status; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_ownership_transfers_status ON public.ownership_transfers USING btree (status);

--
-- Name: idx_ownership_transfers_to_user; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_ownership_transfers_to_user ON public.ownership_transfers USING btree (to_user_id);

--
-- Name: idx_ownership_transfers_workspace; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_ownership_transfers_workspace ON public.ownership_transfers USING btree (workspace_id);

--
-- Name: idx_query_cache_last_accessed; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_query_cache_last_accessed ON public.query_cache USING btree (last_accessed_at);

--
-- Name: idx_query_history_datasource; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_query_history_datasource ON public.sql_query_history USING btree (datasource_config_id, workspace_id);

--
-- Name: idx_query_history_executed_at; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_query_history_executed_at ON public.sql_query_history USING btree (executed_at);

--
-- Name: idx_query_history_saved; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_query_history_saved ON public.sql_query_history USING btree (is_saved, workspace_id, user_id);

--
-- Name: idx_query_history_text_search; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_query_history_text_search ON public.sql_query_history USING gin (query_text public.gin_trgm_ops);

--
-- Name: idx_query_history_workspace_user; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_query_history_workspace_user ON public.sql_query_history USING btree (workspace_id, user_id);

--
-- Name: idx_query_search_emb_vector_hnsw; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_query_search_emb_vector_hnsw ON public.sql_query_search_embeddings USING hnsw (embedding public.vector_cosine_ops) WITH (m='16', ef_construction='64');

--
-- Name: idx_query_search_emb_workspace_user; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_query_search_emb_workspace_user ON public.sql_query_search_embeddings USING btree (workspace_id, user_id);

--
-- Name: idx_refresh_tokens_active; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_refresh_tokens_active ON public.refresh_tokens USING btree (is_active);

--
-- Name: idx_refresh_tokens_expires; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_refresh_tokens_expires ON public.refresh_tokens USING btree (expires_at);

--
-- Name: idx_refresh_tokens_hash; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_refresh_tokens_hash ON public.refresh_tokens USING btree (token_hash);

--
-- Name: idx_refresh_tokens_user; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_refresh_tokens_user ON public.refresh_tokens USING btree (user_id);

--
-- Name: idx_search_emb_table; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_search_emb_table ON public.datasource_search_embeddings USING btree (project_id, dataset_id, table_id);

--
-- Name: idx_search_emb_type; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_search_emb_type ON public.datasource_search_embeddings USING btree (entry_type);

--
-- Name: idx_search_emb_vector_hnsw; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_search_emb_vector_hnsw ON public.datasource_search_embeddings USING hnsw (embedding public.vector_cosine_ops) WITH (m='16', ef_construction='64');

--
-- Name: idx_search_emb_workspace; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_search_emb_workspace ON public.datasource_search_embeddings USING btree (workspace_id);

--
-- Name: idx_search_embedding_datasource; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_search_embedding_datasource ON public.datasource_search_embeddings USING btree (datasource_config_id);

--
-- Name: idx_table_cache_datasource; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_table_cache_datasource ON public.datasource_table_cache USING btree (datasource_config_id);

--
-- Name: idx_user_datasource_creds_datasource; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_user_datasource_creds_datasource ON public.user_datasource_credentials USING btree (datasource_config_id);

--
-- Name: idx_user_datasource_creds_user; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_user_datasource_creds_user ON public.user_datasource_credentials USING btree (user_id);

--
-- Name: idx_user_datasource_creds_workspace; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_user_datasource_creds_workspace ON public.user_datasource_credentials USING btree (workspace_id);

--
-- Name: idx_user_datasource_pref_datasource; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_user_datasource_pref_datasource ON public.user_datasource_preferences USING btree (datasource_config_id);

--
-- Name: idx_user_datasource_pref_user; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_user_datasource_pref_user ON public.user_datasource_preferences USING btree (user_id);

--
-- Name: idx_users_active; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_users_active ON public.users USING btree (active);

--
-- Name: idx_users_created_at; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_users_created_at ON public.users USING btree (created_at);

--
-- Name: idx_users_email; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_users_email ON public.users USING btree (email);

--
-- Name: idx_users_last_workspace_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_users_last_workspace_id ON public.users USING btree (last_workspace_id);

--
-- Name: idx_users_terms_accepted_at; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_users_terms_accepted_at ON public.users USING btree (terms_accepted_at);

--
-- Name: idx_verification_tokens_email; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_verification_tokens_email ON public.verification_tokens USING btree (email);

--
-- Name: idx_verification_tokens_expires; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_verification_tokens_expires ON public.verification_tokens USING btree (expires_at);

--
-- Name: idx_verification_tokens_hash; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_verification_tokens_hash ON public.verification_tokens USING btree (token_hash);

--
-- Name: idx_verification_tokens_type; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_verification_tokens_type ON public.verification_tokens USING btree (token_type);

--
-- Name: idx_watch_executions_alert; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_watch_executions_alert ON public.watch_executions USING btree (alert_triggered) WHERE (alert_triggered = true);

--
-- Name: idx_watch_executions_completed; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_watch_executions_completed ON public.watch_executions USING btree (completed_at DESC);

--
-- Name: idx_watch_executions_deleted_at; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_watch_executions_deleted_at ON public.watch_executions USING btree (deleted_at);

--
-- Name: idx_watch_executions_dismissed; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_watch_executions_dismissed ON public.watch_executions USING btree (alert_triggered, deleted_at) WHERE ((alert_triggered = true) AND (deleted_at IS NULL));

--
-- Name: idx_watch_executions_notification; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_watch_executions_notification ON public.watch_executions USING btree (notification_id);

--
-- Name: idx_watch_executions_read_at; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_watch_executions_read_at ON public.watch_executions USING btree (read_at);

--
-- Name: idx_watch_executions_session; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_watch_executions_session ON public.watch_executions USING btree (session_id);

--
-- Name: idx_watch_executions_started; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_watch_executions_started ON public.watch_executions USING btree (started_at DESC);

--
-- Name: idx_watch_executions_status; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_watch_executions_status ON public.watch_executions USING btree (status);

--
-- Name: idx_watch_executions_watch; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_watch_executions_watch ON public.watch_executions USING btree (watch_id);

--
-- Name: idx_watch_executions_workspace; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_watch_executions_workspace ON public.watch_executions USING btree (workspace_id);

--
-- Name: idx_watches_creator; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_watches_creator ON public.watches USING btree (created_by);

--
-- Name: idx_watches_enabled; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_watches_enabled ON public.watches USING btree (enabled) WHERE (enabled = true);

--
-- Name: idx_watches_name_workspace_unique; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX IF NOT EXISTS idx_watches_name_workspace_unique ON public.watches USING btree (workspace_id, lower((name)::text));

--
-- Name: idx_watches_next_run; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_watches_next_run ON public.watches USING btree (next_run_at);

--
-- Name: idx_watches_workspace; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_watches_workspace ON public.watches USING btree (workspace_id);

--
-- Name: idx_workspace_invitations_email; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_workspace_invitations_email ON public.workspace_invitations USING btree (email);

--
-- Name: idx_workspace_invitations_status; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_workspace_invitations_status ON public.workspace_invitations USING btree (status);

--
-- Name: idx_workspace_invitations_workspace; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_workspace_invitations_workspace ON public.workspace_invitations USING btree (workspace_id);

--
-- Name: idx_workspace_usage_date; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_workspace_usage_date ON public.workspace_usage USING btree (usage_date);

--
-- Name: idx_workspace_usage_workspace; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_workspace_usage_workspace ON public.workspace_usage USING btree (workspace_id);

--
-- Name: idx_workspace_users_role; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_workspace_users_role ON public.workspace_users USING btree (role);

--
-- Name: idx_workspace_users_slack; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_workspace_users_slack ON public.workspace_users USING btree (slack_user_id) WHERE (slack_user_id IS NOT NULL);

--
-- Name: idx_workspace_users_user; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_workspace_users_user ON public.workspace_users USING btree (user_id);

--
-- Name: idx_workspace_users_workspace; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_workspace_users_workspace ON public.workspace_users USING btree (workspace_id);

--
-- Name: idx_workspaces_catalog_onboarding; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_workspaces_catalog_onboarding ON public.workspaces USING btree (catalog_onboarding_completed);

--
-- Name: idx_workspaces_catalog_status; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_workspaces_catalog_status ON public.workspaces USING btree (catalog_refresh_status);

--
-- Name: idx_workspaces_created_at; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_workspaces_created_at ON public.workspaces USING btree (created_at);

--
-- Name: idx_workspaces_domain; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_workspaces_domain ON public.workspaces USING btree (domain);

--
-- Name: idx_workspaces_owner_user_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_workspaces_owner_user_id ON public.workspaces USING btree (owner_user_id);

--
-- Name: idx_workspaces_slack_team; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_workspaces_slack_team ON public.workspaces USING btree (slack_team_id) WHERE (slack_team_id IS NOT NULL);

--
-- Name: idx_workspaces_status; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_workspaces_status ON public.workspaces USING btree (status);

--
-- Name: idx_workspaces_stripe_customer; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_workspaces_stripe_customer ON public.workspaces USING btree (stripe_customer_id);

--
-- Name: idx_workspaces_subscription_status; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_workspaces_subscription_status ON public.workspaces USING btree (subscription_status);

--
-- Name: idx_workspaces_subscription_tier; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_workspaces_subscription_tier ON public.workspaces USING btree (subscription_tier);

--
-- Name: unique_pending_transfer; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX IF NOT EXISTS unique_pending_transfer ON public.ownership_transfers USING btree (workspace_id) WHERE ((status)::text = 'pending'::text);

--
-- Name: agent_learnings agent_learnings_search_vector_trigger; Type: TRIGGER; Schema: public; Owner: -
--

DROP TRIGGER IF EXISTS agent_learnings_search_vector_trigger ON public.agent_learnings;
CREATE TRIGGER agent_learnings_search_vector_trigger BEFORE INSERT OR UPDATE ON public.agent_learnings FOR EACH ROW EXECUTE FUNCTION public.agent_learnings_search_vector_update();

--
-- Name: collections collections_updated_at_trigger; Type: TRIGGER; Schema: public; Owner: -
--

DROP TRIGGER IF EXISTS collections_updated_at_trigger ON public.collections;
CREATE TRIGGER collections_updated_at_trigger BEFORE UPDATE ON public.collections FOR EACH ROW EXECUTE FUNCTION public.update_collections_updated_at();

--
-- Name: dashboards dashboards_search_vector_trigger; Type: TRIGGER; Schema: public; Owner: -
--

DROP TRIGGER IF EXISTS dashboards_search_vector_trigger ON public.dashboards;
CREATE TRIGGER dashboards_search_vector_trigger BEFORE INSERT OR UPDATE ON public.dashboards FOR EACH ROW EXECUTE FUNCTION public.dashboards_search_vector_update();

--
-- Name: chat_messages tsvectorupdate_chat_messages; Type: TRIGGER; Schema: public; Owner: -
--

DROP TRIGGER IF EXISTS tsvectorupdate_chat_messages ON public.chat_messages;
CREATE TRIGGER tsvectorupdate_chat_messages BEFORE INSERT OR UPDATE ON public.chat_messages FOR EACH ROW EXECUTE FUNCTION public.chat_messages_content_tsv_trigger();

--
-- Name: agent_learnings agent_learnings_superseded_by_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $$ BEGIN
    ALTER TABLE ONLY public.agent_learnings
    ADD CONSTRAINT agent_learnings_superseded_by_fkey FOREIGN KEY (superseded_by) REFERENCES public.agent_learnings(learning_id) ON DELETE SET NULL;
EXCEPTION
    WHEN duplicate_object THEN NULL;
    WHEN duplicate_table THEN NULL;
    WHEN invalid_table_definition THEN NULL;
END $$;

--
-- Name: agent_learnings agent_learnings_workspace_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $$ BEGIN
    ALTER TABLE ONLY public.agent_learnings
    ADD CONSTRAINT agent_learnings_workspace_id_fkey FOREIGN KEY (workspace_id) REFERENCES public.workspaces(workspace_id) ON DELETE CASCADE;
EXCEPTION
    WHEN duplicate_object THEN NULL;
    WHEN duplicate_table THEN NULL;
    WHEN invalid_table_definition THEN NULL;
END $$;

--
-- Name: api_tokens api_tokens_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $$ BEGIN
    ALTER TABLE ONLY public.api_tokens
    ADD CONSTRAINT api_tokens_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(user_id);
EXCEPTION
    WHEN duplicate_object THEN NULL;
    WHEN duplicate_table THEN NULL;
    WHEN invalid_table_definition THEN NULL;
END $$;

--
-- Name: api_usage_log api_usage_log_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $$ BEGIN
    ALTER TABLE ONLY public.api_usage_log
    ADD CONSTRAINT api_usage_log_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(user_id);
EXCEPTION
    WHEN duplicate_object THEN NULL;
    WHEN duplicate_table THEN NULL;
    WHEN invalid_table_definition THEN NULL;
END $$;

--
-- Name: api_usage_log api_usage_log_workspace_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $$ BEGIN
    ALTER TABLE ONLY public.api_usage_log
    ADD CONSTRAINT api_usage_log_workspace_id_fkey FOREIGN KEY (workspace_id) REFERENCES public.workspaces(workspace_id);
EXCEPTION
    WHEN duplicate_object THEN NULL;
    WHEN duplicate_table THEN NULL;
    WHEN invalid_table_definition THEN NULL;
END $$;

--
-- Name: charts charts_message_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $$ BEGIN
    ALTER TABLE ONLY public.charts
    ADD CONSTRAINT charts_message_id_fkey FOREIGN KEY (message_id) REFERENCES public.chat_messages(message_id) ON DELETE CASCADE;
EXCEPTION
    WHEN duplicate_object THEN NULL;
    WHEN duplicate_table THEN NULL;
    WHEN invalid_table_definition THEN NULL;
END $$;

--
-- Name: chat_messages chat_messages_sent_by_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $$ BEGIN
    ALTER TABLE ONLY public.chat_messages
    ADD CONSTRAINT chat_messages_sent_by_user_id_fkey FOREIGN KEY (sent_by_user_id) REFERENCES public.users(user_id);
EXCEPTION
    WHEN duplicate_object THEN NULL;
    WHEN duplicate_table THEN NULL;
    WHEN invalid_table_definition THEN NULL;
END $$;

--
-- Name: chat_messages chat_messages_session_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $$ BEGIN
    ALTER TABLE ONLY public.chat_messages
    ADD CONSTRAINT chat_messages_session_id_fkey FOREIGN KEY (session_id) REFERENCES public.chat_sessions(session_id);
EXCEPTION
    WHEN duplicate_object THEN NULL;
    WHEN duplicate_table THEN NULL;
    WHEN invalid_table_definition THEN NULL;
END $$;

--
-- Name: chat_sessions chat_sessions_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $$ BEGIN
    ALTER TABLE ONLY public.chat_sessions
    ADD CONSTRAINT chat_sessions_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(user_id);
EXCEPTION
    WHEN duplicate_object THEN NULL;
    WHEN duplicate_table THEN NULL;
    WHEN invalid_table_definition THEN NULL;
END $$;

--
-- Name: chat_sessions chat_sessions_workspace_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $$ BEGIN
    ALTER TABLE ONLY public.chat_sessions
    ADD CONSTRAINT chat_sessions_workspace_id_fkey FOREIGN KEY (workspace_id) REFERENCES public.workspaces(workspace_id);
EXCEPTION
    WHEN duplicate_object THEN NULL;
    WHEN duplicate_table THEN NULL;
    WHEN invalid_table_definition THEN NULL;
END $$;

--
-- Name: collection_dashboards collection_dashboards_collection_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $$ BEGIN
    ALTER TABLE ONLY public.collection_dashboards
    ADD CONSTRAINT collection_dashboards_collection_id_fkey FOREIGN KEY (collection_id) REFERENCES public.collections(id) ON DELETE CASCADE;
EXCEPTION
    WHEN duplicate_object THEN NULL;
    WHEN duplicate_table THEN NULL;
    WHEN invalid_table_definition THEN NULL;
END $$;

--
-- Name: collection_dashboards collection_dashboards_dashboard_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $$ BEGIN
    ALTER TABLE ONLY public.collection_dashboards
    ADD CONSTRAINT collection_dashboards_dashboard_id_fkey FOREIGN KEY (dashboard_id) REFERENCES public.dashboards(dashboard_id) ON DELETE CASCADE;
EXCEPTION
    WHEN duplicate_object THEN NULL;
    WHEN duplicate_table THEN NULL;
    WHEN invalid_table_definition THEN NULL;
END $$;

--
-- Name: collections collections_workspace_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $$ BEGIN
    ALTER TABLE ONLY public.collections
    ADD CONSTRAINT collections_workspace_id_fkey FOREIGN KEY (workspace_id) REFERENCES public.workspaces(workspace_id);
EXCEPTION
    WHEN duplicate_object THEN NULL;
    WHEN duplicate_table THEN NULL;
    WHEN invalid_table_definition THEN NULL;
END $$;

--
-- Name: conversation_read_status conversation_read_status_session_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $$ BEGIN
    ALTER TABLE ONLY public.conversation_read_status
    ADD CONSTRAINT conversation_read_status_session_id_fkey FOREIGN KEY (session_id) REFERENCES public.chat_sessions(session_id) ON DELETE CASCADE;
EXCEPTION
    WHEN duplicate_object THEN NULL;
    WHEN duplicate_table THEN NULL;
    WHEN invalid_table_definition THEN NULL;
END $$;

--
-- Name: conversation_read_status conversation_read_status_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $$ BEGIN
    ALTER TABLE ONLY public.conversation_read_status
    ADD CONSTRAINT conversation_read_status_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(user_id) ON DELETE CASCADE;
EXCEPTION
    WHEN duplicate_object THEN NULL;
    WHEN duplicate_table THEN NULL;
    WHEN invalid_table_definition THEN NULL;
END $$;

--
-- Name: dashboard_versions dashboard_versions_created_by_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $$ BEGIN
    ALTER TABLE ONLY public.dashboard_versions
    ADD CONSTRAINT dashboard_versions_created_by_fkey FOREIGN KEY (created_by) REFERENCES public.users(user_id);
EXCEPTION
    WHEN duplicate_object THEN NULL;
    WHEN duplicate_table THEN NULL;
    WHEN invalid_table_definition THEN NULL;
END $$;

--
-- Name: dashboard_versions dashboard_versions_dashboard_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $$ BEGIN
    ALTER TABLE ONLY public.dashboard_versions
    ADD CONSTRAINT dashboard_versions_dashboard_id_fkey FOREIGN KEY (dashboard_id) REFERENCES public.dashboards(dashboard_id) ON DELETE CASCADE;
EXCEPTION
    WHEN duplicate_object THEN NULL;
    WHEN duplicate_table THEN NULL;
    WHEN invalid_table_definition THEN NULL;
END $$;

--
-- Name: dashboard_views dashboard_views_dashboard_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $$ BEGIN
    ALTER TABLE ONLY public.dashboard_views
    ADD CONSTRAINT dashboard_views_dashboard_id_fkey FOREIGN KEY (dashboard_id) REFERENCES public.dashboards(dashboard_id) ON DELETE CASCADE;
EXCEPTION
    WHEN duplicate_object THEN NULL;
    WHEN duplicate_table THEN NULL;
    WHEN invalid_table_definition THEN NULL;
END $$;

--
-- Name: dashboard_views dashboard_views_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $$ BEGIN
    ALTER TABLE ONLY public.dashboard_views
    ADD CONSTRAINT dashboard_views_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(user_id);
EXCEPTION
    WHEN duplicate_object THEN NULL;
    WHEN duplicate_table THEN NULL;
    WHEN invalid_table_definition THEN NULL;
END $$;

--
-- Name: dashboard_views dashboard_views_workspace_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $$ BEGIN
    ALTER TABLE ONLY public.dashboard_views
    ADD CONSTRAINT dashboard_views_workspace_id_fkey FOREIGN KEY (workspace_id) REFERENCES public.workspaces(workspace_id);
EXCEPTION
    WHEN duplicate_object THEN NULL;
    WHEN duplicate_table THEN NULL;
    WHEN invalid_table_definition THEN NULL;
END $$;

--
-- Name: dashboards dashboards_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $$ BEGIN
    ALTER TABLE ONLY public.dashboards
    ADD CONSTRAINT dashboards_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(user_id);
EXCEPTION
    WHEN duplicate_object THEN NULL;
    WHEN duplicate_table THEN NULL;
    WHEN invalid_table_definition THEN NULL;
END $$;

--
-- Name: dashboards dashboards_workspace_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $$ BEGIN
    ALTER TABLE ONLY public.dashboards
    ADD CONSTRAINT dashboards_workspace_id_fkey FOREIGN KEY (workspace_id) REFERENCES public.workspaces(workspace_id);
EXCEPTION
    WHEN duplicate_object THEN NULL;
    WHEN duplicate_table THEN NULL;
    WHEN invalid_table_definition THEN NULL;
END $$;

--
-- Name: datasource_configs datasource_configs_workspace_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $$ BEGIN
    ALTER TABLE ONLY public.datasource_configs
    ADD CONSTRAINT datasource_configs_workspace_id_fkey FOREIGN KEY (workspace_id) REFERENCES public.workspaces(workspace_id) ON DELETE CASCADE;
EXCEPTION
    WHEN duplicate_object THEN NULL;
    WHEN duplicate_table THEN NULL;
    WHEN invalid_table_definition THEN NULL;
END $$;

--
-- Name: datasource_search_embeddings datasource_search_embeddings_table_cache_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $$ BEGIN
    ALTER TABLE ONLY public.datasource_search_embeddings
    ADD CONSTRAINT datasource_search_embeddings_table_cache_id_fkey FOREIGN KEY (table_cache_id) REFERENCES public.datasource_table_cache(id) ON DELETE CASCADE;
EXCEPTION
    WHEN duplicate_object THEN NULL;
    WHEN duplicate_table THEN NULL;
    WHEN invalid_table_definition THEN NULL;
END $$;

--
-- Name: feedback feedback_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $$ BEGIN
    ALTER TABLE ONLY public.feedback
    ADD CONSTRAINT feedback_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(user_id);
EXCEPTION
    WHEN duplicate_object THEN NULL;
    WHEN duplicate_table THEN NULL;
    WHEN invalid_table_definition THEN NULL;
END $$;

--
-- Name: feedback feedback_workspace_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $$ BEGIN
    ALTER TABLE ONLY public.feedback
    ADD CONSTRAINT feedback_workspace_id_fkey FOREIGN KEY (workspace_id) REFERENCES public.workspaces(workspace_id);
EXCEPTION
    WHEN duplicate_object THEN NULL;
    WHEN duplicate_table THEN NULL;
    WHEN invalid_table_definition THEN NULL;
END $$;

--
-- Name: agent_learnings fk_learnings_datasource; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $$ BEGIN
    ALTER TABLE ONLY public.agent_learnings
    ADD CONSTRAINT fk_learnings_datasource FOREIGN KEY (datasource_config_id) REFERENCES public.datasource_configs(id) ON DELETE SET NULL;
EXCEPTION
    WHEN duplicate_object THEN NULL;
    WHEN duplicate_table THEN NULL;
    WHEN invalid_table_definition THEN NULL;
END $$;

--
-- Name: notifications fk_notifications_source_watch; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $$ BEGIN
    ALTER TABLE ONLY public.notifications
    ADD CONSTRAINT fk_notifications_source_watch FOREIGN KEY (source_watch_id) REFERENCES public.watches(watch_id) ON DELETE SET NULL;
EXCEPTION
    WHEN duplicate_object THEN NULL;
    WHEN duplicate_table THEN NULL;
    WHEN invalid_table_definition THEN NULL;
END $$;

--
-- Name: datasource_search_embeddings fk_search_embedding_datasource; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $$ BEGIN
    ALTER TABLE ONLY public.datasource_search_embeddings
    ADD CONSTRAINT fk_search_embedding_datasource FOREIGN KEY (datasource_config_id) REFERENCES public.datasource_configs(id) ON DELETE CASCADE;
EXCEPTION
    WHEN duplicate_object THEN NULL;
    WHEN duplicate_table THEN NULL;
    WHEN invalid_table_definition THEN NULL;
END $$;

--
-- Name: workspaces fk_slack_installed_by_user; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $$ BEGIN
    ALTER TABLE ONLY public.workspaces
    ADD CONSTRAINT fk_slack_installed_by_user FOREIGN KEY (slack_installed_by_user_id) REFERENCES public.users(user_id);
EXCEPTION
    WHEN duplicate_object THEN NULL;
    WHEN duplicate_table THEN NULL;
    WHEN invalid_table_definition THEN NULL;
END $$;

--
-- Name: datasource_table_cache fk_table_cache_datasource; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $$ BEGIN
    ALTER TABLE ONLY public.datasource_table_cache
    ADD CONSTRAINT fk_table_cache_datasource FOREIGN KEY (datasource_config_id) REFERENCES public.datasource_configs(id) ON DELETE CASCADE;
EXCEPTION
    WHEN duplicate_object THEN NULL;
    WHEN duplicate_table THEN NULL;
    WHEN invalid_table_definition THEN NULL;
END $$;

--
-- Name: notifications notifications_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $$ BEGIN
    ALTER TABLE ONLY public.notifications
    ADD CONSTRAINT notifications_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(user_id) ON DELETE CASCADE;
EXCEPTION
    WHEN duplicate_object THEN NULL;
    WHEN duplicate_table THEN NULL;
    WHEN invalid_table_definition THEN NULL;
END $$;

--
-- Name: notifications notifications_workspace_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $$ BEGIN
    ALTER TABLE ONLY public.notifications
    ADD CONSTRAINT notifications_workspace_id_fkey FOREIGN KEY (workspace_id) REFERENCES public.workspaces(workspace_id) ON DELETE CASCADE;
EXCEPTION
    WHEN duplicate_object THEN NULL;
    WHEN duplicate_table THEN NULL;
    WHEN invalid_table_definition THEN NULL;
END $$;

--
-- Name: ownership_transfers ownership_transfers_from_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $$ BEGIN
    ALTER TABLE ONLY public.ownership_transfers
    ADD CONSTRAINT ownership_transfers_from_user_id_fkey FOREIGN KEY (from_user_id) REFERENCES public.users(user_id);
EXCEPTION
    WHEN duplicate_object THEN NULL;
    WHEN duplicate_table THEN NULL;
    WHEN invalid_table_definition THEN NULL;
END $$;

--
-- Name: ownership_transfers ownership_transfers_to_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $$ BEGIN
    ALTER TABLE ONLY public.ownership_transfers
    ADD CONSTRAINT ownership_transfers_to_user_id_fkey FOREIGN KEY (to_user_id) REFERENCES public.users(user_id);
EXCEPTION
    WHEN duplicate_object THEN NULL;
    WHEN duplicate_table THEN NULL;
    WHEN invalid_table_definition THEN NULL;
END $$;

--
-- Name: ownership_transfers ownership_transfers_workspace_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $$ BEGIN
    ALTER TABLE ONLY public.ownership_transfers
    ADD CONSTRAINT ownership_transfers_workspace_id_fkey FOREIGN KEY (workspace_id) REFERENCES public.workspaces(workspace_id);
EXCEPTION
    WHEN duplicate_object THEN NULL;
    WHEN duplicate_table THEN NULL;
    WHEN invalid_table_definition THEN NULL;
END $$;

--
-- Name: refresh_tokens refresh_tokens_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $$ BEGIN
    ALTER TABLE ONLY public.refresh_tokens
    ADD CONSTRAINT refresh_tokens_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(user_id);
EXCEPTION
    WHEN duplicate_object THEN NULL;
    WHEN duplicate_table THEN NULL;
    WHEN invalid_table_definition THEN NULL;
END $$;

--
-- Name: sql_query_history sql_query_history_datasource_config_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $$ BEGIN
    ALTER TABLE ONLY public.sql_query_history
    ADD CONSTRAINT sql_query_history_datasource_config_id_fkey FOREIGN KEY (datasource_config_id) REFERENCES public.datasource_configs(id) ON DELETE SET NULL;
EXCEPTION
    WHEN duplicate_object THEN NULL;
    WHEN duplicate_table THEN NULL;
    WHEN invalid_table_definition THEN NULL;
END $$;

--
-- Name: sql_query_history sql_query_history_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $$ BEGIN
    ALTER TABLE ONLY public.sql_query_history
    ADD CONSTRAINT sql_query_history_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(user_id);
EXCEPTION
    WHEN duplicate_object THEN NULL;
    WHEN duplicate_table THEN NULL;
    WHEN invalid_table_definition THEN NULL;
END $$;

--
-- Name: sql_query_history sql_query_history_workspace_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $$ BEGIN
    ALTER TABLE ONLY public.sql_query_history
    ADD CONSTRAINT sql_query_history_workspace_id_fkey FOREIGN KEY (workspace_id) REFERENCES public.workspaces(workspace_id);
EXCEPTION
    WHEN duplicate_object THEN NULL;
    WHEN duplicate_table THEN NULL;
    WHEN invalid_table_definition THEN NULL;
END $$;

--
-- Name: sql_query_search_embeddings sql_query_search_embeddings_query_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $$ BEGIN
    ALTER TABLE ONLY public.sql_query_search_embeddings
    ADD CONSTRAINT sql_query_search_embeddings_query_id_fkey FOREIGN KEY (query_id) REFERENCES public.sql_query_history(query_id) ON DELETE CASCADE;
EXCEPTION
    WHEN duplicate_object THEN NULL;
    WHEN duplicate_table THEN NULL;
    WHEN invalid_table_definition THEN NULL;
END $$;

--
-- Name: user_auth_methods user_auth_methods_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $$ BEGIN
    ALTER TABLE ONLY public.user_auth_methods
    ADD CONSTRAINT user_auth_methods_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(user_id);
EXCEPTION
    WHEN duplicate_object THEN NULL;
    WHEN duplicate_table THEN NULL;
    WHEN invalid_table_definition THEN NULL;
END $$;

--
-- Name: user_datasource_credentials user_datasource_credentials_datasource_config_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $$ BEGIN
    ALTER TABLE ONLY public.user_datasource_credentials
    ADD CONSTRAINT user_datasource_credentials_datasource_config_id_fkey FOREIGN KEY (datasource_config_id) REFERENCES public.datasource_configs(id) ON DELETE CASCADE;
EXCEPTION
    WHEN duplicate_object THEN NULL;
    WHEN duplicate_table THEN NULL;
    WHEN invalid_table_definition THEN NULL;
END $$;

--
-- Name: user_datasource_credentials user_datasource_credentials_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $$ BEGIN
    ALTER TABLE ONLY public.user_datasource_credentials
    ADD CONSTRAINT user_datasource_credentials_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(user_id) ON DELETE CASCADE;
EXCEPTION
    WHEN duplicate_object THEN NULL;
    WHEN duplicate_table THEN NULL;
    WHEN invalid_table_definition THEN NULL;
END $$;

--
-- Name: user_datasource_credentials user_datasource_credentials_workspace_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $$ BEGIN
    ALTER TABLE ONLY public.user_datasource_credentials
    ADD CONSTRAINT user_datasource_credentials_workspace_id_fkey FOREIGN KEY (workspace_id) REFERENCES public.workspaces(workspace_id) ON DELETE CASCADE;
EXCEPTION
    WHEN duplicate_object THEN NULL;
    WHEN duplicate_table THEN NULL;
    WHEN invalid_table_definition THEN NULL;
END $$;

--
-- Name: user_datasource_preferences user_datasource_preferences_datasource_config_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $$ BEGIN
    ALTER TABLE ONLY public.user_datasource_preferences
    ADD CONSTRAINT user_datasource_preferences_datasource_config_id_fkey FOREIGN KEY (datasource_config_id) REFERENCES public.datasource_configs(id) ON DELETE CASCADE;
EXCEPTION
    WHEN duplicate_object THEN NULL;
    WHEN duplicate_table THEN NULL;
    WHEN invalid_table_definition THEN NULL;
END $$;

--
-- Name: user_datasource_preferences user_datasource_preferences_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $$ BEGIN
    ALTER TABLE ONLY public.user_datasource_preferences
    ADD CONSTRAINT user_datasource_preferences_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(user_id) ON DELETE CASCADE;
EXCEPTION
    WHEN duplicate_object THEN NULL;
    WHEN duplicate_table THEN NULL;
    WHEN invalid_table_definition THEN NULL;
END $$;

--
-- Name: watch_executions watch_executions_notification_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $$ BEGIN
    ALTER TABLE ONLY public.watch_executions
    ADD CONSTRAINT watch_executions_notification_id_fkey FOREIGN KEY (notification_id) REFERENCES public.notifications(id) ON DELETE SET NULL;
EXCEPTION
    WHEN duplicate_object THEN NULL;
    WHEN duplicate_table THEN NULL;
    WHEN invalid_table_definition THEN NULL;
END $$;

--
-- Name: watch_executions watch_executions_session_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $$ BEGIN
    ALTER TABLE ONLY public.watch_executions
    ADD CONSTRAINT watch_executions_session_id_fkey FOREIGN KEY (session_id) REFERENCES public.chat_sessions(session_id) ON DELETE SET NULL;
EXCEPTION
    WHEN duplicate_object THEN NULL;
    WHEN duplicate_table THEN NULL;
    WHEN invalid_table_definition THEN NULL;
END $$;

--
-- Name: watch_executions watch_executions_watch_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $$ BEGIN
    ALTER TABLE ONLY public.watch_executions
    ADD CONSTRAINT watch_executions_watch_id_fkey FOREIGN KEY (watch_id) REFERENCES public.watches(watch_id) ON DELETE SET NULL;
EXCEPTION
    WHEN duplicate_object THEN NULL;
    WHEN duplicate_table THEN NULL;
    WHEN invalid_table_definition THEN NULL;
END $$;

--
-- Name: watches watches_created_by_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $$ BEGIN
    ALTER TABLE ONLY public.watches
    ADD CONSTRAINT watches_created_by_fkey FOREIGN KEY (created_by) REFERENCES public.users(user_id);
EXCEPTION
    WHEN duplicate_object THEN NULL;
    WHEN duplicate_table THEN NULL;
    WHEN invalid_table_definition THEN NULL;
END $$;

--
-- Name: watches watches_workspace_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $$ BEGIN
    ALTER TABLE ONLY public.watches
    ADD CONSTRAINT watches_workspace_id_fkey FOREIGN KEY (workspace_id) REFERENCES public.workspaces(workspace_id) ON DELETE CASCADE;
EXCEPTION
    WHEN duplicate_object THEN NULL;
    WHEN duplicate_table THEN NULL;
    WHEN invalid_table_definition THEN NULL;
END $$;

--
-- Name: workspace_invitations workspace_invitations_accepted_by_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $$ BEGIN
    ALTER TABLE ONLY public.workspace_invitations
    ADD CONSTRAINT workspace_invitations_accepted_by_user_id_fkey FOREIGN KEY (accepted_by_user_id) REFERENCES public.users(user_id);
EXCEPTION
    WHEN duplicate_object THEN NULL;
    WHEN duplicate_table THEN NULL;
    WHEN invalid_table_definition THEN NULL;
END $$;

--
-- Name: workspace_invitations workspace_invitations_invited_by_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $$ BEGIN
    ALTER TABLE ONLY public.workspace_invitations
    ADD CONSTRAINT workspace_invitations_invited_by_user_id_fkey FOREIGN KEY (invited_by_user_id) REFERENCES public.users(user_id);
EXCEPTION
    WHEN duplicate_object THEN NULL;
    WHEN duplicate_table THEN NULL;
    WHEN invalid_table_definition THEN NULL;
END $$;

--
-- Name: workspace_invitations workspace_invitations_workspace_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $$ BEGIN
    ALTER TABLE ONLY public.workspace_invitations
    ADD CONSTRAINT workspace_invitations_workspace_id_fkey FOREIGN KEY (workspace_id) REFERENCES public.workspaces(workspace_id);
EXCEPTION
    WHEN duplicate_object THEN NULL;
    WHEN duplicate_table THEN NULL;
    WHEN invalid_table_definition THEN NULL;
END $$;

--
-- Name: workspace_knowledge_chunks workspace_knowledge_chunks_workspace_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $$ BEGIN
    ALTER TABLE ONLY public.workspace_knowledge_chunks
    ADD CONSTRAINT workspace_knowledge_chunks_workspace_id_fkey FOREIGN KEY (workspace_id) REFERENCES public.workspaces(workspace_id) ON DELETE CASCADE;
EXCEPTION
    WHEN duplicate_object THEN NULL;
    WHEN duplicate_table THEN NULL;
    WHEN invalid_table_definition THEN NULL;
END $$;

--
-- Name: workspace_usage workspace_usage_workspace_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $$ BEGIN
    ALTER TABLE ONLY public.workspace_usage
    ADD CONSTRAINT workspace_usage_workspace_id_fkey FOREIGN KEY (workspace_id) REFERENCES public.workspaces(workspace_id);
EXCEPTION
    WHEN duplicate_object THEN NULL;
    WHEN duplicate_table THEN NULL;
    WHEN invalid_table_definition THEN NULL;
END $$;

--
-- Name: workspace_users workspace_users_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $$ BEGIN
    ALTER TABLE ONLY public.workspace_users
    ADD CONSTRAINT workspace_users_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(user_id);
EXCEPTION
    WHEN duplicate_object THEN NULL;
    WHEN duplicate_table THEN NULL;
    WHEN invalid_table_definition THEN NULL;
END $$;

--
-- Name: workspace_users workspace_users_workspace_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $$ BEGIN
    ALTER TABLE ONLY public.workspace_users
    ADD CONSTRAINT workspace_users_workspace_id_fkey FOREIGN KEY (workspace_id) REFERENCES public.workspaces(workspace_id);
EXCEPTION
    WHEN duplicate_object THEN NULL;
    WHEN duplicate_table THEN NULL;
    WHEN invalid_table_definition THEN NULL;
END $$;

--
-- Name: workspaces workspaces_owner_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $$ BEGIN
    ALTER TABLE ONLY public.workspaces
    ADD CONSTRAINT workspaces_owner_user_id_fkey FOREIGN KEY (owner_user_id) REFERENCES public.users(user_id);
EXCEPTION
    WHEN duplicate_object THEN NULL;
    WHEN duplicate_table THEN NULL;
    WHEN invalid_table_definition THEN NULL;
END $$;

--
-- PostgreSQL database dump complete
--
