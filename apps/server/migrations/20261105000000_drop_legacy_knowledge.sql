-- KYO-953: drop the legacy knowledge stores left unread by KYO-949/KYO-952.
--
-- IRREVERSIBLE ON DEPLOYED DATABASES. Forward-only drop migration; the baseline
-- migration is not edited. Merge requires Jason's explicit approval, and a
-- release containing KYO-949 and KYO-952 must be fully rolled out (no pre-949
-- pods remain): those releases removed the request-path reads and writes, but
-- this file removes the last reader (the daily catalog-scheduler
-- learning-reference backfill) and the rows it maintained, and a pre-949
-- replica still queries these columns.
--
-- KYO-952 recorded the disposition of the existing rows: zero rows had
-- `learning_type = 'metric'`, and the 7 non-metric `agent_learnings` rows (1
-- workspace) are not preserved — this migration drops them. The 9
-- `learning_references` rows are dropped with their table. Production
-- `users.knowledge` and `workspaces.business_knowledge` were empty (0 of 44 and
-- 0 of 38), so no text content is lost there. The coordinated disclosure is on
-- KYO-953.
--
-- These counts cover the production database only. The same migrations also run
-- on self-hosted Postgres and SQLite databases (and the desktop app), where the
-- counts do not apply; back up before upgrading.
--
-- Dropped objects: `workspaces.business_knowledge`,
-- `workspaces.knowledge_updated_at`, `users.knowledge`, `agent_learnings` (with
-- its primary key, indexes, self-FK, workspace/datasource FKs, the
-- `agent_learnings_search_vector_trigger` trigger and the
-- `agent_learnings_search_vector_update()` function), `learning_references`,
-- and the `learning_scope` enum type.
--
-- Drop order is FK-driven: `learning_references.learning_id` references
-- `agent_learnings.learning_id`, so that table goes first. Dropping
-- `agent_learnings` also drops its primary key, every index (including the HNSW
-- `idx_learnings_embedding` and GIN `idx_learnings_search_vector`), its self-FK
-- and its trigger. The function and the enum type are separate objects and are
-- dropped explicitly.
--
-- `conversation_discussed` has an unenforced `entity_type = 'learning'`
-- reference and no foreign key to `agent_learnings`; those rows dangle rather
-- than block the drop, and KYO-956 owns them.
--
-- Rollback is not possible: rolling back to a pre-953 binary after this
-- migration has applied fails at startup with `VersionMissing`, so recovery is
-- a forward fix or a database restore.

DROP TABLE IF EXISTS learning_references;

DROP TABLE IF EXISTS agent_learnings;
DROP FUNCTION IF EXISTS agent_learnings_search_vector_update();

-- The enum type exists only for `agent_learnings.scope`.
DROP TYPE IF EXISTS learning_scope;

ALTER TABLE workspaces
    DROP COLUMN IF EXISTS business_knowledge,
    DROP COLUMN IF EXISTS knowledge_updated_at;

ALTER TABLE users
    DROP COLUMN IF EXISTS knowledge;
