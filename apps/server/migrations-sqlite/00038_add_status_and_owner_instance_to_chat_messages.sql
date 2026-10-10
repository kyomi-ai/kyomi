-- Add `status` and `owner_instance` observability columns to chat_messages.
--
-- See the Postgres counterpart
-- (apps/server/migrations/20260923000000_add_status_and_owner_instance_to_chat_messages.sql)
-- for the full KYO-493 background — this file repeats only what differs for
-- SQLite: the CHECK constraint is expressed inline (SQLite has no separate
-- ADD CONSTRAINT for ALTER TABLE), and there is no COMMENT ON COLUMN
-- equivalent.
ALTER TABLE chat_messages ADD COLUMN status TEXT NOT NULL DEFAULT 'complete'
    CHECK (status IN ('in_progress', 'complete', 'error', 'cancelled', 'interrupted'));

ALTER TABLE chat_messages ADD COLUMN owner_instance TEXT;
