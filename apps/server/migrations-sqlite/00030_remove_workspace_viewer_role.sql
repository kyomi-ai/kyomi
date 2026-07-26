-- KYO-183: remove the never-assignable `workspace_viewer` role.
--
-- `WorkspaceRole::WorkspaceViewer` has no UI path that can ever create one --
-- the invite form only offers "user"/"admin", and `map_role_to_db` maps
-- anything other than "admin" to `roles.user`. Product decision (Jason,
-- 2026-07-25): there is no read-only role and none is planned, so the
-- variant is being removed from the Rust enum.
--
-- `WorkspaceRole::FromStr` returns `Err` on any string it doesn't recognise
-- (crates/kyomi-core/src/enums.rs). Once the `WorkspaceViewer` variant is
-- gone, any row still holding the literal string 'workspace_viewer' fails
-- deserialization at read time -- a hard runtime error, not a silent
-- fallback. This migration must run and land before that code change ships.
--
-- Only `workspace_users` and `workspace_invitations` store workspace-role
-- tokens (confirmed against 00001_baseline.sql and every migration since --
-- `chat_messages.role` is an unrelated chat-message-speaker column, not a
-- workspace role). No production row has ever been able to hold
-- 'workspace_viewer' since nothing can assign it, but this covers any row
-- created by a since-removed code path, a manual DB edit, or test/seed data.
--
-- Idempotent: the WHERE clause makes re-running a no-op.
UPDATE workspace_users
   SET role = 'workspace_user'
 WHERE role = 'workspace_viewer';

UPDATE workspace_invitations
   SET role = 'workspace_user'
 WHERE role = 'workspace_viewer';
