-- Fix workspace role column defaults.
--
-- The baseline schema created `workspace_invitations.role` and
-- `workspace_users.role` with DEFAULT 'user', but the Rust WorkspaceRole
-- enum only recognises 'workspace_user' (not 'user').  Any row that was
-- inserted without an explicit role (or that pre-dates the enum rename)
-- causes a 500 when the ORM deserialises it.
--
-- This migration:
--   1. Back-fills any existing 'user' values to 'workspace_user'.
--   2. Corrects the column defaults so future bare INSERTs are safe.

UPDATE workspace_invitations
SET role = 'workspace_user'
WHERE role = 'user';

ALTER TABLE workspace_invitations
    ALTER COLUMN role SET DEFAULT 'workspace_user';

UPDATE workspace_users
SET role = 'workspace_user'
WHERE role = 'user';

ALTER TABLE workspace_users
    ALTER COLUMN role SET DEFAULT 'workspace_user';
