-- KYO-683: reconcile `users` rows left `verified = 0` by the pre-KYO-683
-- signup flow, which wrote a `users` row at signup/start, before the address
-- was ever confirmed. On dev this produced 19 unverified rows out of 24
-- users. As of this migration, `signup_start_service`'s SaaS path no longer
-- writes a `users` row at all -- it only mints a token and sends the email;
-- the row is created (`verified = 1`) only when the token is redeemed in
-- `signup_verify_service`. This migration is the one-time cleanup of rows
-- created under the old behaviour, going forward.
--
-- SQLite twin of 20260910120000_reconcile_unverified_user_rows.sql -- see
-- that file for the full rationale (the two dispositions, and how the
-- referencing-table list below was built by tracing every migration file,
-- not just the baseline, for columns pointing at users.user_id, including
-- the dashboards/watch_executions/feedback/chartml_validation_log/
-- conversation_discussed columns and why knowledge_files is excluded).
-- Table and column names match 1:1 between the two chains; this file only
-- differs in SQLite's `verified = 0/1` integer booleans and
-- `datetime('now')`.

UPDATE users
SET verified = 1, updated_at = datetime('now')
WHERE verified = 0
  AND (
       EXISTS (SELECT 1 FROM api_tokens t WHERE t.user_id = users.user_id)
    OR EXISTS (SELECT 1 FROM api_usage_log t WHERE t.user_id = users.user_id)
    OR EXISTS (SELECT 1 FROM chat_messages t WHERE t.sent_by_user_id = users.user_id)
    OR EXISTS (SELECT 1 FROM chat_sessions t WHERE t.user_id = users.user_id)
    OR EXISTS (SELECT 1 FROM conversation_read_status t WHERE t.user_id = users.user_id)
    OR EXISTS (SELECT 1 FROM dashboard_versions t WHERE t.created_by = users.user_id)
    OR EXISTS (SELECT 1 FROM dashboard_views t WHERE t.user_id = users.user_id)
    OR EXISTS (SELECT 1 FROM dashboards t WHERE t.user_id = users.user_id)
    OR EXISTS (SELECT 1 FROM dashboards t WHERE t.created_by = users.user_id)
    OR EXISTS (SELECT 1 FROM dashboards t WHERE t.updated_by = users.user_id)
    OR EXISTS (SELECT 1 FROM feedback t WHERE t.user_id = users.user_id)
    OR EXISTS (SELECT 1 FROM feedback t WHERE t.resolved_by = users.user_id)
    OR EXISTS (SELECT 1 FROM notifications t WHERE t.user_id = users.user_id)
    OR EXISTS (
         SELECT 1 FROM ownership_transfers t
         WHERE t.from_user_id = users.user_id OR t.to_user_id = users.user_id
       )
    OR EXISTS (SELECT 1 FROM refresh_tokens t WHERE t.user_id = users.user_id)
    OR EXISTS (SELECT 1 FROM sql_query_history t WHERE t.user_id = users.user_id)
    OR EXISTS (SELECT 1 FROM user_auth_methods t WHERE t.user_id = users.user_id)
    OR EXISTS (SELECT 1 FROM user_datasource_credentials t WHERE t.user_id = users.user_id)
    OR EXISTS (SELECT 1 FROM user_datasource_preferences t WHERE t.user_id = users.user_id)
    OR EXISTS (SELECT 1 FROM watches t WHERE t.created_by = users.user_id)
    OR EXISTS (
         SELECT 1 FROM workspace_invitations t
         WHERE t.accepted_by_user_id = users.user_id OR t.invited_by_user_id = users.user_id
       )
    OR EXISTS (SELECT 1 FROM workspace_users t WHERE t.user_id = users.user_id)
    OR EXISTS (SELECT 1 FROM workspaces t WHERE t.owner_user_id = users.user_id)
    OR EXISTS (SELECT 1 FROM push_subscriptions t WHERE t.user_id = users.user_id)
    OR EXISTS (SELECT 1 FROM platform_user_links t WHERE t.user_id = users.user_id)
    OR EXISTS (SELECT 1 FROM workspace_integrations t WHERE t.installed_by = users.user_id)
    OR EXISTS (SELECT 1 FROM workspace_user_integrations t WHERE t.user_id = users.user_id)
    OR EXISTS (SELECT 1 FROM collections t WHERE t.created_by = users.user_id)
    OR EXISTS (SELECT 1 FROM oauth_states t WHERE t.user_id = users.user_id)
    OR EXISTS (SELECT 1 FROM sql_query_search_embeddings t WHERE t.user_id = users.user_id)
    OR EXISTS (SELECT 1 FROM watch_executions t WHERE t.created_by = users.user_id)
    OR EXISTS (SELECT 1 FROM watch_executions t WHERE t.deleted_by = users.user_id)
    OR EXISTS (SELECT 1 FROM watch_executions t WHERE t.dismissed_by = users.user_id)
    OR EXISTS (SELECT 1 FROM sync_log t WHERE t.owner_user_id = users.user_id)
    OR EXISTS (SELECT 1 FROM chartml_validation_log t WHERE t.user_id = users.user_id)
    OR EXISTS (SELECT 1 FROM conversation_discussed t WHERE t.user_id = users.user_id)
  );

