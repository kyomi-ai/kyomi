-- Workspace seat cap removed (KYO-167): member count is never capped — billing
-- is per active user. Lift all existing workspaces to unlimited.
-- (Self-hosted/SQLite deployments are the 'team' tier and never enforce a cap;
-- signup inserts user_limit explicitly, so the column DEFAULT is not relied on.)
UPDATE workspaces
SET user_limit = 999999
WHERE user_limit IS NULL OR user_limit < 999999;
