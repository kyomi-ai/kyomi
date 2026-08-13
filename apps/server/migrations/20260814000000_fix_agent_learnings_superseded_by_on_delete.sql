-- KYO-346: restore ON DELETE SET NULL on agent_learnings_superseded_by_fkey.
--
-- The baseline (apps/server/migrations/20260215000000_baseline.sql:3302) had
-- this self-referential FK as `ON DELETE SET NULL`: deleting a superseding
-- `agent_learnings` row should null out `superseded_by` on the row(s) that
-- pointed at it, not block the delete.
--
-- 20260315000000_uuid_columns_to_text.sql dropped this constraint to convert
-- `superseded_by` from `uuid` to `text` and re-added it without the delete
-- action, so it silently defaulted to `NO ACTION`. That migration's sibling
-- FK (`learning_references_learning_id_fkey`, re-added two lines later in
-- the same file) correctly preserved its `ON DELETE CASCADE`, so this was an
-- incidental omission, not a deliberate change — the migration's purpose was
-- a type conversion, nothing about delete semantics. SQLite's baseline
-- (apps/server/migrations-sqlite/00001_baseline.sql:97) was never touched by
-- an equivalent migration and still has `ON DELETE SET NULL`, so Postgres
-- and SQLite have diverged since 20260315000000: deleting a superseding row
-- on Postgres now raises an FK violation instead of nulling the referencing
-- row, while SQLite behaves as designed.
--
-- `DROP CONSTRAINT IF EXISTS` + unconditional re-add makes this safe to run
-- against a database where the constraint is already missing (shouldn't
-- happen, but costs nothing to tolerate) and leaves the constraint in the
-- same state whether this migration runs once or is inspected after the
-- fact — there's no partial-application window since both statements run in
-- the same migration transaction.
ALTER TABLE agent_learnings DROP CONSTRAINT IF EXISTS agent_learnings_superseded_by_fkey;

ALTER TABLE agent_learnings
    ADD CONSTRAINT agent_learnings_superseded_by_fkey
    FOREIGN KEY (superseded_by) REFERENCES agent_learnings(learning_id) ON DELETE SET NULL;
