-- Phase 12 Task 6: Drop legacy slack columns.
-- No-op for SQLite: slack columns were removed from the SQLite baseline
-- (00001_baseline.sql) since there are no existing SQLite deployments.
-- The Postgres migration (20260308103230_drop_legacy_slack_columns.sql)
-- handles the actual column drops for production databases.
SELECT 1;
