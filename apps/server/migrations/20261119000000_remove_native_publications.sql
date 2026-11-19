-- The native-publication admission fence was removed with the personal-data
-- tool restriction. Drop its column and table, and recreate the private-session
-- guard without it.
ALTER TABLE chat_sessions DROP CONSTRAINT IF EXISTS personal_mcp_private_session;
DROP TABLE IF EXISTS personal_mcp_native_publications;
ALTER TABLE chat_sessions DROP COLUMN IF EXISTS native_publications;
ALTER TABLE chat_sessions ADD CONSTRAINT personal_mcp_private_session CHECK
 (personal_data_owner_id IS NULL OR (personal_data_owner_id = user_id AND NOT COALESCE(shared, FALSE)));
