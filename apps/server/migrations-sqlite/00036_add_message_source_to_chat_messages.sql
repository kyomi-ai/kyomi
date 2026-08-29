-- Add `message_source` observability column to chat_messages.
--
-- See the Postgres counterpart
-- (apps/server/migrations/20260829000000_add_message_source_to_chat_messages.sql)
-- for the full KYO-506 background — this file repeats only what differs for
-- SQLite: purely additive, nullable, no backfill (rows written before this
-- migration never had a source to record).
--
-- Matches `current_time_user_tz`'s `TEXT` type in 00001_baseline.sql.
ALTER TABLE chat_messages ADD COLUMN message_source TEXT;
