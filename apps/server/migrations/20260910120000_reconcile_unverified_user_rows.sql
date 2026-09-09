-- KYO-683: reconcile `users` rows left `verified = false` by the pre-KYO-683
-- signup flow, which wrote a `users` row at signup/start, before the address
-- was ever confirmed. On dev this produced 19 unverified rows out of 24
-- users. As of this migration, `signup_start_service`'s SaaS path no longer
-- writes a `users` row at all -- it only mints a token and sends the email;
-- the row is created (`verified = true`) only when the token is redeemed in
-- `signup_verify_service`. This migration is the one-time cleanup of rows
-- created under the old behaviour, going forward.
--
-- Two dispositions for a `verified = false` row:
--
--  1. VERIFY (force `verified = true`) if the row is referenced anywhere
--     else in the schema -- it holds credentials or owns data, so it's a
--     real account that got stuck mid-signup, not a rejectable draft.
--     Deleting it would destroy someone's account. Once verified, it is
--     usable exactly like any other verified-but-credential-less account:
--     `recovery_start_service` already serves those (`user exists &&
--     user.verified`, no credential requirement).
--
--  2. DELETE if the row is provably inert -- referenced by nothing. This is
--     an abandoned signup: the address was never confirmed and nothing in
--     the product ever touched the row.
--
-- The referencing-table list below was built by tracing every migration
-- file under apps/server/migrations/ (not just the baseline) for columns
-- that point at users.user_id, in either direction:
--
--   - Declared, enforced foreign keys (23 in the baseline, plus
--     push_subscriptions, platform_user_links, workspace_integrations,
--     workspace_user_integrations, and collections added later). For these,
--     even a missed table would not silently orphan a row: Postgres would
--     reject the DELETE below with a foreign-key-violation error rather
--     than running it, because this migration runs after every migration
--     that adds those constraints.
--   - Columns that logically reference users.user_id but carry no DB-level
--     FK (oauth_states.user_id, sql_query_search_embeddings.user_id,
--     watch_executions.created_by/deleted_by/dismissed_by,
--     sync_log.owner_user_id, dashboards.created_by/updated_by,
--     feedback.resolved_by, chartml_validation_log.user_id,
--     conversation_discussed.user_id). These have no such safety net -- an
--     omission here really would silently orphan a row -- so they are
--     included explicitly. dashboards.{created_by,updated_by} are written
--     at dashboard_service.rs:414 (INSERT) and :691 (UPDATE);
--     watch_executions.deleted_by is written at watch_service.rs:1791/1894;
--     chartml_validation_log.user_id is written at agent.rs:1107 and
--     watch_execution.rs:1386. watch_executions.dismissed_by,
--     feedback.resolved_by, and conversation_discussed.user_id are declared
--     columns with no current Rust write site -- included anyway since an
--     unwritten column costs nothing to check and this DELETE is
--     irreversible on a deployed database.
--
-- Two tables intentionally excluded, both checked and rejected:
--   - verification_tokens: keyed by email only, no user_id column, no FK.
--     Not a reference to a specific users row at all.
--   - api_tokens.created_by / revoked_by: varchar(255), free-text
--     admin/audit annotations (distinct from api_tokens.user_id, the real
--     owner FK, which IS covered below) -- not confirmed to be a user_id
--     reference, so not included; api_tokens.user_id already covers
--     ownership of that table.
--
-- workspaces.slack_installed_by_user_id and workspace_users.slack_user_id
-- existed in the original baseline but were dropped by
-- 20260308103230_drop_legacy_slack_columns.sql (replaced by
-- workspace_integrations.installed_by and platform_user_links.user_id /
-- workspace_user_integrations.user_id, both covered below) -- they no
-- longer exist in the schema this migration runs against and are excluded
-- for that reason, not merely omitted.
--
-- knowledge_files.created_by / updated_by are also excluded, but for a
-- different reason than the slack columns above: knowledge_files is never
-- dropped by a SQL migration -- it is dropped at runtime by the Rust
-- post-migration hook `kyomi_knowledge::unify::migrate_folders_to_collections`
-- (apps/server/src/main.rs, called right after migrations run on every
-- boot; see 20260409000000_unify_knowledge_dashboards.sql's header comment).
-- On any already-deployed database that has booted even once since that
-- migration shipped, the table is already physically gone by the time this
-- migration runs, and `EXISTS (SELECT 1 FROM knowledge_files ...)` would
-- fail the whole migration with "relation \"knowledge_files\" does not
-- exist" rather than silently doing the wrong thing. It is not merely
-- redundant like the FK-enforced tables above -- referencing it is actively
-- unsafe, so it must stay excluded. (The rows it used to hold are not lost
-- from this reconciliation's point of view either: 20260409000000 copies
-- every knowledge_files row's created_by/updated_by into the corresponding
-- dashboards row before the table is dropped, and dashboards is covered
-- above.)

UPDATE users
SET verified = true, updated_at = now()
WHERE verified = false
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
WHERE verified = false
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
