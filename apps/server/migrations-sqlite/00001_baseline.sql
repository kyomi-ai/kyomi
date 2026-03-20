-- Kyomi baseline migration (SQLite)
-- Converted from Postgres baseline migration (20260215000000_baseline.sql)
-- All Postgres-specific types converted to SQLite equivalents
-- UUID generation handled in Rust application layer

PRAGMA foreign_keys = ON;

-- ============================================================================
-- TABLES (ordered by foreign key dependencies)
-- ============================================================================

-- Users must be created first (many tables reference it)
CREATE TABLE IF NOT EXISTS users (
    user_id TEXT NOT NULL PRIMARY KEY,
    email TEXT NOT NULL UNIQUE,
    name TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now')),
    last_login TEXT,
    active INTEGER NOT NULL DEFAULT 1,
    verified INTEGER NOT NULL DEFAULT 0,
    terms_accepted_at TEXT,
    terms_accepted_version TEXT,
    marketing_consent INTEGER NOT NULL DEFAULT 0,
    oauth_data TEXT,
    extra_metadata TEXT,
    chartml_config TEXT,
    last_workspace_id TEXT,
    knowledge TEXT,
    billing_project TEXT,
    default_project TEXT,
    query_size_limit_gb INTEGER NOT NULL DEFAULT 50
);

-- Workspaces
CREATE TABLE IF NOT EXISTS workspaces (
    workspace_id TEXT NOT NULL PRIMARY KEY,
    name TEXT,
    domain TEXT UNIQUE,
    status TEXT NOT NULL DEFAULT 'trial',
    admin_email TEXT,
    owner_user_id TEXT NOT NULL REFERENCES users(user_id),
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now')),
    subscription_tier TEXT NOT NULL DEFAULT 'free',
    subscription_status TEXT NOT NULL DEFAULT 'active',
    billing_cycle TEXT,
    subscription_period_start TEXT,
    subscription_period_end TEXT,
    trial_ends_at TEXT,
    ai_credits_used_usd REAL NOT NULL DEFAULT 0.0,
    user_limit INTEGER DEFAULT 1,
    stripe_customer_id TEXT,
    stripe_subscription_id TEXT,
    stripe_additional_users_item_id TEXT,
    settings TEXT,
    business_knowledge TEXT DEFAULT '',
    knowledge_updated_at TEXT,
    last_catalog_refresh TEXT,
    catalog_refresh_status TEXT DEFAULT 'idle',
    catalog_refresh_progress TEXT,
    catalog_onboarding_completed INTEGER NOT NULL DEFAULT 0,
    catalog_indexed_projects TEXT DEFAULT '[]'
);

-- Datasource configs
CREATE TABLE IF NOT EXISTS datasource_configs (
    id TEXT NOT NULL PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    datasource_type TEXT NOT NULL,
    connection_config TEXT NOT NULL DEFAULT '{}',
    active INTEGER DEFAULT 1,
    created_at TEXT DEFAULT (datetime('now')),
    updated_at TEXT DEFAULT (datetime('now')),
    slug TEXT NOT NULL,
    last_catalog_refresh TEXT,
    auto_refresh_allowed INTEGER DEFAULT 1,
    UNIQUE(workspace_id, name),
    UNIQUE(workspace_id, slug)
);

-- Agent learnings
CREATE TABLE IF NOT EXISTS agent_learnings (
    learning_id TEXT NOT NULL PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    insight TEXT NOT NULL,
    context TEXT,
    embedding BLOB,
    enabled INTEGER DEFAULT 1,
    learned_from_session TEXT,
    learned_from_user TEXT,
    created_at TEXT DEFAULT (datetime('now')),
    times_used INTEGER DEFAULT 0,
    last_used_at TEXT,
    scope TEXT NOT NULL DEFAULT 'workspace',
    superseded_by TEXT REFERENCES agent_learnings(learning_id) ON DELETE SET NULL,
    superseded_at TEXT,
    datasource_config_id TEXT REFERENCES datasource_configs(id) ON DELETE SET NULL,
    learning_type TEXT NOT NULL DEFAULT 'learning',
    reference_queries TEXT,
    structured_metadata TEXT
);

-- API tokens
CREATE TABLE IF NOT EXISTS api_tokens (
    token_id TEXT NOT NULL PRIMARY KEY,
    user_id TEXT NOT NULL REFERENCES users(user_id),
    name TEXT NOT NULL,
    token_hash TEXT NOT NULL,
    active INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    expires_at TEXT,
    last_used TEXT,
    revoked_at TEXT,
    created_by TEXT,
    revoked_by TEXT
);

-- API usage log
CREATE TABLE IF NOT EXISTS api_usage_log (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id TEXT NOT NULL REFERENCES users(user_id),
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id),
    session_id TEXT,
    timestamp TEXT NOT NULL,
    provider TEXT NOT NULL,
    model TEXT NOT NULL,
    input_tokens INTEGER NOT NULL,
    output_tokens INTEGER NOT NULL,
    total_tokens INTEGER NOT NULL,
    cache_creation_input_tokens INTEGER NOT NULL DEFAULT 0,
    cache_read_input_tokens INTEGER NOT NULL DEFAULT 0,
    cost_estimate REAL,
    component TEXT,
    request_id TEXT,
    extra_metadata TEXT
);

