-- The native-publication admission fence was removed with the personal-data
-- tool restriction. Drop its column and table, and recreate the private-session
-- guards without it.
DROP TRIGGER IF EXISTS personal_mcp_private_session;
DROP TRIGGER IF EXISTS personal_mcp_private_session_insert;
DROP TABLE IF EXISTS personal_mcp_native_publications;
ALTER TABLE chat_sessions DROP COLUMN native_publications;
CREATE TRIGGER personal_mcp_private_session BEFORE UPDATE ON chat_sessions
WHEN NEW.personal_data_owner_id IS NOT NULL AND
 (NEW.personal_data_owner_id != NEW.user_id OR COALESCE(NEW.shared, 0) != 0)
BEGIN SELECT RAISE(ABORT, 'Personal external data must remain private'); END;
CREATE TRIGGER personal_mcp_private_session_insert BEFORE INSERT ON chat_sessions
WHEN NEW.personal_data_owner_id IS NOT NULL AND
 (NEW.personal_data_owner_id != NEW.user_id OR COALESCE(NEW.shared, 0) != 0)
BEGIN SELECT RAISE(ABORT, 'Personal external data must remain private'); END;
