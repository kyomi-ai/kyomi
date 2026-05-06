-- Phase 12 Task 6: Drop legacy slack columns from workspaces, workspace_users,
-- watches, and chat_sessions.  These columns are replaced by the platform
-- abstraction tables (workspace_integrations, workspace_user_integrations,
-- platform_user_links, watch_alert_channels).

-- workspaces: remove per-workspace Slack installation columns
ALTER TABLE workspaces DROP COLUMN IF EXISTS slack_team_id;
ALTER TABLE workspaces DROP COLUMN IF EXISTS slack_team_name;
ALTER TABLE workspaces DROP COLUMN IF EXISTS slack_bot_token;
ALTER TABLE workspaces DROP COLUMN IF EXISTS slack_bot_user_id;
ALTER TABLE workspaces DROP COLUMN IF EXISTS slack_installed_by_user_id;
ALTER TABLE workspaces DROP COLUMN IF EXISTS slack_installed_at;

-- workspace_users: remove per-user Slack connection columns
ALTER TABLE workspace_users DROP COLUMN IF EXISTS slack_user_id;
ALTER TABLE workspace_users DROP COLUMN IF EXISTS slack_username;
ALTER TABLE workspace_users DROP COLUMN IF EXISTS slack_connected_at;
ALTER TABLE workspace_users DROP COLUMN IF EXISTS slack_default_channel_id;
ALTER TABLE workspace_users DROP COLUMN IF EXISTS slack_default_channel_name;
ALTER TABLE workspace_users DROP COLUMN IF EXISTS slack_user_token;
ALTER TABLE workspace_users DROP COLUMN IF EXISTS slack_user_refresh_token;
ALTER TABLE workspace_users DROP COLUMN IF EXISTS slack_user_token_expires_at;
ALTER TABLE workspace_users DROP COLUMN IF EXISTS slack_timezone;
ALTER TABLE workspace_users DROP COLUMN IF EXISTS slack_timezone_fetched_at;

-- watches: remove per-watch Slack channel column
ALTER TABLE watches DROP COLUMN IF EXISTS slack_channel_id;

-- chat_sessions: drop legacy slack columns (platform_type and platform_thread_key
-- were already added in the platform_integration_tables migration)
ALTER TABLE chat_sessions DROP COLUMN IF EXISTS slack_channel_id;
ALTER TABLE chat_sessions DROP COLUMN IF EXISTS slack_thread_ts;