-- Datasource table cache (BEFORE datasource_search_embeddings which references it)
CREATE TABLE IF NOT EXISTS datasource_table_cache (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    workspace_id TEXT NOT NULL,
    project_id TEXT NOT NULL,
    dataset_id TEXT NOT NULL,
    table_id TEXT NOT NULL,
    table_metadata TEXT NOT NULL,
    column_descriptions TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now')),
    structure_refreshed_at TEXT,
    descriptions_refreshed_at TEXT,
    is_archived INTEGER NOT NULL DEFAULT 0,
    last_verified TEXT,
    datasource_config_id TEXT REFERENCES datasource_configs(id) ON DELETE CASCADE
);

-- Datasource search embeddings (AFTER datasource_table_cache)
CREATE TABLE IF NOT EXISTS datasource_search_embeddings (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    table_cache_id INTEGER NOT NULL REFERENCES datasource_table_cache(id) ON DELETE CASCADE,
    workspace_id TEXT NOT NULL,
    project_id TEXT NOT NULL,
    dataset_id TEXT NOT NULL,
    table_id TEXT NOT NULL,
    entry_type TEXT NOT NULL,
    text TEXT NOT NULL,
    weight REAL NOT NULL,
    column_name TEXT,
    embedding BLOB NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    datasource_config_id TEXT REFERENCES datasource_configs(id) ON DELETE CASCADE
);

-- Chat sessions (BEFORE chat_messages and charts)
CREATE TABLE IF NOT EXISTS chat_sessions (
    session_id TEXT NOT NULL PRIMARY KEY,
    user_id TEXT NOT NULL REFERENCES users(user_id),
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id),
    title TEXT,
    model TEXT,
    session_type TEXT NOT NULL DEFAULT 'chat',
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now')),
    config TEXT,
    shared INTEGER DEFAULT 0,
    shared_at TEXT
);

-- Chat messages (AFTER chat_sessions, BEFORE charts) (skip content_tsv tsvector column)
CREATE TABLE IF NOT EXISTS chat_messages (
    message_id TEXT NOT NULL PRIMARY KEY,
    session_id TEXT NOT NULL REFERENCES chat_sessions(session_id),
    role TEXT NOT NULL,
    content TEXT NOT NULL,
    pinned INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    extra_metadata TEXT,
    current_time_user_tz TEXT,
    sent_by_user_id TEXT REFERENCES users(user_id),
    tool_call_id TEXT,
    tool_name TEXT,
    tool_calls TEXT
);

-- Charts (AFTER chat_messages)
CREATE TABLE IF NOT EXISTS charts (
    chart_id TEXT NOT NULL PRIMARY KEY,
    message_id TEXT NOT NULL REFERENCES chat_messages(message_id) ON DELETE CASCADE,
    chart_data TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);

-- Collections (BEFORE collection_dashboards)
CREATE TABLE IF NOT EXISTS collections (
    id TEXT NOT NULL PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id),
    name TEXT NOT NULL,
    description TEXT,
    color TEXT,
    is_public INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(workspace_id, name)
);

-- Dashboards (BEFORE collection_dashboards, dashboard_versions, dashboard_views) (skip search_vector tsvector column)
CREATE TABLE IF NOT EXISTS dashboards (
    dashboard_id TEXT NOT NULL PRIMARY KEY,
    user_id TEXT NOT NULL REFERENCES users(user_id),
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id),
    title TEXT NOT NULL,
    content TEXT NOT NULL DEFAULT '',
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now')),
    last_change_summary TEXT,
    embedding BLOB
);

-- Collection dashboards (AFTER collections and dashboards)
CREATE TABLE IF NOT EXISTS collection_dashboards (
    collection_id TEXT NOT NULL,
    dashboard_id TEXT NOT NULL,
    position INTEGER NOT NULL DEFAULT 0,
    added_at TEXT NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (collection_id, dashboard_id),
    FOREIGN KEY (collection_id) REFERENCES collections(id) ON DELETE CASCADE,
    FOREIGN KEY (dashboard_id) REFERENCES dashboards(dashboard_id) ON DELETE CASCADE
);

-- Conversation read status
CREATE TABLE IF NOT EXISTS conversation_read_status (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id TEXT NOT NULL REFERENCES chat_sessions(session_id) ON DELETE CASCADE,
    user_id TEXT NOT NULL REFERENCES users(user_id) ON DELETE CASCADE,
    last_read_at TEXT NOT NULL DEFAULT (datetime('now')),
    last_read_message_id TEXT,
    UNIQUE(session_id, user_id)
);

