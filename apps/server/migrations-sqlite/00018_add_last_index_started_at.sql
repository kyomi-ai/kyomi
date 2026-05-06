-- Add last_index_started_at to datasource_configs.
--
-- See migrations/20260414100000_add_last_index_started_at.sql for rationale.
-- SQLite stores `chrono::DateTime<Utc>` as TEXT (ISO 8601) via sqlx.
ALTER TABLE datasource_configs
    ADD COLUMN last_index_started_at TEXT NULL;
