-- SPDX-License-Identifier: AGPL-3.0-or-later
--
-- KYO-622: container-liveness bookkeeping for the catalog-archive GC.
--
-- Background: `archive_missing_tables`'s KYO-614 fix (`is_archive_candidate`,
-- crates/kyomi-auth/src/catalog/helpers.rs) skips any cached row whose
-- `(project_id, dataset_id)` container was not enumerated THIS run — an
-- un-enumerated container is an unknown, not a confirmed absence, so its
-- rows must never be archived off one run's silence alone. That is correct
-- and must not be relaxed, but it has a permanent side effect nobody closed
-- the loop on: a container genuinely deleted at the source stops being
-- enumerated forever, so its rows never re-enter any run's scope set and
-- stay `is_archived = false` indefinitely. They also permanently inflate the
-- live-container denominator `check_container_coverage` counts against,
-- which can push an otherwise-healthy run's `status` toward `"failed"`.
--
-- This table is the evidence a *separate* mechanism
-- (`kyomi_auth::catalog::helpers::reconcile_container_liveness`) uses to
-- reclaim those rows safely, without weakening the KYO-614 invariant above.
-- The governing asymmetry is the same one KYO-614 was built against:
-- preserving a genuinely-deleted table costs one stale row; archiving
-- wrongly costs the customer their catalog with no error surfaced.
-- Reclaiming therefore needs its own, independent evidence — several
-- *complete* enumerations in a row that didn't see the container — never a
-- relaxation of the single-run scope check.
--
-- "Complete" is the load-bearing word. Neither `discover_all_containers`
-- (`kyomi-agent/src/catalog/traits.rs`, a flat `Result<Vec<String>>` with no
-- per-container success/failure channel) nor Connect's `CatalogResult`
-- (per-container failures folded into opaque `errors: Vec<String>` message
-- strings — a container that failed partway through discovery is simply
-- absent from the result) can say "enumeration succeeded for containers X
-- and Y but not Z". There is no sound way to attribute a partial failure to
-- a specific container, so the only evidence available is whole-run
-- completeness: every call site passes `run_complete = false` whenever its
-- own run produced ANY error, and a `run_complete = false` call is a strict
-- no-op in `reconcile_container_liveness` — not even a `missed_runs` counter
-- reset. A run that could not see a container can never advance it toward
-- deletion, no matter how many incomplete runs pass.
--
-- `last_seen_at` is stamped whenever a container appears in a complete
-- enumeration; `missed_runs` counts consecutive complete runs in which it
-- was absent, and resets to 0 the moment it reappears. Once a container's
-- `missed_runs` reaches `MISSED_COMPLETE_RUNS_BEFORE_ARCHIVE` (3 — see that
-- constant's doc comment in helpers.rs for the full reasoning, anchored to
-- KYO-619, the confirmed discovery-layer bug this threshold has to hold
-- margin against), its still-live `datasource_table_cache` rows are
-- archived and its liveness row deleted, so a later reappearance starts
-- clean rather than resuming a stale counter.
--
-- NO BACKFILL: an existing container has no observation history to
-- reconstruct — there is nothing to backfill it TO other than 0, which is
-- exactly where a first-seen container already starts. It must earn its way
-- to the threshold like any other container, over real subsequent runs, not
-- be seeded with an assumed miss count.
CREATE TABLE IF NOT EXISTS public.datasource_container_cache (
    id SERIAL PRIMARY KEY,
    workspace_id character varying(50) NOT NULL,
    datasource_config_id character varying(50) NOT NULL
        REFERENCES public.datasource_configs(id) ON DELETE CASCADE,
    project_id character varying(255) NOT NULL,
    dataset_id character varying(255) NOT NULL,
    last_seen_at timestamp with time zone,
    missed_runs BIGINT NOT NULL DEFAULT 0,
    created_at timestamp with time zone NOT NULL DEFAULT NOW(),
    updated_at timestamp with time zone NOT NULL DEFAULT NOW(),
    UNIQUE (workspace_id, datasource_config_id, project_id, dataset_id)
);

CREATE INDEX IF NOT EXISTS idx_datasource_container_cache_lookup
    ON public.datasource_container_cache (workspace_id, datasource_config_id);

COMMENT ON TABLE public.datasource_container_cache IS
    'KYO-622: per-container liveness bookkeeping backing the catalog-archive GC. Read/written by kyomi_auth::catalog::helpers::reconcile_container_liveness, called immediately before check_container_coverage from kyomi_agent::catalog::traits::index_catalog_sql, kyomi_agent::catalog::indexers::connect::process_discovered_catalog, and kyomi_auth::catalog::indexers::user_dataset::index_workspace_catalog. Reclaims datasource_table_cache rows whose container has been absent from MISSED_COMPLETE_RUNS_BEFORE_ARCHIVE consecutive *complete* enumerations -- never a partial one. No backfill: pre-existing containers start at missed_runs = 0, same as any newly-seen one.';
