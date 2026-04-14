-- Add last_index_started_at to datasource_configs.
--
-- This column is stamped at the start of every catalog indexing run so that
-- the scheduler (and any other caller of CatalogIndexingService::index_datasource)
-- can skip a datasource that already has an indexing run in progress.
--
-- The check is "start stamp is within the last hour → skip", which makes the
-- guard self-healing: if a previous run panicked, the stamp ages out after 60
-- minutes and the next attempt proceeds. No "running" status to get stuck in.
--
-- Complements the existing `last_catalog_refresh` (finish timestamp, used by
-- the 24-hour recency rate limit in `can_refresh_now`). Both gates are needed:
-- the finish stamp guards against "just finished, don't re-index", the start
-- stamp guards against "just started, don't double up".
ALTER TABLE datasource_configs
    ADD COLUMN last_index_started_at TIMESTAMPTZ NULL;
