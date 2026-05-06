-- Convert Postgres UUID columns to TEXT for dual-database (Postgres + SQLite) compatibility.
-- SQLite stores UUIDs as TEXT strings; this migration makes Postgres match.
-- All data is preserved via `USING column_name::text`.

-- ============================================================================
-- 1. agent_learnings.learning_id (PK) + agent_learnings.superseded_by (self-FK)
--    Referenced by: learning_references.learning_id, agent_learnings.superseded_by
-- ============================================================================

-- Drop FKs that reference agent_learnings.learning_id
ALTER TABLE learning_references DROP CONSTRAINT IF EXISTS learning_references_learning_id_fkey;
ALTER TABLE agent_learnings DROP CONSTRAINT IF EXISTS agent_learnings_superseded_by_fkey;

-- Convert columns
ALTER TABLE agent_learnings
    ALTER COLUMN learning_id TYPE TEXT USING learning_id::text,
    ALTER COLUMN learning_id DROP DEFAULT;

ALTER TABLE agent_learnings
    ALTER COLUMN superseded_by TYPE TEXT USING superseded_by::text;

-- Convert the FK column in learning_references
ALTER TABLE learning_references
    ALTER COLUMN learning_id TYPE TEXT USING learning_id::text;

-- Re-add FKs with TEXT types
ALTER TABLE learning_references
    ADD CONSTRAINT learning_references_learning_id_fkey
    FOREIGN KEY (learning_id) REFERENCES agent_learnings(learning_id) ON DELETE CASCADE;

ALTER TABLE agent_learnings
    ADD CONSTRAINT agent_learnings_superseded_by_fkey
    FOREIGN KEY (superseded_by) REFERENCES agent_learnings(learning_id);

-- ============================================================================
-- 2. charts.chart_id (PK)
-- ============================================================================

ALTER TABLE charts
    ALTER COLUMN chart_id TYPE TEXT USING chart_id::text,
    ALTER COLUMN chart_id DROP DEFAULT;

-- ============================================================================
-- 3. collections.id (PK)
--    Referenced by: collection_dashboards.collection_id
-- ============================================================================

-- Drop FK first
ALTER TABLE collection_dashboards DROP CONSTRAINT IF EXISTS collection_dashboards_collection_id_fkey;

-- Convert columns
ALTER TABLE collections
    ALTER COLUMN id TYPE TEXT USING id::text,
    ALTER COLUMN id DROP DEFAULT;

ALTER TABLE collection_dashboards
    ALTER COLUMN collection_id TYPE TEXT USING collection_id::text;

-- Re-add FK
ALTER TABLE collection_dashboards
    ADD CONSTRAINT collection_dashboards_collection_id_fkey
    FOREIGN KEY (collection_id) REFERENCES collections(id) ON DELETE CASCADE;

-- ============================================================================
-- 4. oauth_clients.id (PK, no FKs reference it)
-- ============================================================================

ALTER TABLE oauth_clients
    ALTER COLUMN id TYPE TEXT USING id::text,
    ALTER COLUMN id DROP DEFAULT;

-- ============================================================================
-- 5. analytics_sites.id (PK, no FKs reference it)
-- ============================================================================

ALTER TABLE analytics_sites
    ALTER COLUMN id TYPE TEXT USING id::text,
    ALTER COLUMN id DROP DEFAULT;
