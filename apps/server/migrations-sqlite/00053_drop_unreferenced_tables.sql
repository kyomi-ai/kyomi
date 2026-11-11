-- KYO-956: drop the two unreferenced legacy tables.
--
-- IRREVERSIBLE ON DEPLOYED DATABASES. Forward-only; see the matching Postgres
-- migration (apps/server/migrations/20261106000000_drop_unreferenced_tables.sql)
-- for the rationale, rollout precondition and data disposition.
--
-- This chain also runs on self-hosted servers and the desktop app; the dropped
-- tables held legacy workspace knowledge chunks and episodic
-- conversation-to-entity references. Back up before upgrading.
DROP TABLE IF EXISTS conversation_discussed;
DROP TABLE IF EXISTS workspace_knowledge_chunks;
