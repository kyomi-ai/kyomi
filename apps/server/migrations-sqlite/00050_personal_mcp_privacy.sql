ALTER TABLE chat_sessions ADD COLUMN personal_data_owner_id TEXT REFERENCES users(user_id) ON DELETE CASCADE;
ALTER TABLE chat_sessions ADD COLUMN personal_data_document_id TEXT;
ALTER TABLE chat_sessions ADD COLUMN native_publications BIGINT NOT NULL DEFAULT 0 CHECK (native_publications >= 0);
CREATE TABLE personal_mcp_private_documents (
 dashboard_id TEXT PRIMARY KEY NOT NULL REFERENCES dashboards(dashboard_id) ON DELETE CASCADE,
 owner_user_id TEXT NOT NULL REFERENCES users(user_id) ON DELETE CASCADE
);
-- SQLite serializes writers before evaluating these triggers.
CREATE TRIGGER personal_mcp_private_session BEFORE UPDATE ON chat_sessions
WHEN NEW.personal_data_owner_id IS NOT NULL AND
 (NEW.personal_data_owner_id != NEW.user_id OR COALESCE(NEW.shared, 0) != 0 OR NEW.native_publications != 0)
BEGIN SELECT RAISE(ABORT, 'Personal external data must remain private'); END;
CREATE TRIGGER personal_mcp_private_session_insert BEFORE INSERT ON chat_sessions
WHEN NEW.personal_data_owner_id IS NOT NULL AND
 (NEW.personal_data_owner_id != NEW.user_id OR COALESCE(NEW.shared, 0) != 0 OR NEW.native_publications != 0)
BEGIN SELECT RAISE(ABORT, 'Personal external data must remain private'); END;
CREATE TRIGGER personal_mcp_session_immutable BEFORE UPDATE ON chat_sessions
WHEN OLD.personal_data_owner_id IS NOT NULL AND (NEW.personal_data_owner_id IS NOT OLD.personal_data_owner_id OR NEW.personal_data_document_id IS NOT OLD.personal_data_document_id)
BEGIN SELECT RAISE(ABORT, 'Personal external data restriction is permanent'); END;
CREATE TRIGGER personal_mcp_collection_add BEFORE INSERT ON collection_dashboards
WHEN EXISTS (SELECT 1 FROM personal_mcp_private_documents WHERE dashboard_id = NEW.dashboard_id)
BEGIN SELECT RAISE(ABORT, 'Personal external data cannot be added to a collection'); END;
CREATE TRIGGER personal_mcp_collection_move BEFORE UPDATE ON collection_dashboards
WHEN EXISTS (SELECT 1 FROM personal_mcp_private_documents WHERE dashboard_id = NEW.dashboard_id)
BEGIN SELECT RAISE(ABORT, 'Personal external data cannot be added to a collection'); END;
CREATE TRIGGER personal_mcp_collection_share BEFORE UPDATE OF is_public ON collections
WHEN NEW.is_public AND EXISTS (SELECT 1 FROM collection_dashboards cd JOIN personal_mcp_private_documents p ON p.dashboard_id = cd.dashboard_id WHERE cd.collection_id = NEW.id)
BEGIN SELECT RAISE(ABORT, 'Personal external data cannot be shared'); END;
CREATE TRIGGER personal_mcp_document_owner BEFORE UPDATE OF user_id ON dashboards
WHEN NEW.user_id != OLD.user_id AND EXISTS (SELECT 1 FROM personal_mcp_private_documents WHERE dashboard_id = OLD.dashboard_id)
BEGIN SELECT RAISE(ABORT, 'Personal external data ownership is permanent'); END;

CREATE TABLE personal_mcp_native_publications (
 publication_id TEXT PRIMARY KEY NOT NULL,
 session_id TEXT NOT NULL REFERENCES chat_sessions(session_id) ON DELETE CASCADE,
 actor_id TEXT NOT NULL REFERENCES users(user_id) ON DELETE CASCADE
);
CREATE INDEX personal_mcp_native_session ON personal_mcp_native_publications(session_id);

-- Keep inheritance evidence after document deletion, including reads already in flight.
CREATE TABLE personal_mcp_private_contexts (
 owner_user_id TEXT NOT NULL REFERENCES users(user_id) ON DELETE CASCADE,
 workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
 PRIMARY KEY (owner_user_id, workspace_id)
);