-- Dashboard versions
CREATE TABLE IF NOT EXISTS dashboard_versions (
    version_id INTEGER PRIMARY KEY AUTOINCREMENT,
    dashboard_id TEXT NOT NULL REFERENCES dashboards(dashboard_id) ON DELETE CASCADE,
    version_number INTEGER NOT NULL,
    content TEXT NOT NULL,
    title TEXT NOT NULL,
    change_summary TEXT,
    created_by TEXT NOT NULL REFERENCES users(user_id),
    created_at TEXT DEFAULT (datetime('now')),
    content_hash TEXT,
    byte_size INTEGER,
    UNIQUE(dashboard_id, version_number)
);

-- Dashboard views
CREATE TABLE IF NOT EXISTS dashboard_views (
    view_id TEXT NOT NULL PRIMARY KEY,
    dashboard_id TEXT NOT NULL REFERENCES dashboards(dashboard_id) ON DELETE CASCADE,
    user_id TEXT NOT NULL REFERENCES users(user_id),
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id),
    viewed_at TEXT NOT NULL DEFAULT (datetime('now'))
);

-- Email subscribers
CREATE TABLE IF NOT EXISTS email_subscribers (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    email TEXT NOT NULL UNIQUE,
    company_name TEXT,
    company_size TEXT,
    use_case TEXT,
    marketing_consent INTEGER DEFAULT 0,
    created_at TEXT DEFAULT (datetime('now')),
    updated_at TEXT DEFAULT (datetime('now')),
    source TEXT DEFAULT 'web',
    notified INTEGER DEFAULT 0,
    notified_at TEXT
);

-- Feedback
CREATE TABLE IF NOT EXISTS feedback (
    id TEXT NOT NULL PRIMARY KEY,
    user_id TEXT NOT NULL REFERENCES users(user_id),
    workspace_id TEXT REFERENCES workspaces(workspace_id),
    type TEXT NOT NULL,
    description TEXT NOT NULL,
    screenshot_url TEXT,
    include_context INTEGER NOT NULL DEFAULT 1,
    context TEXT,
    status TEXT NOT NULL DEFAULT 'new',
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    resolved_at TEXT,
    resolution_notes TEXT,
    resolved_by TEXT
);

-- Watches (BEFORE notifications and watch_executions)
CREATE TABLE IF NOT EXISTS watches (
    watch_id TEXT NOT NULL PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    created_by TEXT NOT NULL REFERENCES users(user_id),
    name TEXT NOT NULL,
    prompt TEXT NOT NULL,
    schedule TEXT NOT NULL,
    datasource_hints TEXT,
    enabled INTEGER DEFAULT 1,
    last_run_at TEXT,
    last_run_status TEXT,
    next_run_at TEXT,
    created_at TEXT DEFAULT (datetime('now')),
    updated_at TEXT DEFAULT (datetime('now')),
    alert_emails_enabled INTEGER NOT NULL DEFAULT 0,
    alert_emails TEXT,
    mode TEXT DEFAULT 'alert',
    queries TEXT
);

-- Notifications (AFTER watches)
CREATE TABLE IF NOT EXISTS notifications (
    id TEXT NOT NULL PRIMARY KEY,
    user_id TEXT NOT NULL REFERENCES users(user_id) ON DELETE CASCADE,
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    type TEXT NOT NULL,
    title TEXT NOT NULL,
    message TEXT NOT NULL,
    data TEXT,
    source_watch_id TEXT REFERENCES watches(watch_id) ON DELETE SET NULL,
    read INTEGER DEFAULT 0,
    dismissed INTEGER DEFAULT 0,
    created_at TEXT DEFAULT (datetime('now')),
    read_at TEXT
);