DELETE FROM users
WHERE verified = 0
  AND NOT EXISTS (SELECT 1 FROM api_tokens t WHERE t.user_id = users.user_id)
  AND NOT EXISTS (SELECT 1 FROM api_usage_log t WHERE t.user_id = users.user_id)
  AND NOT EXISTS (SELECT 1 FROM chat_messages t WHERE t.sent_by_user_id = users.user_id)
  AND NOT EXISTS (SELECT 1 FROM chat_sessions t WHERE t.user_id = users.user_id)
  AND NOT EXISTS (SELECT 1 FROM conversation_read_status t WHERE t.user_id = users.user_id)
  AND NOT EXISTS (SELECT 1 FROM dashboard_versions t WHERE t.created_by = users.user_id)
  AND NOT EXISTS (SELECT 1 FROM dashboard_views t WHERE t.user_id = users.user_id)
  AND NOT EXISTS (SELECT 1 FROM dashboards t WHERE t.user_id = users.user_id)
  AND NOT EXISTS (SELECT 1 FROM dashboards t WHERE t.created_by = users.user_id)
  AND NOT EXISTS (SELECT 1 FROM dashboards t WHERE t.updated_by = users.user_id)
  AND NOT EXISTS (SELECT 1 FROM feedback t WHERE t.user_id = users.user_id)
  AND NOT EXISTS (SELECT 1 FROM feedback t WHERE t.resolved_by = users.user_id)
  AND NOT EXISTS (SELECT 1 FROM notifications t WHERE t.user_id = users.user_id)
  AND NOT EXISTS (
        SELECT 1 FROM ownership_transfers t
        WHERE t.from_user_id = users.user_id OR t.to_user_id = users.user_id
      )
  AND NOT EXISTS (SELECT 1 FROM refresh_tokens t WHERE t.user_id = users.user_id)
  AND NOT EXISTS (SELECT 1 FROM sql_query_history t WHERE t.user_id = users.user_id)
  AND NOT EXISTS (SELECT 1 FROM user_auth_methods t WHERE t.user_id = users.user_id)
  AND NOT EXISTS (SELECT 1 FROM user_datasource_credentials t WHERE t.user_id = users.user_id)
  AND NOT EXISTS (SELECT 1 FROM user_datasource_preferences t WHERE t.user_id = users.user_id)
  AND NOT EXISTS (SELECT 1 FROM watches t WHERE t.created_by = users.user_id)
  AND NOT EXISTS (
        SELECT 1 FROM workspace_invitations t
        WHERE t.accepted_by_user_id = users.user_id OR t.invited_by_user_id = users.user_id
      )
  AND NOT EXISTS (SELECT 1 FROM workspace_users t WHERE t.user_id = users.user_id)
  AND NOT EXISTS (SELECT 1 FROM workspaces t WHERE t.owner_user_id = users.user_id)
  AND NOT EXISTS (SELECT 1 FROM push_subscriptions t WHERE t.user_id = users.user_id)
  AND NOT EXISTS (SELECT 1 FROM platform_user_links t WHERE t.user_id = users.user_id)
  AND NOT EXISTS (SELECT 1 FROM workspace_integrations t WHERE t.installed_by = users.user_id)
  AND NOT EXISTS (SELECT 1 FROM workspace_user_integrations t WHERE t.user_id = users.user_id)
  AND NOT EXISTS (SELECT 1 FROM collections t WHERE t.created_by = users.user_id)
  AND NOT EXISTS (SELECT 1 FROM oauth_states t WHERE t.user_id = users.user_id)
  AND NOT EXISTS (SELECT 1 FROM sql_query_search_embeddings t WHERE t.user_id = users.user_id)
  AND NOT EXISTS (SELECT 1 FROM watch_executions t WHERE t.created_by = users.user_id)
  AND NOT EXISTS (SELECT 1 FROM watch_executions t WHERE t.deleted_by = users.user_id)
  AND NOT EXISTS (SELECT 1 FROM watch_executions t WHERE t.dismissed_by = users.user_id)
  AND NOT EXISTS (SELECT 1 FROM sync_log t WHERE t.owner_user_id = users.user_id)
  AND NOT EXISTS (SELECT 1 FROM chartml_validation_log t WHERE t.user_id = users.user_id)
  AND NOT EXISTS (SELECT 1 FROM conversation_discussed t WHERE t.user_id = users.user_id);
