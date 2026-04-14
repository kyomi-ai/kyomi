-- Add doc_type to collections so they are scoped to either dashboards or knowledge.
-- Previously collections were type-agnostic and filtering relied on JOINing to
-- the dashboards table, which caused empty collections to disappear.

ALTER TABLE collections ADD COLUMN IF NOT EXISTS doc_type TEXT NOT NULL DEFAULT 'dashboard';

-- Backfill: set doc_type based on the majority doc_type of member documents.
-- Collections with only knowledge docs → 'knowledge', otherwise 'dashboard'.
UPDATE collections
SET doc_type = 'knowledge'
WHERE id IN (
    SELECT cd.collection_id
    FROM collection_dashboards cd
    JOIN dashboards d ON d.dashboard_id = cd.dashboard_id
    GROUP BY cd.collection_id
    HAVING COUNT(*) = COUNT(CASE WHEN d.doc_type = 'knowledge' THEN 1 END)
);

-- Index for fast filtering by workspace + doc_type
CREATE INDEX IF NOT EXISTS idx_collections_doc_type ON collections(workspace_id, doc_type);