-- OAuth clients
CREATE TABLE IF NOT EXISTS oauth_clients (
    id TEXT NOT NULL PRIMARY KEY,
    client_id TEXT NOT NULL UNIQUE,
    client_secret_hash TEXT,
    name TEXT NOT NULL,
    redirect_uris TEXT NOT NULL DEFAULT '[]',
    scopes TEXT NOT NULL DEFAULT '[]',
    client_type TEXT NOT NULL DEFAULT 'public',
    active INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

-- OAuth states
CREATE TABLE IF NOT EXISTS oauth_states (
    state TEXT NOT NULL PRIMARY KEY,
    user_id TEXT NOT NULL,
    action TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

-- Ownership transfers (skip INTERVAL default for expires_at, computed in Rust)
CREATE TABLE IF NOT EXISTS ownership_transfers (
    transfer_id TEXT NOT NULL PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id),
    from_user_id TEXT NOT NULL REFERENCES users(user_id),
    to_user_id TEXT NOT NULL REFERENCES users(user_id),
    status TEXT NOT NULL DEFAULT 'pending',
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    expires_at TEXT NOT NULL,
    completed_at TEXT
);

-- Query cache
CREATE TABLE IF NOT EXISTS query_cache (
    query_id TEXT NOT NULL PRIMARY KEY,
    sql TEXT NOT NULL,
    last_accessed_at TEXT NOT NULL DEFAULT (datetime('now')),
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

-- Refresh tokens
CREATE TABLE IF NOT EXISTS refresh_tokens (
    token_id TEXT NOT NULL PRIMARY KEY,
    user_id TEXT NOT NULL REFERENCES users(user_id),
    token_hash TEXT NOT NULL,
    demo_token_value TEXT,
    expires_at TEXT NOT NULL,
    is_active INTEGER NOT NULL DEFAULT 1,
    revoked_at TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    last_used TEXT,
    user_agent TEXT,
    ip_address TEXT,
    oauth_client_id TEXT,
    country_code TEXT
);

-- SQL query history
CREATE TABLE IF NOT EXISTS sql_query_history (
    query_id TEXT NOT NULL PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id),
    user_id TEXT NOT NULL REFERENCES users(user_id),
    query_text TEXT NOT NULL,
    executed_at TEXT NOT NULL DEFAULT (datetime('now')),
    execution_time_ms INTEGER,
    bytes_processed INTEGER,
    row_count INTEGER,
    status TEXT NOT NULL,
    error_message TEXT,
    is_saved INTEGER NOT NULL DEFAULT 0,
    query_name TEXT,
    tags TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now')),
    datasource_config_id TEXT REFERENCES datasource_configs(id) ON DELETE SET NULL
);

-- SQL query search embeddings
CREATE TABLE IF NOT EXISTS sql_query_search_embeddings (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    query_id TEXT NOT NULL REFERENCES sql_query_history(query_id) ON DELETE CASCADE,
    workspace_id TEXT NOT NULL,
    user_id TEXT NOT NULL,
    search_text TEXT NOT NULL,
    embedding BLOB NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

-- User auth methods
CREATE TABLE IF NOT EXISTS user_auth_methods (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id TEXT NOT NULL REFERENCES users(user_id),
    auth_type TEXT NOT NULL,
    auth_data TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    last_used TEXT,
    active INTEGER NOT NULL DEFAULT 1,
    UNIQUE(user_id, auth_type)
);

-- User datasource credentials
CREATE TABLE IF NOT EXISTS user_datasource_credentials (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id TEXT NOT NULL REFERENCES users(user_id) ON DELETE CASCADE,
    datasource_config_id TEXT NOT NULL REFERENCES datasource_configs(id) ON DELETE CASCADE,
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    credentials TEXT NOT NULL,
    created_at TEXT DEFAULT (datetime('now')),
    updated_at TEXT DEFAULT (datetime('now')),
    enabled INTEGER NOT NULL DEFAULT 1,
    UNIQUE(user_id, datasource_config_id)
);

-- User datasource preferences
CREATE TABLE IF NOT EXISTS user_datasource_preferences (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id TEXT NOT NULL REFERENCES users(user_id) ON DELETE CASCADE,
    datasource_config_id TEXT NOT NULL REFERENCES datasource_configs(id) ON DELETE CASCADE,
    enabled INTEGER NOT NULL DEFAULT 1,
    created_at TEXT DEFAULT (datetime('now')),
    updated_at TEXT DEFAULT (datetime('now')),
    UNIQUE(user_id, datasource_config_id)
);

-- Verification tokens
CREATE TABLE IF NOT EXISTS verification_tokens (
    token_id TEXT NOT NULL PRIMARY KEY,
    email TEXT NOT NULL,
    token_hash TEXT NOT NULL,
    token_type TEXT NOT NULL,
    expires_at TEXT NOT NULL,
    used INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    used_at TEXT
);

-- Watch executions (AFTER watches, notifications, chat_sessions)
CREATE TABLE IF NOT EXISTS watch_executions (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    watch_id TEXT REFERENCES watches(watch_id) ON DELETE SET NULL,
    started_at TEXT DEFAULT (datetime('now')),
    completed_at TEXT,
    status TEXT NOT NULL,
    agent_response TEXT,
    error_message TEXT,
    input_tokens INTEGER DEFAULT 0,
    output_tokens INTEGER DEFAULT 0,
    cost_estimate REAL,
    execution_trace TEXT,
    alert_triggered INTEGER DEFAULT 0,
    notification_id TEXT REFERENCES notifications(id) ON DELETE SET NULL,
    watch_name TEXT,
    workspace_id TEXT,
    deleted_at TEXT,
    deleted_by TEXT,
    read_at TEXT,
    dismissed_at TEXT,
    dismissed_by TEXT,
    mode TEXT,
    session_id TEXT REFERENCES chat_sessions(session_id) ON DELETE SET NULL
);

-- Workspace invitations (skip INTERVAL default for expires_at, computed in Rust)
CREATE TABLE IF NOT EXISTS workspace_invitations (
    invitation_id TEXT NOT NULL PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id),
    email TEXT NOT NULL,
    role TEXT NOT NULL DEFAULT 'user',
    invited_by_user_id TEXT NOT NULL REFERENCES users(user_id),
    status TEXT NOT NULL DEFAULT 'pending',
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    expires_at TEXT NOT NULL,
    accepted_at TEXT,
    accepted_by_user_id TEXT REFERENCES users(user_id)
);

-- Workspace knowledge chunks
CREATE TABLE IF NOT EXISTS workspace_knowledge_chunks (
    chunk_id TEXT NOT NULL PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    chunk_text TEXT NOT NULL,
    chunk_index INTEGER NOT NULL,
    embedding BLOB,
    created_at TEXT DEFAULT (datetime('now'))
);

-- Workspace usage
CREATE TABLE IF NOT EXISTS workspace_usage (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id),
    usage_date TEXT NOT NULL,
    api_calls INTEGER NOT NULL DEFAULT 0,
    tokens_used INTEGER NOT NULL DEFAULT 0,
    storage_bytes INTEGER NOT NULL DEFAULT 0,
    metrics TEXT,
    UNIQUE(workspace_id, usage_date)
);

