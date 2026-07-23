-- Add visibility columns to sync_log for per-user delta filtering.
-- owner_user_id: the entity's owner (NULL for workspace-wide entities).
-- is_workspace_visible: true (1) = visible to all workspace members.

ALTER TABLE sync_log ADD COLUMN owner_user_id TEXT NULL;
ALTER TABLE sync_log ADD COLUMN is_workspace_visible INTEGER NOT NULL DEFAULT 1;

-- Index for the delta query: fetch entries for a workspace, filtered by owner.
CREATE INDEX IF NOT EXISTS idx_sync_log_visibility ON sync_log(workspace_id, owner_user_id, sync_id);

-- Backfill: workspace-wide entities stay visible to all, owner_user_id stays NULL.
UPDATE sync_log
   SET is_workspace_visible = 1,
       owner_user_id = NULL
 WHERE entity_type IN ('watch', 'workspace_settings');

-- Backfill: private entities default to NOT visible; extract owner from data JSON.
UPDATE sync_log
   SET is_workspace_visible = 0,
       owner_user_id = json_extract(data, '$.user_id')
 WHERE entity_type IN ('dashboard', 'knowledge');

UPDATE sync_log
   SET is_workspace_visible = 0,
       owner_user_id = json_extract(data, '$.created_by.user_id')
 WHERE entity_type = 'chat_session';
