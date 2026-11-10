-- KYO-953: drop the legacy knowledge stores left unread by KYO-949/KYO-952.
--
-- IRREVERSIBLE ON DEPLOYED DATABASES. Forward-only; see the matching Postgres
-- migration (apps/server/migrations/20261105000000_drop_legacy_knowledge.sql)
-- for the full rationale, the KYO-952 row disposition, the data-loss counts and
-- the rollout precondition (a release containing KYO-949 and KYO-952 must be
-- deployed before this runs).
--
-- This same chain runs on self-hosted servers and the desktop app, not just
-- production Postgres: the counts disclosed on KYO-953 cover the production
-- database only. Back up a self-hosted/desktop database before upgrading.
--
-- SQLite spells the search implementation as separate objects: the
-- `agent_learnings_fts` FTS5 virtual table plus three sync triggers. They are
-- dropped explicitly, before the table that backs the external-content FTS5
-- index. Dropping the virtual table also removes its shadow tables
-- (`agent_learnings_fts_data`, `_idx`, `_docsize`, `_config`).
--
-- `ALTER TABLE ... DROP COLUMN` requires SQLite >= 3.35; this project bundles a
-- much newer libsqlite3-sys via sqlx, and 00032 already uses the same form.
-- None of the three dropped columns is referenced by an index or trigger, so no
-- dependent object has to be removed first.

DROP TRIGGER IF EXISTS agent_learnings_fts_insert;
DROP TRIGGER IF EXISTS agent_learnings_fts_delete;
DROP TRIGGER IF EXISTS agent_learnings_fts_update;

DROP TABLE IF EXISTS agent_learnings_fts;
DROP TABLE IF EXISTS learning_references;
DROP TABLE IF EXISTS agent_learnings;

ALTER TABLE workspaces DROP COLUMN business_knowledge;
ALTER TABLE workspaces DROP COLUMN knowledge_updated_at;
ALTER TABLE users DROP COLUMN knowledge;