-- Workspace users
CREATE TABLE IF NOT EXISTS workspace_users (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id),
    user_id TEXT NOT NULL REFERENCES users(user_id),
    role TEXT NOT NULL DEFAULT 'user',
    active INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    last_active TEXT,
    extra_metadata TEXT,
    UNIQUE(workspace_id, user_id)
);

-- ============================================================================
-- INDEXES
-- ============================================================================

-- agent_learnings indexes
CREATE INDEX IF NOT EXISTS idx_agent_learnings_superseded ON agent_learnings(workspace_id, enabled, superseded_by) WHERE superseded_by IS NULL;
CREATE INDEX IF NOT EXISTS idx_learnings_datasource ON agent_learnings(datasource_config_id);
CREATE INDEX IF NOT EXISTS idx_learnings_scope ON agent_learnings(workspace_id, scope, enabled);
CREATE INDEX IF NOT EXISTS idx_learnings_workspace ON agent_learnings(workspace_id);
CREATE INDEX IF NOT EXISTS idx_learnings_workspace_enabled ON agent_learnings(workspace_id, enabled);

-- api_tokens indexes
CREATE INDEX IF NOT EXISTS idx_api_tokens_active ON api_tokens(active);
CREATE INDEX IF NOT EXISTS idx_api_tokens_expires ON api_tokens(expires_at);
CREATE INDEX IF NOT EXISTS idx_api_tokens_hash ON api_tokens(token_hash);
CREATE INDEX IF NOT EXISTS idx_api_tokens_user ON api_tokens(user_id);

-- api_usage_log indexes
CREATE INDEX IF NOT EXISTS idx_api_usage_cache_tokens ON api_usage_log(cache_creation_input_tokens, cache_read_input_tokens) WHERE cache_creation_input_tokens > 0 OR cache_read_input_tokens > 0;
CREATE INDEX IF NOT EXISTS idx_api_usage_session ON api_usage_log(session_id);
CREATE INDEX IF NOT EXISTS idx_api_usage_timestamp ON api_usage_log(timestamp);
CREATE INDEX IF NOT EXISTS idx_api_usage_user ON api_usage_log(user_id);
CREATE INDEX IF NOT EXISTS idx_api_usage_workspace ON api_usage_log(workspace_id);

-- user_auth_methods indexes
CREATE INDEX IF NOT EXISTS idx_auth_methods_type ON user_auth_methods(auth_type);
CREATE INDEX IF NOT EXISTS idx_auth_methods_user ON user_auth_methods(user_id);

-- charts indexes
CREATE INDEX IF NOT EXISTS idx_charts_created ON charts(created_at);
CREATE INDEX IF NOT EXISTS idx_charts_message ON charts(message_id);

-- chat_messages indexes (skip GIN tsvector index)
CREATE INDEX IF NOT EXISTS idx_chat_messages_created ON chat_messages(created_at);
CREATE INDEX IF NOT EXISTS idx_chat_messages_role ON chat_messages(role);
CREATE INDEX IF NOT EXISTS idx_chat_messages_sent_by_user ON chat_messages(sent_by_user_id);
CREATE INDEX IF NOT EXISTS idx_chat_messages_session ON chat_messages(session_id);
CREATE INDEX IF NOT EXISTS idx_chat_messages_tool_call_id ON chat_messages(tool_call_id);

-- chat_sessions indexes
CREATE INDEX IF NOT EXISTS idx_chat_sessions_created ON chat_sessions(created_at);
CREATE INDEX IF NOT EXISTS idx_chat_sessions_shared ON chat_sessions(workspace_id, shared) WHERE shared = 1;
CREATE INDEX IF NOT EXISTS idx_chat_sessions_type ON chat_sessions(session_type);
CREATE INDEX IF NOT EXISTS idx_chat_sessions_user ON chat_sessions(user_id);
CREATE INDEX IF NOT EXISTS idx_chat_sessions_workspace ON chat_sessions(workspace_id);

-- collection_dashboards indexes
CREATE INDEX IF NOT EXISTS idx_collection_dashboards_collection_id ON collection_dashboards(collection_id);
CREATE INDEX IF NOT EXISTS idx_collection_dashboards_dashboard_id ON collection_dashboards(dashboard_id);
CREATE INDEX IF NOT EXISTS idx_collection_dashboards_position ON collection_dashboards(collection_id, position);

-- collections indexes
CREATE INDEX IF NOT EXISTS idx_collections_is_public ON collections(is_public);
CREATE INDEX IF NOT EXISTS idx_collections_workspace_id ON collections(workspace_id);

