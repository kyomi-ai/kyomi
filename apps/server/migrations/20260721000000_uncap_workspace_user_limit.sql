-- Workspace seat cap removed (KYO-167): member count is never capped — billing
-- is per active user. Lift all existing workspaces to unlimited and change the
-- column default so new workspaces are uncapped by default.
UPDATE workspaces
SET user_limit = 999999
WHERE user_limit IS NULL OR user_limit < 999999;

ALTER TABLE workspaces ALTER COLUMN user_limit SET DEFAULT 999999;

COMMENT ON COLUMN workspaces.user_limit IS
    'Maximum members an admin can invite. Defaults to unlimited (999999). '
    'An owner may lower it from Settings > Billing as a spend ceiling; '
    'billing is per active user regardless.';
