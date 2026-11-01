-- Permanent conversation confidentiality. No API can clear this marker.
ALTER TABLE chat_sessions ADD COLUMN personal_data_owner_id TEXT REFERENCES users(user_id) ON DELETE CASCADE;
ALTER TABLE chat_sessions ADD COLUMN personal_data_document_id TEXT;
ALTER TABLE chat_sessions ADD COLUMN native_publications BIGINT NOT NULL DEFAULT 0 CHECK (native_publications >= 0);
ALTER TABLE chat_sessions ADD CONSTRAINT personal_mcp_private_session CHECK
 (personal_data_owner_id IS NULL OR (personal_data_owner_id = user_id AND NOT COALESCE(shared, FALSE) AND native_publications = 0));
CREATE TABLE personal_mcp_private_documents (
 dashboard_id TEXT PRIMARY KEY NOT NULL REFERENCES dashboards(dashboard_id) ON DELETE CASCADE,
 owner_user_id TEXT NOT NULL REFERENCES users(user_id) ON DELETE CASCADE
);
-- The document row is the common serialization boundary for admission and visibility changes.
CREATE FUNCTION personal_mcp_collection_privacy() RETURNS TRIGGER LANGUAGE plpgsql AS $$
DECLARE doc TEXT;
BEGIN
 IF TG_TABLE_NAME = 'collection_dashboards' THEN
  PERFORM dashboard_id FROM dashboards WHERE dashboard_id = NEW.dashboard_id FOR UPDATE;
  IF EXISTS (SELECT 1 FROM personal_mcp_private_documents WHERE dashboard_id = NEW.dashboard_id) THEN
   RAISE EXCEPTION 'Personal external data cannot be added to a collection';
  END IF;
 ELSE
  IF NEW.is_public THEN
   FOR doc IN SELECT dashboard_id FROM collection_dashboards WHERE collection_id = NEW.id ORDER BY dashboard_id LOOP
    PERFORM dashboard_id FROM dashboards WHERE dashboard_id = doc FOR UPDATE;
    IF EXISTS (SELECT 1 FROM personal_mcp_private_documents WHERE dashboard_id = doc) THEN
     RAISE EXCEPTION 'Personal external data cannot be shared';
    END IF;
   END LOOP;
  END IF;
 END IF;
 RETURN NEW;
END $$;
CREATE TRIGGER personal_mcp_collection_add BEFORE INSERT OR UPDATE ON collection_dashboards FOR EACH ROW EXECUTE FUNCTION personal_mcp_collection_privacy();
CREATE TRIGGER personal_mcp_collection_share BEFORE UPDATE OF is_public ON collections FOR EACH ROW EXECUTE FUNCTION personal_mcp_collection_privacy();
CREATE FUNCTION personal_mcp_immutable_privacy() RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
 IF TG_TABLE_NAME = 'chat_sessions' THEN
  IF OLD.personal_data_owner_id IS NOT NULL AND (NEW.personal_data_owner_id IS DISTINCT FROM OLD.personal_data_owner_id OR NEW.personal_data_document_id IS DISTINCT FROM OLD.personal_data_document_id) THEN
   RAISE EXCEPTION 'Personal external data restriction is permanent';
  END IF;
 ELSE
  IF NEW.user_id IS DISTINCT FROM OLD.user_id AND EXISTS (SELECT 1 FROM personal_mcp_private_documents WHERE dashboard_id = OLD.dashboard_id) THEN
   RAISE EXCEPTION 'Personal external data ownership is permanent';
  END IF;
 END IF;
 RETURN NEW;
END $$;
CREATE TRIGGER personal_mcp_session_immutable BEFORE UPDATE ON chat_sessions FOR EACH ROW EXECUTE FUNCTION personal_mcp_immutable_privacy();
CREATE TRIGGER personal_mcp_document_owner BEFORE UPDATE OF user_id ON dashboards FOR EACH ROW EXECUTE FUNCTION personal_mcp_immutable_privacy();

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