-- conversation_read_status indexes
CREATE INDEX IF NOT EXISTS idx_conversation_read_status_user ON conversation_read_status(user_id);

-- dashboard_versions indexes
CREATE INDEX IF NOT EXISTS idx_dashboard_versions_created ON dashboard_versions(created_at DESC);
CREATE INDEX IF NOT EXISTS idx_dashboard_versions_dashboard ON dashboard_versions(dashboard_id);
CREATE INDEX IF NOT EXISTS idx_dashboard_versions_user ON dashboard_versions(created_by);

-- dashboard_views indexes
CREATE INDEX IF NOT EXISTS idx_dashboard_views_dashboard ON dashboard_views(dashboard_id);
CREATE INDEX IF NOT EXISTS idx_dashboard_views_viewed_at ON dashboard_views(viewed_at);
CREATE INDEX IF NOT EXISTS idx_dashboard_views_workspace ON dashboard_views(workspace_id);

-- dashboards indexes (skip HNSW embedding index and GIN search_vector index)
CREATE INDEX IF NOT EXISTS idx_dashboards_created ON dashboards(created_at);
CREATE INDEX IF NOT EXISTS idx_dashboards_updated ON dashboards(updated_at);
CREATE INDEX IF NOT EXISTS idx_dashboards_user ON dashboards(user_id);
CREATE INDEX IF NOT EXISTS idx_dashboards_workspace_id ON dashboards(workspace_id);

-- datasource_table_cache indexes
CREATE INDEX IF NOT EXISTS idx_datasource_cache_archived ON datasource_table_cache(is_archived);
CREATE INDEX IF NOT EXISTS idx_datasource_cache_dataset ON datasource_table_cache(dataset_id);
CREATE INDEX IF NOT EXISTS idx_datasource_cache_descriptions_refresh ON datasource_table_cache(descriptions_refreshed_at);
CREATE INDEX IF NOT EXISTS idx_datasource_cache_project ON datasource_table_cache(project_id);
CREATE INDEX IF NOT EXISTS idx_datasource_cache_structure_refresh ON datasource_table_cache(structure_refreshed_at);
CREATE INDEX IF NOT EXISTS idx_datasource_cache_updated ON datasource_table_cache(updated_at);
CREATE INDEX IF NOT EXISTS idx_datasource_cache_workspace ON datasource_table_cache(workspace_id);
CREATE INDEX IF NOT EXISTS idx_table_cache_datasource ON datasource_table_cache(datasource_config_id);

-- datasource_configs indexes
CREATE INDEX IF NOT EXISTS idx_datasource_configs_active ON datasource_configs(active);
CREATE INDEX IF NOT EXISTS idx_datasource_configs_slug ON datasource_configs(workspace_id, slug);
CREATE INDEX IF NOT EXISTS idx_datasource_configs_type ON datasource_configs(datasource_type);
CREATE INDEX IF NOT EXISTS idx_datasource_configs_workspace ON datasource_configs(workspace_id);

-- datasource_search_embeddings indexes (skip HNSW vector index)
CREATE INDEX IF NOT EXISTS idx_search_emb_table ON datasource_search_embeddings(project_id, dataset_id, table_id);
CREATE INDEX IF NOT EXISTS idx_search_emb_type ON datasource_search_embeddings(entry_type);
CREATE INDEX IF NOT EXISTS idx_search_emb_workspace ON datasource_search_embeddings(workspace_id);
CREATE INDEX IF NOT EXISTS idx_search_embedding_datasource ON datasource_search_embeddings(datasource_config_id);

-- email_subscribers indexes
CREATE INDEX IF NOT EXISTS idx_email_subscribers_created_at ON email_subscribers(created_at DESC);
CREATE INDEX IF NOT EXISTS idx_email_subscribers_email ON email_subscribers(email);
CREATE INDEX IF NOT EXISTS idx_email_subscribers_marketing_consent ON email_subscribers(marketing_consent) WHERE marketing_consent = 1;

-- feedback indexes
CREATE INDEX IF NOT EXISTS idx_feedback_created ON feedback(created_at);
CREATE INDEX IF NOT EXISTS idx_feedback_status ON feedback(status);
CREATE INDEX IF NOT EXISTS idx_feedback_type ON feedback(type);
CREATE INDEX IF NOT EXISTS idx_feedback_user ON feedback(user_id);
CREATE INDEX IF NOT EXISTS idx_feedback_workspace ON feedback(workspace_id);

-- workspace_knowledge_chunks indexes (skip IVFFLAT vector index)
CREATE INDEX IF NOT EXISTS idx_knowledge_chunks_workspace ON workspace_knowledge_chunks(workspace_id);
CREATE INDEX IF NOT EXISTS idx_knowledge_chunks_workspace_index ON workspace_knowledge_chunks(workspace_id, chunk_index);

-- notifications indexes
CREATE INDEX IF NOT EXISTS idx_notifications_created ON notifications(created_at DESC);
CREATE INDEX IF NOT EXISTS idx_notifications_source_watch ON notifications(source_watch_id);
CREATE INDEX IF NOT EXISTS idx_notifications_unread ON notifications(user_id, read) WHERE read = 0;
CREATE INDEX IF NOT EXISTS idx_notifications_user ON notifications(user_id);
CREATE INDEX IF NOT EXISTS idx_notifications_workspace ON notifications(workspace_id);

-- oauth_clients indexes
CREATE INDEX IF NOT EXISTS idx_oauth_clients_client_id ON oauth_clients(client_id);

-- oauth_states indexes
CREATE INDEX IF NOT EXISTS idx_oauth_states_created_at ON oauth_states(created_at);

-- ownership_transfers indexes
CREATE INDEX IF NOT EXISTS idx_ownership_transfers_expires ON ownership_transfers(expires_at);
CREATE INDEX IF NOT EXISTS idx_ownership_transfers_status ON ownership_transfers(status);
CREATE INDEX IF NOT EXISTS idx_ownership_transfers_to_user ON ownership_transfers(to_user_id);
CREATE INDEX IF NOT EXISTS idx_ownership_transfers_workspace ON ownership_transfers(workspace_id);
CREATE UNIQUE INDEX IF NOT EXISTS unique_pending_transfer ON ownership_transfers(workspace_id) WHERE status = 'pending';

-- query_cache indexes
CREATE INDEX IF NOT EXISTS idx_query_cache_last_accessed ON query_cache(last_accessed_at);

-- sql_query_history indexes (skip GIN trigram index)
CREATE INDEX IF NOT EXISTS idx_query_history_datasource ON sql_query_history(datasource_config_id, workspace_id);
CREATE INDEX IF NOT EXISTS idx_query_history_executed_at ON sql_query_history(executed_at);
CREATE INDEX IF NOT EXISTS idx_query_history_saved ON sql_query_history(is_saved, workspace_id, user_id);
CREATE INDEX IF NOT EXISTS idx_query_history_workspace_user ON sql_query_history(workspace_id, user_id);

-- sql_query_search_embeddings indexes (skip HNSW vector index)
CREATE INDEX IF NOT EXISTS idx_query_search_emb_workspace_user ON sql_query_search_embeddings(workspace_id, user_id);

-- refresh_tokens indexes
CREATE INDEX IF NOT EXISTS idx_refresh_tokens_active ON refresh_tokens(is_active);
CREATE INDEX IF NOT EXISTS idx_refresh_tokens_expires ON refresh_tokens(expires_at);
CREATE INDEX IF NOT EXISTS idx_refresh_tokens_hash ON refresh_tokens(token_hash);
CREATE INDEX IF NOT EXISTS idx_refresh_tokens_user ON refresh_tokens(user_id);

-- user_datasource_credentials indexes
CREATE INDEX IF NOT EXISTS idx_user_datasource_creds_datasource ON user_datasource_credentials(datasource_config_id);
CREATE INDEX IF NOT EXISTS idx_user_datasource_creds_user ON user_datasource_credentials(user_id);
CREATE INDEX IF NOT EXISTS idx_user_datasource_creds_workspace ON user_datasource_credentials(workspace_id);

-- user_datasource_preferences indexes
CREATE INDEX IF NOT EXISTS idx_user_datasource_pref_datasource ON user_datasource_preferences(datasource_config_id);
CREATE INDEX IF NOT EXISTS idx_user_datasource_pref_user ON user_datasource_preferences(user_id);

-- users indexes
CREATE INDEX IF NOT EXISTS idx_users_active ON users(active);
CREATE INDEX IF NOT EXISTS idx_users_created_at ON users(created_at);
CREATE INDEX IF NOT EXISTS idx_users_email ON users(email);
CREATE INDEX IF NOT EXISTS idx_users_last_workspace_id ON users(last_workspace_id);
CREATE INDEX IF NOT EXISTS idx_users_terms_accepted_at ON users(terms_accepted_at);

-- verification_tokens indexes
CREATE INDEX IF NOT EXISTS idx_verification_tokens_email ON verification_tokens(email);
CREATE INDEX IF NOT EXISTS idx_verification_tokens_expires ON verification_tokens(expires_at);
CREATE INDEX IF NOT EXISTS idx_verification_tokens_hash ON verification_tokens(token_hash);
CREATE INDEX IF NOT EXISTS idx_verification_tokens_type ON verification_tokens(token_type);

-- watch_executions indexes
CREATE INDEX IF NOT EXISTS idx_watch_executions_alert ON watch_executions(alert_triggered) WHERE alert_triggered = 1;
CREATE INDEX IF NOT EXISTS idx_watch_executions_completed ON watch_executions(completed_at DESC);
CREATE INDEX IF NOT EXISTS idx_watch_executions_deleted_at ON watch_executions(deleted_at);
CREATE INDEX IF NOT EXISTS idx_watch_executions_dismissed ON watch_executions(alert_triggered, deleted_at) WHERE alert_triggered = 1 AND deleted_at IS NULL;
CREATE INDEX IF NOT EXISTS idx_watch_executions_notification ON watch_executions(notification_id);
CREATE INDEX IF NOT EXISTS idx_watch_executions_read_at ON watch_executions(read_at);
CREATE INDEX IF NOT EXISTS idx_watch_executions_session ON watch_executions(session_id);
CREATE INDEX IF NOT EXISTS idx_watch_executions_started ON watch_executions(started_at DESC);
CREATE INDEX IF NOT EXISTS idx_watch_executions_status ON watch_executions(status);
CREATE INDEX IF NOT EXISTS idx_watch_executions_watch ON watch_executions(watch_id);
CREATE INDEX IF NOT EXISTS idx_watch_executions_workspace ON watch_executions(workspace_id);

-- watches indexes
CREATE INDEX IF NOT EXISTS idx_watches_creator ON watches(created_by);
CREATE INDEX IF NOT EXISTS idx_watches_enabled ON watches(enabled) WHERE enabled = 1;
CREATE UNIQUE INDEX IF NOT EXISTS idx_watches_name_workspace_unique ON watches(workspace_id, name);
CREATE INDEX IF NOT EXISTS idx_watches_next_run ON watches(next_run_at);
CREATE INDEX IF NOT EXISTS idx_watches_workspace ON watches(workspace_id);

-- workspace_invitations indexes
CREATE INDEX IF NOT EXISTS idx_workspace_invitations_email ON workspace_invitations(email);
CREATE INDEX IF NOT EXISTS idx_workspace_invitations_status ON workspace_invitations(status);
CREATE INDEX IF NOT EXISTS idx_workspace_invitations_workspace ON workspace_invitations(workspace_id);

-- workspace_usage indexes
CREATE INDEX IF NOT EXISTS idx_workspace_usage_date ON workspace_usage(usage_date);
CREATE INDEX IF NOT EXISTS idx_workspace_usage_workspace ON workspace_usage(workspace_id);

-- workspace_users indexes
CREATE INDEX IF NOT EXISTS idx_workspace_users_role ON workspace_users(role);
CREATE INDEX IF NOT EXISTS idx_workspace_users_user ON workspace_users(user_id);
CREATE INDEX IF NOT EXISTS idx_workspace_users_workspace ON workspace_users(workspace_id);

-- workspaces indexes
CREATE INDEX IF NOT EXISTS idx_workspaces_catalog_onboarding ON workspaces(catalog_onboarding_completed);
CREATE INDEX IF NOT EXISTS idx_workspaces_catalog_status ON workspaces(catalog_refresh_status);
CREATE INDEX IF NOT EXISTS idx_workspaces_created_at ON workspaces(created_at);
CREATE INDEX IF NOT EXISTS idx_workspaces_domain ON workspaces(domain);
CREATE INDEX IF NOT EXISTS idx_workspaces_owner_user_id ON workspaces(owner_user_id);
CREATE INDEX IF NOT EXISTS idx_workspaces_status ON workspaces(status);
CREATE INDEX IF NOT EXISTS idx_workspaces_stripe_customer ON workspaces(stripe_customer_id);
CREATE INDEX IF NOT EXISTS idx_workspaces_subscription_status ON workspaces(subscription_status);
CREATE INDEX IF NOT EXISTS idx_workspaces_subscription_tier ON workspaces(subscription_tier);

-- ============================================================================
-- FTS5 VIRTUAL TABLE (replaces Postgres tsvector for agent_learnings)
-- ============================================================================

CREATE VIRTUAL TABLE IF NOT EXISTS agent_learnings_fts USING fts5(
    learning_id UNINDEXED,
    insight,
    context,
    content='agent_learnings',
    content_rowid='rowid'
);

-- Sync triggers to keep FTS5 index in sync with agent_learnings
-- Mirrors Postgres tsvector which indexes both insight (weight A) and context (weight B)
CREATE TRIGGER agent_learnings_fts_insert AFTER INSERT ON agent_learnings BEGIN
    INSERT INTO agent_learnings_fts(rowid, learning_id, insight, context)
    VALUES (NEW.rowid, NEW.learning_id, COALESCE(NEW.insight, ''), COALESCE(NEW.context, ''));
END;

CREATE TRIGGER agent_learnings_fts_delete AFTER DELETE ON agent_learnings BEGIN
    INSERT INTO agent_learnings_fts(agent_learnings_fts, rowid, learning_id, insight, context)
    VALUES ('delete', OLD.rowid, OLD.learning_id, COALESCE(OLD.insight, ''), COALESCE(OLD.context, ''));
END;

CREATE TRIGGER agent_learnings_fts_update AFTER UPDATE ON agent_learnings BEGIN
    INSERT INTO agent_learnings_fts(agent_learnings_fts, rowid, learning_id, insight, context)
    VALUES ('delete', OLD.rowid, OLD.learning_id, COALESCE(OLD.insight, ''), COALESCE(OLD.context, ''));
    INSERT INTO agent_learnings_fts(rowid, learning_id, insight, context)
    VALUES (NEW.rowid, NEW.learning_id, COALESCE(NEW.insight, ''), COALESCE(NEW.context, ''));
END;
