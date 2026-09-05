// SPDX-License-Identifier: AGPL-3.0-or-later

//! Catalog shared helpers — caching, archiving, status updates.
//!
//! These are the `kyomi_datasource`-independent helpers used by both SQL-based
//! indexers and special indexers (sample data, user dataset, BigQuery public).
//!
//! The `SQLCatalogIndexer` trait and `index_catalog_sql()` template method
//! live in `traits.rs` and depend on `kyomi_datasource` (compiled from `kyomi-api`).

use chrono::Utc;
use kyomi_core::{DbPool, Result};
use kyomi_embed::EmbeddingService;
use pgvector::Vector;
use serde_json::Value;
use std::collections::HashSet;
use tracing::{debug, info, warn};

use super::search_entries::{compute_schema_signature, create_search_entries};
use super::types::ColumnEntry;

use crate::embedding_persistence::{
    delete_embeddings_for_table, store_search_embeddings, SearchEntryInsert,
};

// ─── IndexerContext ────────────────────────────────────────────────────────────

/// Shared context passed to all indexing operations.
///
/// Contains the workspace/datasource identifiers, connection configuration,
/// and encryption key needed by shared helper functions.
#[derive(Clone)]
pub struct IndexerContext {
    /// Workspace ID this indexing run belongs to.
    pub workspace_id: String,
    /// Datasource config ID being indexed.
    pub datasource_config_id: String,
    /// Connection configuration from the datasource config row.
    pub connection_config: Value,
    /// Encryption key for decrypting stored credentials.
    pub encryption_key: std::sync::Arc<[u8; 32]>,
}

// ─── Shared helpers ────────────────────────────────────────────────────────────

/// Look up the workspace owner's email address.
///
/// Used by catalog indexing paths that need a "default user" to resolve stored
/// credentials against. The workspace owner is the creator/admin who is most
/// likely to have valid datasource credentials stored.
///
/// Returns `None` if the workspace doesn't exist or the owner can't be resolved.
pub async fn get_workspace_owner_email(db: &DbPool, workspace_id: &str) -> Option<String> {
    #[derive(sqlx::FromRow)]
    struct EmailRow {
        email: String,
    }

    let row = kyomi_core::db_fetch_optional!(
        db,
        EmailRow,
        "SELECT u.email \
         FROM workspaces w \
         JOIN users u ON u.user_id = w.owner_user_id \
         WHERE w.workspace_id = $1",
        workspace_id
    )
    .ok()
    .flatten();

    row.map(|r| r.email)
}

/// Check if a datasource can be refreshed now (respects rate limit).
///
/// Returns `true` if the datasource has never been refreshed, or if more
/// than `hours_threshold` hours have passed since the last refresh.
pub async fn can_refresh_now(
    db: &DbPool,
    datasource_config_id: &str,
    hours_threshold: i64,
) -> bool {
    #[derive(sqlx::FromRow)]
    struct RefreshRow {
        last_catalog_refresh: Option<chrono::DateTime<Utc>>,
    }

    let row = kyomi_core::db_fetch_optional!(
        db,
        RefreshRow,
        "SELECT last_catalog_refresh FROM datasource_configs WHERE id = $1",
        datasource_config_id
    );

    let Ok(Some(row)) = row else {
        return true; // datasource not found or error → allow refresh
    };

    match row.last_catalog_refresh {
        None => true, // never refreshed
        Some(ts) => {
            let elapsed = Utc::now() - ts;
            elapsed.num_hours() >= hours_threshold
        }
    }
}

/// A `(project_id, dataset_id)` pair identifying one container in
/// `datasource_table_cache` — its unique key is
/// `(workspace_id, datasource_config_id, project_id, dataset_id, table_id)`,
/// and `dataset_id` alone is not enough to identify a container: BigQuery
/// can index several GCP projects in one datasource
/// (`catalog_projects`/`ConfiguredProjectScope::Explicit`,
/// `kyomi_agent::catalog::indexers::bigquery`), and two of them sharing a
/// dataset name (e.g. `acme-dev.analytics` / `acme-prod.analytics`, a
/// routine dev/prod convention) is not an edge case. A bare `dataset_id`
/// key would let one project's `analytics` enumeration authorise archiving
/// another project's same-named, never-looked-at `analytics` — the exact
/// KYO-614 defect, arriving on the project axis instead of the dataset
/// axis. A tuple (rather than a delimiter-joined `"{project_id}.{dataset_id}"`
/// string) is deliberate: a joined string reintroduces exactly this class
/// of ambiguity the moment either half contains the delimiter.
///
/// For the SQL-template and Connect paths `project_id` is a constant for
/// the whole run (`indexer.get_project_id(ctx)` and the literal `""`
/// respectively), so this is a behavioural no-op there — the point is one
/// shared key shape, not a provider fork.
pub type ContainerKey = (String, String);

/// Which cached rows a run is authorised to archive.
///
/// A catalog index run only ever *enumerates* part of a datasource's
/// containers — a subset of schemas/datasets/catalogs, chosen by discovery
/// or by user configuration. Before KYO-614, [`archive_missing_tables`]
/// compared `seen_table_ids` against **every** non-archived row for the
/// datasource, with no container filter at all: a run that enumerated one
/// dataset out of four archived the other three datasets' tables in full,
/// because "not seen this run" was indistinguishable from "no longer
/// exists" for any container the run never looked at. An un-enumerated
/// container is an unknown, not a confirmed absence — the asymmetry is
/// decisive (preserving a genuinely-deleted table costs one stale row;
/// archiving wrongly costs the customer their catalog with no error
/// surfaced), so this type makes "which containers may this run touch at
/// all" an explicit, required input rather than an implicit "all of them".
#[derive(Debug)]
pub enum ArchiveScope {
    /// The user configured an empty container selection: the intended scope
    /// is *nothing*, so every cached row is out of scope and may be
    /// archived. This is the ONLY variant that may sweep the whole
    /// datasource, and it must only be constructed from a deliberate user
    /// action (KYO-162 / KYO-385's `empty_scope_is_expected`) — never from
    /// discovery merely coming back empty (e.g. `discover_all_containers`
    /// returning zero containers, or every configured container turning out
    /// invalid this run). Those are enumeration shortfalls, not user intent,
    /// and must resolve to `Containers` with whatever (possibly empty) set
    /// was actually enumerated — archiving nothing when nothing was
    /// enumerated, per the "when in doubt, preserve" invariant.
    EntireDatasource,
    /// Only these `(project_id, dataset_id)` pairs — see [`ContainerKey`] —
    /// were completely enumerated this run. A cached row whose
    /// `(project_id, dataset_id)` is not in this set is skipped before the
    /// `seen_table_ids` check even runs: it was never looked at this run,
    /// so "not seen" says nothing about whether it still exists (KYO-614).
    /// An empty set is valid and means "archive nothing" — a run that
    /// enumerated zero containers (as opposed to a user who configured
    /// zero) must not touch the existing catalog at all.
    Containers(HashSet<ContainerKey>),
}

/// Whether one cached row is a candidate for archiving under `scope` and
/// `seen_table_ids`.
///
/// Shared between the KYO-616 read-only pre-pass in [`archive_missing_tables`]
/// that logs the archive decision and the loop right below it that actually
/// performs the archiving, so the two can never independently drift onto a
/// different answer for the same row — see
/// `docs/standards/code-organization/propagate-predicate-changes-to-every-copy.md`.
fn is_archive_candidate(
    scope: &ArchiveScope,
    seen_table_ids: &HashSet<String>,
    project_id: &str,
    dataset_id: &str,
    table_id: &str,
) -> bool {
    let in_scope = match scope {
        ArchiveScope::EntireDatasource => true,
        ArchiveScope::Containers(containers) => {
            containers.contains(&(project_id.to_string(), dataset_id.to_string()))
        }
    };
    if !in_scope {
        return false;
    }

    let full_id = kyomi_core::build_full_table_name(project_id, dataset_id, table_id);
    !seen_table_ids.contains(&full_id)
}

/// KYO-616: ratio threshold for the "disproportionate archive" WARN in
/// [`archive_missing_tables`] — a run whose archived count is at least this
/// many times its indexed count is flagged as worth a human's attention.
/// Anchored to the confirmed production incident this ticket exists to
/// prevent: 34 tables archived against 2 indexed is a 17x ratio. `5` is
/// deliberately well below that, with headroom so an ordinary partial
/// refresh — a container that legitimately shed a handful more tables than
/// this run happened to (re-)index, e.g. 3 archived against 1 indexed —
/// does not cry wolf on every routine run.
///
/// PURELY OBSERVATIONAL. This constant gates a log line only — it MUST
/// NEVER be used to gate archiving, status, or any other control-flow
/// decision. The one and only decision threshold for "did this run look at
/// enough of the datasource" is [`is_material_shortfall`]
/// (`check_container_coverage`/`apply_container_coverage`); reusing this
/// value as a second decision threshold is exactly the two-thresholds-that-
/// can-silently-disagree anti-pattern KYO-616 was explicitly told to avoid.
const DISPROPORTIONATE_ARCHIVE_RATIO: usize = 5;

/// Whether this run's about-to-archive count is disproportionate to what it
/// indexed — the KYO-616 "disproportionate archive" WARN trigger. See
/// [`DISPROPORTIONATE_ARCHIVE_RATIO`]'s doc comment: purely observational,
/// never a decision input.
///
/// `tables_indexed == 0` with anything archived is treated as always
/// disproportionate (rather than skipped as "undefined ratio", and rather
/// than divided by zero) — a run that indexed nothing yet still archived
/// rows, even a deliberate [`ArchiveScope::EntireDatasource`] wipe, is
/// exactly the shape worth a human's glance rather than silence.
fn is_disproportionate_archive(tables_indexed: usize, archived_count: usize) -> bool {
    if archived_count == 0 {
        return false;
    }
    if tables_indexed == 0 {
        return true;
    }
    archived_count >= tables_indexed.saturating_mul(DISPROPORTIONATE_ARCHIVE_RATIO)
}

/// Archive tables that were not seen during the current refresh cycle.
///
/// Marks tables as `is_archived = true` in the cache. Returns the full_name
/// strings of archived tables (format: `project_id.dataset_id.table_id`) so
/// callers can forward them to graph cleanup.
///
/// `scope` (KYO-614) gates which cached rows are even eligible for
/// archiving, before `seen_table_ids` is ever consulted — see
/// [`ArchiveScope`]. This is deliberately a Rust-side filter over the rows
/// this function already fetches and iterates, rather than a dynamic SQL
/// `IN (...)` list: the number of containers is caller-controlled and
/// unbounded, and a hand-built `IN` list would need to behave identically
/// on both the Postgres and SQLite backends this function runs against.
///
/// KYO-616: before any row is mutated, this logs exactly what is about to
/// happen — the run that destroyed most of a production catalog left only
/// `tables_indexed:2 tables_archived:34 errors:0`, reconstructable
/// afterward only by diffing the database. The log line below is built from
/// a read-only pass over `rows` using the *same* [`is_archive_candidate`]
/// predicate the mutating loop uses, so it can never claim a different
/// decision than the one that actually runs.
pub async fn archive_missing_tables(
    db: &DbPool,
    workspace_id: &str,
    datasource_config_id: &str,
    scope: &ArchiveScope,
    seen_table_ids: &HashSet<String>,
    tables_indexed: usize,
) -> Result<Vec<String>> {
    // Callers are responsible for only calling this when discovery succeeded.
    // An empty seen_table_ids with a successful discovery means the datasource
    // genuinely has no tables — archiving everything is correct in that case
    // (subject to `scope`, which still gates which containers were actually
    // discovered this run).

    #[derive(sqlx::FromRow)]
    struct CacheRow {
        id: i32,
        project_id: String,
        dataset_id: String,
        table_id: String,
    }

    // Fetch all non-archived tables for this datasource
    let rows = kyomi_core::db_fetch_all!(
        db,
        CacheRow,
        r#"
        SELECT id, project_id, dataset_id, table_id
        FROM datasource_table_cache
        WHERE workspace_id = $1
          AND datasource_config_id = $2
          AND is_archived = false
        "#,
        workspace_id,
        datasource_config_id
    )
    .map_err(|e| kyomi_core::Error::Internal(format!("failed to fetch cached tables: {e}")))?;

    let is_pg = db.is_postgres();
    let now_expr = kyomi_core::sql_compat::now(is_pg);

    // KYO-616: the archive decision, before anything is written. See the
    // function doc comment — this is the read-only twin of the mutating
    // loop below, sharing `is_archive_candidate` so the two can't disagree.
    let to_archive: Vec<&CacheRow> = rows
        .iter()
        .filter(|row| {
            is_archive_candidate(
                scope,
                seen_table_ids,
                &row.project_id,
                &row.dataset_id,
                &row.table_id,
            )
        })
        .collect();

    let about_to_archive_containers: HashSet<ContainerKey> = to_archive
        .iter()
        .map(|row| (row.project_id.clone(), row.dataset_id.clone()))
        .collect();

    match scope {
        ArchiveScope::EntireDatasource => {
            info!(
                workspace_id,
                datasource_config_id,
                archive_scope = "entire_datasource",
                seen_count = seen_table_ids.len(),
                candidates_about_to_archive = to_archive.len(),
                about_to_archive_containers = about_to_archive_containers.len(),
                about_to_archive_container_names =
                    %format_container_keys_capped(&about_to_archive_containers),
                "evaluating archive scope before applying destructive changes"
            );
        }
        ArchiveScope::Containers(enumerated) => {
            info!(
                workspace_id,
                datasource_config_id,
                archive_scope = "containers",
                enumerated_containers = enumerated.len(),
                enumerated_container_names = %format_container_keys_capped(enumerated),
                seen_count = seen_table_ids.len(),
                candidates_about_to_archive = to_archive.len(),
                about_to_archive_containers = about_to_archive_containers.len(),
                about_to_archive_container_names =
                    %format_container_keys_capped(&about_to_archive_containers),
                "evaluating archive scope before applying destructive changes"
            );
        }
    }

    // KYO-616: purely observational — see `DISPROPORTIONATE_ARCHIVE_RATIO`'s
    // doc comment. Sits alongside the archive-decision log above rather than
    // replacing it: that log answers "what is this run about to do and to
    // which containers"; this WARN answers a narrower question — "is the
    // archive count wildly out of proportion to what this run indexed" —
    // and can fire (or not) independently of the coverage-shortfall WARN in
    // `check_container_coverage`, which answers a third, different question
    // ("did this run look at enough of the datasource at all"). A run can
    // trip either WARN alone: full container coverage with a disproportionate
    // archive trips only this one; partial coverage with a small, proportionate
    // archive trips only the other.
    if is_disproportionate_archive(tables_indexed, to_archive.len()) {
        let archive_to_indexed_ratio = if tables_indexed == 0 {
            "undefined (zero indexed)".to_string()
        } else {
            format!("{:.1}x", to_archive.len() as f64 / tables_indexed as f64)
        };
        warn!(
            workspace_id,
            datasource_config_id,
            tables_indexed,
            candidates_about_to_archive = to_archive.len(),
            archive_to_indexed_ratio = %archive_to_indexed_ratio,
            "this run is about to archive a disproportionately large share of the \
             catalog relative to what it indexed this run — worth a human's \
             attention even though the archive scope/coverage checks did not \
             block it"
        );
    }

    let mut archived_names = Vec::with_capacity(to_archive.len());
    for row in &rows {
        // KYO-614: a row whose container was never enumerated this run is
        // an unknown, not a confirmed absence — skip it before even
        // consulting `seen_table_ids`. Same predicate the log line above
        // was computed from.
        if !is_archive_candidate(
            scope,
            seen_table_ids,
            &row.project_id,
            &row.dataset_id,
            &row.table_id,
        ) {
            continue;
        }

        let full_id = kyomi_core::build_full_table_name(&row.project_id, &row.dataset_id, &row.table_id);
        let sql = format!(
            "UPDATE datasource_table_cache SET is_archived = true, updated_at = {now_expr} WHERE id = $1"
        );
        kyomi_core::db_execute!(db, &sql, row.id)
            .map_err(|e| {
                kyomi_core::Error::Internal(format!("failed to archive table {full_id}: {e}"))
            })?;
        archived_names.push(full_id);
    }

    if !archived_names.is_empty() {
        info!(
            workspace_id,
            datasource_config_id,
            archived_count = archived_names.len(),
            "archived tables no longer present in datasource"
        );
    }

    Ok(archived_names)
}

/// Update a datasource's catalog refresh status.
///
/// Sets the `catalog_refresh_status` VARCHAR column on `datasource_configs`,
/// scoped to `datasource_config_id`.
///
/// The column is VARCHAR(50) with values: 'idle', 'running', 'failed'.
/// Progress details are stored in `catalog_refresh_progress` (json column).
///
/// Filters on `workspace_id` in addition to `datasource_config_id` — that is
/// a tenant-isolation boundary (a caller must not be able to update another
/// workspace's datasource by id alone) and must not be dropped just because
/// `datasource_config_id` is already globally unique.
///
/// `error` is a human-readable failure reason, set whenever `status` is
/// `"failed"` and a specific cause is known (KYO-126). It is written as a
/// top-level `"error"` key in the stored envelope — a sibling of
/// `"progress"`, not nested inside it — because `get_catalog_refresh_status`
/// (`kyomi-ui/src/server_fns/sql_editor.rs`) and the settings page's refresh
/// poller already read `envelope.get("error")` directly on the whole
/// `catalog_refresh_progress` column value. Before this parameter existed,
/// every caller passed `None` here implicitly (there was no such field), so
/// that lookup always missed and the poller fell back to a generic message
/// even when a concrete failure reason was available.
///
/// `warnings` (KYO-327) is the run's collected per-container/per-table
/// discovery errors, written as a top-level `"warnings"` array — always
/// present, even when empty, so readers never need null-handling. Unlike
/// `error`, which only applies to a hard `"failed"` outcome, `warnings` is
/// meaningful on `"idle"` too: `resolve_final_status` folds a partial run
/// (some tables found, some containers denied) down to `"idle"` and
/// discards the individual error strings in that decision — this is the one
/// place they survive to be shown to the user. Callers doing an
/// intermediate/progress write (status `"running"`, or an early exit that
/// isn't the run's resolved final status) pass `&[]`; only the three
/// terminal call sites that already call `resolve_final_status`
/// (`kyomi_agent::catalog::traits::index_catalog_sql`,
/// `kyomi_agent::catalog::indexers::connect::process_discovered_catalog`,
/// `UserDatasetIndexer::index_workspace_catalog`) pass the run's real error
/// slice, on both the `"idle"` and `"failed"` outcomes it can resolve to.
///
/// KYO-267: this was previously `update_workspace_status`, writing to
/// `workspaces.catalog_refresh_status`/`catalog_refresh_progress` — columns
/// shared by every datasource in the workspace. Two different datasources
/// refreshing concurrently (`index_started_within` below keys off
/// `datasource_config_id`, not `workspace_id`, so this was always possible)
/// meant one datasource's successful `"idle"` write could silently
/// overwrite another's `"failed"` + reason, with no history of the failure
/// ever having happened. Moving both columns onto `datasource_configs`
/// removes the shared-state entirely — each datasource now owns its own
/// status/reason pair.
pub async fn update_datasource_status(
    db: &DbPool,
    workspace_id: &str,
    datasource_config_id: &str,
    status: &str,
    progress: Option<Value>,
    error: Option<&str>,
    warnings: &[String],
) -> Result<()> {
    let progress_json =
        build_progress_envelope(datasource_config_id, progress.as_ref(), error, warnings);

    let is_pg = db.is_postgres();
    let json_cast = if is_pg { "::json" } else { "" };
    let sql = format!(
        "UPDATE datasource_configs SET catalog_refresh_status = $1, catalog_refresh_progress = $2{json_cast} WHERE id = $3 AND workspace_id = $4"
    );

    kyomi_core::db_execute!(db, &sql, status, progress_json, datasource_config_id, workspace_id)
        .map_err(|e| {
            kyomi_core::Error::Internal(format!("failed to update datasource status: {e}"))
        })?;

    Ok(())
}

/// Build the JSON envelope stored in `datasource_configs.catalog_refresh_progress`.
///
/// Extracted from [`update_datasource_status`] so the shape — in particular,
/// `"error"` living as a top-level sibling of `"progress"` rather than
/// nested inside it — is directly testable. This is the exact shape
/// `get_catalog_refresh_status` and the settings page's refresh poller read
/// (KYO-126): both call `envelope.get("error")` on the whole column value.
///
/// The `"datasource_config_id"` key is redundant now that the envelope
/// lives on the datasource's own row (KYO-267) rather than a shared
/// workspace column — a reader no longer needs it to know which datasource
/// a status belongs to. It is kept purely as informational/debugging
/// context (costs nothing, and removing it risked breaking a reader that
/// wasn't checked), not because anything still depends on it for
/// attribution.
///
/// `"warnings"` (KYO-327) is always a JSON array, never null — a top-level
/// sibling of `"progress"`/`"error"`, same reasoning as `"error"` above.
/// Readers must never parse `"error"`'s collapsed `"<first> (+N more
/// errors)"` string for this; that string's wording is deliberately opaque
/// (see `CatalogIndexResult::errors`'s doc comment) and exists only for a
/// human reading the hard-failure reason, not for driving UI.
fn build_progress_envelope(
    datasource_config_id: &str,
    progress: Option<&Value>,
    error: Option<&str>,
    warnings: &[String],
) -> Value {
    serde_json::json!({
        "datasource_config_id": datasource_config_id,
        "updated_at": Utc::now().to_rfc3339(),
        "progress": progress,
        "error": error,
        "warnings": warnings,
    })
}

/// Determine the final `datasource_configs.catalog_refresh_status` value and
/// (when applicable) a failure reason, from the outcome of a catalog
/// indexing run.
///
/// Shared by both the SQL-based indexing template method
/// (`kyomi_agent::catalog::traits::index_catalog_sql`) and the BigQuery
/// user-dataset (REST) indexer (`kyomi_auth::catalog::indexers::user_dataset`)
/// — both fold their own container/dataset-shaped discovery loop down to the
/// same two inputs before calling this.
///
/// KYO-126: before this function existed, `index_catalog_sql` unconditionally
/// wrote `"idle"` at the end of every run — including one where every
/// container's discovery query failed (e.g. the role lacks permission to
/// read the catalog) and zero tables were found. That made a total discovery
/// failure indistinguishable from a healthy, empty datasource. KYO-264 found
/// and fixed the same bug on the BigQuery REST path (`UserDatasetIndexer`),
/// which had the identical unconditional-`"idle"` write plus a second layer
/// of the bug: per-dataset failures were only `warn!`-logged and dropped
/// before ever reaching an `errors` vec, so simply routing
/// `nothing_found`/`errors` through this function was not enough on its own
/// — the per-dataset errors first had to be propagated up to the caller (see
/// `fold_dataset_outcomes` in `catalog/indexers/user_dataset.rs`).
///
/// This function draws the line at whether any discovery error was observed:
/// - `nothing_found` + at least one error → the zero tables are *caused by*
///   a real failure: report `"failed"` with a reason built from the
///   collected errors.
/// - `nothing_found` with no errors → every container/dataset query
///   genuinely succeeded and simply returned no tables (or the user
///   configured zero containers) — this is not a failure and must keep
///   reporting `"idle"`, or a legitimately empty-but-accessible schema would
///   wrongly show as a broken datasource.
/// - not `nothing_found` → normal completion, `"idle"`, regardless of
///   whether some individual tables/containers/datasets errored along the
///   way (partial success is still success; those errors are already
///   surfaced via `CatalogIndexResult::errors`).
pub fn resolve_final_status(nothing_found: bool, errors: &[String]) -> (&'static str, Option<String>) {
    if !nothing_found || errors.is_empty() {
        return ("idle", None);
    }

    let reason = match errors.len() {
        1 => errors[0].clone(),
        n => format!("{} (+{} more error{})", errors[0], n - 1, if n == 2 { "" } else { "s" }),
    };
    ("failed", Some(reason))
}

/// The two decisions every catalog indexer needs once discovery and caching
/// for a run are complete: whether `archive_missing_tables` should run at
/// all, and what final status this run resolves to.
///
/// Ported from `kyomi_auth::catalog::indexers::user_dataset::resolve_run_outcome`
/// (KYO-324) into this shared module for KYO-385, once a third and fourth
/// hand-rolled copy of the same decision (`kyomi-agent`'s SQL-indexer
/// template `index_catalog_sql` and its Connect indexer) would otherwise
/// have been written — see `docs/CODING_STANDARDS.md`, "the third copy of a
/// test helper is the extraction trigger": the same reasoning applies to a
/// production decision. `resolve_final_status` already crosses this exact
/// `kyomi-auth` → `kyomi-agent` boundary, so this is not a new dependency
/// shape.
pub struct RunOutcome {
    /// Whether `archive_missing_tables` should run at all.
    pub archive: bool,
    /// Value to write to `datasource_configs.catalog_refresh_status`.
    pub status: &'static str,
    /// Failure reason to persist alongside a `"failed"` status.
    pub failure_reason: Option<String>,
}

/// Resolve the archive/status decision for one indexing run.
///
/// `seen_any_table` answers "did discovery (listing) see anything?" — a
/// table that was listed but never successfully cached (a schema-read
/// denial, or a `cache_table` write failure) still counts, because it
/// demonstrably exists and must not be evicted by `archive_missing_tables`
/// (the KYO-324 invariant: archiving keys off *listed*, not *usable*).
///
/// `tables_indexed` / `errors` answer "did we get anything *usable*?" —
/// this is the question `status` must answer instead. Conflating the two
/// into one predicate is the exact KYO-324/KYO-364/KYO-385 family of
/// regressions: every caller of this function populates its "seen" set
/// (`seen_table_ids` or equivalent) *before* attempting to cache a table, so
/// a run where every table was listed and read fine but every `cache_table`
/// write failed has `seen_any_table == true` — a predicate built only from
/// `seen_any_table` reports `"idle"` no matter how many `errors`
/// accumulated. See `resolve_final_status` for how `tables_indexed == 0`
/// and `errors` combine into an actual status.
///
/// `empty_scope_is_expected` is the one input this function needs beyond
/// `user_dataset.rs`'s original two-question split (KYO-385). The SQL and
/// Connect indexers both have a configuration state where an empty result
/// is intentional — zero containers configured (SQL path), or an explicitly
/// cleared container selection (Connect path) — and that state must be
/// treated differently from either "nothing was listed" or "nothing was
/// usable": the existing catalog should be archived (the user intentionally
/// emptied the scope, so stale cached tables are genuinely stale), and the
/// run must not be reported as `"failed"` (there is nothing to fail at;
/// discovery was never attempted). When `true`, this overrides both
/// `seen_any_table` and `tables_indexed` for their respective decisions.
/// `user_dataset.rs` has no equivalent state — an empty `project_ids`
/// returns `CatalogIndexResult::skipped(..)` before ever reaching this
/// function — so it always passes `false`.
pub fn resolve_run_outcome(
    seen_any_table: bool,
    tables_indexed: usize,
    errors: &[String],
    empty_scope_is_expected: bool,
) -> RunOutcome {
    let nothing_listed = !seen_any_table && !empty_scope_is_expected;
    let nothing_usable = tables_indexed == 0 && !empty_scope_is_expected;

    let (status, failure_reason) = resolve_final_status(nothing_usable, errors);

    RunOutcome {
        archive: !nothing_listed,
        status,
        failure_reason,
    }
}

// ─── Container coverage (KYO-614) ──────────────────────────────────────────────

/// Cap on how many un-enumerated container names [`check_container_coverage`]
/// lists individually in its warning, mirroring
/// `MAX_TABLE_ERRORS_PER_DATASET`'s summarize-the-rest idiom above — a
/// datasource with dozens of un-enumerated containers must not grow the
/// persisted `warnings` array to dozens of near-identical lines.
pub const MAX_MISSING_CONTAINERS_IN_WARNING: usize = 5;

/// The outcome of comparing what a run enumerated against what the
/// datasource's cache currently believes is live.
pub struct ContainerCoverage {
    /// A single warning entry naming the un-enumerated containers (capped at
    /// [`MAX_MISSING_CONTAINERS_IN_WARNING`]), to append to the run's
    /// `errors`/persisted `warnings`. `None` when every currently-live
    /// container was enumerated this run (including the common case where
    /// there is no prior cache to compare against at all).
    pub warning: Option<String>,
    /// Whether the shortfall is large enough that the run must not resolve
    /// to `"idle"` — see [`is_material_shortfall`].
    pub material: bool,
}

/// Whether an enumerated-vs-live container shortfall is "material" —
/// large enough that reporting the run as `"idle"` would misrepresent how
/// much of the datasource was actually looked at.
///
/// Proportional, not absolute: `enumerated_count * 2 <= live_count`, i.e.
/// this run enumerated *at most half* of the datasource's currently-live
/// containers. Chosen because it satisfies both ends of the requirement
/// this check exists for — it fires on the confirmed production incident
/// (1 of 4 datasets enumerated: `1 * 2 = 2 <= 4`, material) without
/// flapping on the ordinary case of a single genuinely-deleted dataset out
/// of ten (`9 * 2 = 18 > 10`, not material). An absolute threshold (e.g.
/// "more than 2 missing") would either miss the four-dataset incident or
/// misfire on datasources with dozens of containers where losing a handful
/// to natural churn is unremarkable.
fn is_material_shortfall(enumerated_count: usize, live_count: usize) -> bool {
    enumerated_count.saturating_mul(2) <= live_count
}

/// Compare the containers a run enumerated against the containers the
/// datasource's cache currently believes are live, and report whether the
/// run fell materially short of full coverage.
///
/// This is deliberately independent of [`ArchiveScope`]: it is a coverage
/// check on the run's *outcome* (did we look at enough of the datasource to
/// trust a clean status?), not an input to what [`archive_missing_tables`]
/// is allowed to touch. Callers should skip this check entirely when the
/// run used [`ArchiveScope::EntireDatasource`] — a deliberately emptied
/// selection is expected to leave every previously-live container
/// "un-enumerated" by design, and warning about that would be noise, not
/// signal.
///
/// On a first-ever run (or any datasource with nothing cached yet) the live
/// count is zero, so there is nothing to fall short of — this always
/// reports no shortfall in that case, matching `resolve_final_status`'s
/// existing "empty-but-accessible is not a failure" doctrine.
pub async fn check_container_coverage(
    db: &DbPool,
    workspace_id: &str,
    datasource_config_id: &str,
    enumerated_containers: &HashSet<ContainerKey>,
) -> Result<ContainerCoverage> {
    #[derive(sqlx::FromRow)]
    struct ContainerRow {
        project_id: String,
        dataset_id: String,
    }

    let rows = kyomi_core::db_fetch_all!(
        db,
        ContainerRow,
        r#"
        SELECT DISTINCT project_id, dataset_id
        FROM datasource_table_cache
        WHERE workspace_id = $1
          AND datasource_config_id = $2
          AND is_archived = false
        "#,
        workspace_id,
        datasource_config_id
    )
    .map_err(|e| {
        kyomi_core::Error::Internal(format!("failed to check container coverage: {e}"))
    })?;

    let live_count = rows.len();
    if live_count == 0 {
        return Ok(ContainerCoverage {
            warning: None,
            material: false,
        });
    }

    // KYO-614 follow-up: keyed by the full `(project_id, dataset_id)` pair —
    // see `ContainerKey`'s doc comment for why `dataset_id` alone would
    // collapse two same-named datasets in different BigQuery projects into
    // one entry, under-counting both the shortfall here and the archive
    // scope in `archive_missing_tables`.
    let mut missing: Vec<ContainerKey> = rows
        .into_iter()
        .map(|r| (r.project_id, r.dataset_id))
        .filter(|key| !enumerated_containers.contains(key))
        .collect();

    if missing.is_empty() {
        return Ok(ContainerCoverage {
            warning: None,
            material: false,
        });
    }
    missing.sort();

    let material = is_material_shortfall(enumerated_containers.len(), live_count);

    // KYO-616: this is the logging half of the KYO-614 material-shortfall
    // check above — reusing `material` (from `is_material_shortfall`)
    // rather than a second, independently-tuned threshold on top of it, so
    // there is exactly one definition of "materially short" anywhere in the
    // catalog indexers rather than two that can silently disagree.
    if material {
        warn!(
            workspace_id,
            datasource_config_id,
            enumerated_containers = enumerated_containers.len(),
            live_containers = live_count,
            missing_containers = missing.len(),
            "catalog refresh's container enumeration fell materially short of this \
             datasource's known containers — archive/status decisions for this run treat \
             that as elevated risk rather than a clean pass"
        );
    }

    let shown_count = missing.len().min(MAX_MISSING_CONTAINERS_IN_WARNING);
    let remainder = missing.len() - shown_count;
    let shown = missing[..shown_count]
        .iter()
        .map(format_container_key)
        .collect::<Vec<_>>()
        .join(", ");
    let names = if remainder > 0 {
        format!("{shown} (+{remainder} more)")
    } else {
        shown
    };

    let warning = format!(
        "Catalog refresh enumerated {} of {live_count} known container(s) this run — \
         not yet re-verified and left untouched: {names}",
        enumerated_containers.len()
    );

    Ok(ContainerCoverage {
        warning: Some(warning),
        material,
    })
}

/// Render a [`ContainerKey`] for a human-readable warning: `"project.dataset"`
/// when the project half is non-empty (BigQuery), or just `dataset_id`
/// otherwise (every other provider always has an empty `project_id` half
/// here, per `ContainerKey`'s doc comment).
fn format_container_key((project_id, dataset_id): &ContainerKey) -> String {
    if project_id.is_empty() {
        dataset_id.clone()
    } else {
        format!("{project_id}.{dataset_id}")
    }
}

/// Render a capped, sorted, human-readable list of container keys —
/// mirrors [`check_container_coverage`]'s own "+N more" summarization
/// (sharing its [`MAX_MISSING_CONTAINERS_IN_WARNING`] cap rather than a
/// second cap value) so a datasource with dozens of containers doesn't blow
/// up a single KYO-616 log line.
fn format_container_keys_capped(containers: &HashSet<ContainerKey>) -> String {
    let mut sorted: Vec<&ContainerKey> = containers.iter().collect();
    sorted.sort();

    let shown_count = sorted.len().min(MAX_MISSING_CONTAINERS_IN_WARNING);
    let remainder = sorted.len() - shown_count;
    let shown = sorted[..shown_count]
        .iter()
        .map(|k| format_container_key(k))
        .collect::<Vec<_>>()
        .join(", ");

    if remainder > 0 {
        format!("{shown} (+{remainder} more)")
    } else {
        shown
    }
}

// ─── Cross-indexer enumeration logging (KYO-616) ───────────────────────────────
//
// The three catalog indexer paths — the SQL-template method
// (`kyomi_agent::catalog::traits::index_catalog_sql`), the BigQuery
// user-dataset REST indexer (`kyomi_auth::catalog::indexers::user_dataset`),
// and the Connect indexer (`kyomi_agent::catalog::indexers::connect`) — each
// walk their own containers/tables with a genuinely different loop shape (a
// generic trait-driven container loop; nested BigQuery project/dataset REST
// calls; a single already-materialized `CatalogResult`), so there is no one
// loop to share between them. What IS shared is the log line itself — its
// field names and what "discovered" vs. "enumerated" mean — so the three
// paths report the same incident-reconstruction vocabulary instead of three
// independently-worded ones.

/// Log a run's enumeration summary: how many containers were discovered vs.
/// successfully enumerated this run, and the enumerated containers' names
/// (capped). Called once per run, after the container-discovery/enumeration
/// loop completes, by all three catalog indexer paths.
pub fn log_run_enumeration_summary(
    workspace_id: &str,
    datasource_config_id: &str,
    container_label: &str,
    containers_discovered: usize,
    enumerated_containers: &HashSet<ContainerKey>,
) {
    info!(
        workspace_id,
        datasource_config_id,
        container_label,
        containers_discovered,
        containers_enumerated = enumerated_containers.len(),
        enumerated_container_names = %format_container_keys_capped(enumerated_containers),
        "catalog run enumeration summary"
    );
}

/// Log one container's per-table indexing summary: how many tables it
/// listed, how many were cached successfully, and how many errored (a
/// schema-read denial or a `cache_table` write failure). Called once per
/// container by all three catalog indexer paths.
pub fn log_container_table_summary(
    workspace_id: &str,
    datasource_config_id: &str,
    container_label: &str,
    container_name: &str,
    tables_listed: usize,
    tables_cached: usize,
    tables_errored: usize,
) {
    debug!(
        workspace_id,
        datasource_config_id,
        container_label,
        container = container_name,
        tables_listed,
        tables_cached,
        tables_errored,
        "catalog container indexing summary"
    );
}

/// Apply a [`ContainerCoverage`] check's outcome onto a run's persisted
/// status/failure-reason pair and its accumulated `errors`.
///
/// All three catalog-indexer call sites (`kyomi_agent::catalog::traits::
/// index_catalog_sql`, `kyomi_agent::catalog::indexers::connect::
/// process_discovered_catalog`, and this crate's own
/// `UserDatasetIndexer::index_workspace_catalog`) need the exact same two
/// rules applied to the exact same two outputs — extracted here rather than
/// duplicated a third time (see `docs/CODING_STANDARDS.md`, "the third copy
/// of a test helper is the extraction trigger": the same reasoning applies
/// to a production decision, per
/// [`RunOutcome`]'s own doc comment on why `resolve_run_outcome` itself was
/// shared):
///
/// - A *material* shortfall overrides the status ONLY when it would
///   otherwise be `"idle"` — a run already `"failed"` for an unrelated
///   reason stays `"failed"` with its original reason, and the shortfall
///   only contributes its warning.
/// - The warning, when present, is always appended to `errors` (which
///   becomes the persisted `warnings` array) regardless of `material` — a
///   small, non-material shortfall still deserves a visible note about
///   which container wasn't re-verified, it just must not flip a healthy
///   status to `"failed"`.
///
/// Pure and I/O-free (the coverage check itself already happened) so this
/// exact decision can be exercised directly by a unit test — including for
/// `UserDatasetIndexer::index_workspace_catalog`, which has no HTTP-mocking
/// seam to drive an end-to-end test through (no such dependency exists in
/// this crate; verified against `[dev-dependencies]`).
pub fn apply_container_coverage(
    coverage: ContainerCoverage,
    status: &mut &'static str,
    failure_reason: &mut Option<String>,
    errors: &mut Vec<String>,
) {
    if coverage.material && *status == "idle" {
        *status = "failed";
        *failure_reason = coverage.warning.clone();
    }
    if let Some(warning) = coverage.warning {
        errors.push(warning);
    }
}

// ─── Container liveness GC (KYO-622) ───────────────────────────────────────

/// Consecutive *complete* enumerations (see [`reconcile_container_liveness`]'s
/// `run_complete` parameter) a container must be absent from before its
/// still-live cached rows are archived by the liveness GC.
///
/// `archive_missing_tables`'s KYO-614 scope check (`is_archive_candidate`) is
/// deliberately permanent: a container that stops being enumerated never
/// re-enters any run's scope set, so its rows stay `is_archived = false`
/// forever unless something else closes the loop. This constant is that
/// something else's threshold, chosen against the exact same asymmetry that
/// motivates the KYO-614 check itself — preserving a genuinely-deleted
/// table costs one stale row; archiving wrongly costs the customer their
/// catalog with no error surfaced.
///
/// A single complete run being wrong would require the discovery layer
/// itself to lie about having seen everything — exactly KYO-619's shape
/// (confirmed BigQuery pagination/absent-vs-empty bug), now fixed, and not
/// something this threshold should have to re-guard against on its own. `3`
/// gives margin against an unknown-unknown of the same class — a second,
/// undiscovered way a "complete" run's enumeration could still miss a live
/// container — while bounding how stale a genuinely-live container's cache
/// can get to roughly three refresh cycles, the same order of magnitude
/// `can_refresh_now`'s rate limit already imposes between runs.
const MISSED_COMPLETE_RUNS_BEFORE_ARCHIVE: i64 = 3;

/// Outcome of one [`reconcile_container_liveness`] call.
pub struct ContainerLivenessOutcome {
    /// `(project_id, dataset_id)` pairs archived this call, for logging.
    pub archived_containers: Vec<ContainerKey>,
    /// `full_table_name`-formatted table ids archived this call — same
    /// shape as [`archive_missing_tables`]'s own return value.
    pub archived_tables: Vec<String>,
}

/// Reclaim `datasource_table_cache` rows belonging to a container that has
/// genuinely stopped existing at the source.
///
/// `archive_missing_tables` never touches a row whose container wasn't
/// enumerated THIS run — an un-enumerated container is an unknown, not a
/// confirmed absence (KYO-614) — so without this function a container
/// deleted at the source stays un-archived forever, permanently inflating
/// [`check_container_coverage`]'s live-container denominator. This function
/// is the separate mechanism, with its own independent evidence, that
/// reclaims it — see `datasource_container_cache`'s migration for the full
/// background.
///
/// `run_complete` MUST be `false` whenever the calling run produced ANY
/// error (a denied container, a failed table listing, a partial discovery
/// failure folded into an opaque error string). Neither `discover_all_containers`
/// (`kyomi_agent::catalog::traits`, a flat `Result<Vec<String>>`) nor
/// Connect's `CatalogResult` (per-container failures are opaque strings
/// folded into `errors`, not attributable to a container) can say
/// "enumeration succeeded for containers X and Y but not Z" — only "did
/// this run's enumeration succeed as a whole". Whole-run completeness is
/// therefore the only sound evidence available; this deliberately does NOT
/// attempt to parse `errors` strings to attribute a failure to a specific
/// container. Being conservative here is deliberate: a table-level error
/// does not strictly mean a container was missed, but the archive-wrongly
/// -vs-preserve-a-stale-row asymmetry means treating "maybe missed" as
/// "definitely missed, for GC purposes" is the only safe default.
///
/// When `run_complete` is `false` this is a strict no-op — not even a
/// `missed_runs` counter reset — and returns an empty outcome without
/// touching the database at all. That is the load-bearing guarantee: a run
/// that could not see a container can never advance it toward deletion, no
/// matter how many incomplete runs pass.
///
/// When `run_complete` is `true`:
/// 1. Every container in `enumerated_containers` is upserted with
///    `last_seen_at = now`, `missed_runs = 0` — it was just seen, so any
///    prior miss streak is void.
/// 2. Every *live* container (distinct `(project_id, dataset_id)` over
///    non-archived `datasource_table_cache` rows for this datasource) that
///    is NOT in `enumerated_containers` has `missed_runs` incremented
///    (starting at 1 if it has no liveness row yet).
/// 3. Any container whose `missed_runs` has now reached
///    [`MISSED_COMPLETE_RUNS_BEFORE_ARCHIVE`] has its still-live cache rows
///    archived, and its liveness row deleted — a later reappearance starts
///    clean rather than resuming a stale counter.
/// 4. Liveness rows for containers with no live cache rows left at all are
///    pruned — there is nothing left for them to protect, whether they were
///    archived by this call, by [`archive_missing_tables`] itself, or by an
///    [`ArchiveScope::EntireDatasource`] sweep.
///
/// Callers run this immediately before [`check_container_coverage`] at all
/// three catalog-indexer call sites, so rows this call reclaims drop out of
/// that function's live-container denominator in the same run they're
/// reclaimed.
///
/// Never reuses [`is_disproportionate_archive`] or [`is_material_shortfall`]
/// as a decision input — both are purely observational (see their own doc
/// comments) and MUST NEVER gate control flow. This function's only decision
/// threshold is [`MISSED_COMPLETE_RUNS_BEFORE_ARCHIVE`].
pub async fn reconcile_container_liveness(
    db: &DbPool,
    workspace_id: &str,
    datasource_config_id: &str,
    enumerated_containers: &HashSet<ContainerKey>,
    run_complete: bool,
) -> Result<ContainerLivenessOutcome> {
    if !run_complete {
        // KYO-622's load-bearing guarantee — see the doc comment above. An
        // incomplete run genuinely knows nothing about whether an absent
        // container still exists, so it must behave as if it were never
        // called at all: no upsert, no increment, no read.
        return Ok(ContainerLivenessOutcome {
            archived_containers: Vec::new(),
            archived_tables: Vec::new(),
        });
    }

    let is_pg = db.is_postgres();
    let now_expr = kyomi_core::sql_compat::now(is_pg);

    // Step 1: every enumerated container was just seen this (complete) run
    // — reset its streak.
    for (project_id, dataset_id) in enumerated_containers {
        let sql = format!(
            "INSERT INTO datasource_container_cache \
                (workspace_id, datasource_config_id, project_id, dataset_id, last_seen_at, missed_runs, created_at, updated_at) \
             VALUES ($1, $2, $3, $4, {now}, 0, {now}, {now}) \
             ON CONFLICT (workspace_id, datasource_config_id, project_id, dataset_id) \
             DO UPDATE SET last_seen_at = {now}, missed_runs = 0, updated_at = {now}",
            now = now_expr,
        );
        kyomi_core::db_execute!(
            db,
            &sql,
            workspace_id,
            datasource_config_id,
            project_id,
            dataset_id
        )
        .map_err(|e| {
            kyomi_core::Error::Internal(format!(
                "failed to record container liveness for {project_id}.{dataset_id}: {e}"
            ))
        })?;
    }

    // Step 2: which currently-live containers were NOT enumerated this run
    // — same "live" definition `check_container_coverage` uses, keyed by
    // the same `(project_id, dataset_id)` tuple per `ContainerKey`'s doc
    // comment.
    #[derive(sqlx::FromRow)]
    struct LiveContainerRow {
        project_id: String,
        dataset_id: String,
    }

    let live_rows = kyomi_core::db_fetch_all!(
        db,
        LiveContainerRow,
        r#"
        SELECT DISTINCT project_id, dataset_id
        FROM datasource_table_cache
        WHERE workspace_id = $1
          AND datasource_config_id = $2
          AND is_archived = false
        "#,
        workspace_id,
        datasource_config_id
    )
    .map_err(|e| {
        kyomi_core::Error::Internal(format!(
            "failed to list live containers for liveness reconciliation: {e}"
        ))
    })?;

    let missing: Vec<ContainerKey> = live_rows
        .into_iter()
        .map(|r| (r.project_id, r.dataset_id))
        .filter(|key| !enumerated_containers.contains(key))
        .collect();

    let mut archived_containers = Vec::new();
    let mut archived_tables = Vec::new();

    for (project_id, dataset_id) in &missing {
        let sql = format!(
            "INSERT INTO datasource_container_cache \
                (workspace_id, datasource_config_id, project_id, dataset_id, missed_runs, created_at, updated_at) \
             VALUES ($1, $2, $3, $4, 1, {now}, {now}) \
             ON CONFLICT (workspace_id, datasource_config_id, project_id, dataset_id) \
             DO UPDATE SET missed_runs = datasource_container_cache.missed_runs + 1, updated_at = {now}",
            now = now_expr,
        );
        kyomi_core::db_execute!(
            db,
            &sql,
            workspace_id,
            datasource_config_id,
            project_id,
            dataset_id
        )
        .map_err(|e| {
            kyomi_core::Error::Internal(format!(
                "failed to advance miss streak for container {project_id}.{dataset_id}: {e}"
            ))
        })?;

        let missed_runs: i64 = kyomi_core::db_fetch_scalar!(
            db,
            i64,
            "SELECT missed_runs FROM datasource_container_cache \
             WHERE workspace_id = $1 AND datasource_config_id = $2 \
               AND project_id = $3 AND dataset_id = $4",
            workspace_id,
            datasource_config_id,
            project_id,
            dataset_id
        )
        .map_err(|e| {
            kyomi_core::Error::Internal(format!(
                "failed to read back miss streak for container {project_id}.{dataset_id}: {e}"
            ))
        })?;

        if missed_runs < MISSED_COMPLETE_RUNS_BEFORE_ARCHIVE {
            continue;
        }

        // Step 3: this container has been absent from
        // MISSED_COMPLETE_RUNS_BEFORE_ARCHIVE consecutive complete runs —
        // reclaim its still-live rows. Fetch the table ids BEFORE archiving
        // so the names this call reports match exactly what it archived.
        #[derive(sqlx::FromRow)]
        struct TableIdRow {
            table_id: String,
        }
        let table_rows = kyomi_core::db_fetch_all!(
            db,
            TableIdRow,
            "SELECT table_id FROM datasource_table_cache \
             WHERE workspace_id = $1 AND datasource_config_id = $2 \
               AND project_id = $3 AND dataset_id = $4 AND is_archived = false",
            workspace_id,
            datasource_config_id,
            project_id,
            dataset_id
        )
        .map_err(|e| {
            kyomi_core::Error::Internal(format!(
                "failed to list live tables for container {project_id}.{dataset_id} \
                 before liveness-GC archive: {e}"
            ))
        })?;

        let archive_sql = format!(
            "UPDATE datasource_table_cache SET is_archived = true, updated_at = {now_expr} \
             WHERE workspace_id = $1 AND datasource_config_id = $2 \
               AND project_id = $3 AND dataset_id = $4 AND is_archived = false"
        );
        kyomi_core::db_execute!(
            db,
            &archive_sql,
            workspace_id,
            datasource_config_id,
            project_id,
            dataset_id
        )
        .map_err(|e| {
            kyomi_core::Error::Internal(format!(
                "failed to archive container {project_id}.{dataset_id} via liveness GC: {e}"
            ))
        })?;

        // A returning reappearance must start clean, not resume a counter
        // that would otherwise immediately re-trip the threshold.
        let delete_sql = "DELETE FROM datasource_container_cache \
             WHERE workspace_id = $1 AND datasource_config_id = $2 \
               AND project_id = $3 AND dataset_id = $4";
        kyomi_core::db_execute!(
            db,
            delete_sql,
            workspace_id,
            datasource_config_id,
            project_id,
            dataset_id
        )
        .map_err(|e| {
            kyomi_core::Error::Internal(format!(
                "failed to clear liveness row for archived container {project_id}.{dataset_id}: {e}"
            ))
        })?;

        for row in table_rows {
            archived_tables.push(kyomi_core::build_full_table_name(
                project_id,
                dataset_id,
                &row.table_id,
            ));
        }
        archived_containers.push((project_id.clone(), dataset_id.clone()));

        warn!(
            workspace_id,
            datasource_config_id,
            project_id,
            dataset_id,
            missed_runs,
            "liveness GC archived a container absent from \
             MISSED_COMPLETE_RUNS_BEFORE_ARCHIVE consecutive complete enumerations"
        );
    }

    // Step 4: prune liveness rows for containers with no live cache rows
    // left at all — nothing left for them to protect, whether they were
    // just archived above, archived earlier by `archive_missing_tables`
    // itself, or wiped by an `ArchiveScope::EntireDatasource` sweep.
    let prune_sql = "DELETE FROM datasource_container_cache \
         WHERE workspace_id = $1 AND datasource_config_id = $2 \
           AND NOT EXISTS ( \
             SELECT 1 FROM datasource_table_cache t \
             WHERE t.workspace_id = datasource_container_cache.workspace_id \
               AND t.datasource_config_id = datasource_container_cache.datasource_config_id \
               AND t.project_id = datasource_container_cache.project_id \
               AND t.dataset_id = datasource_container_cache.dataset_id \
               AND t.is_archived = false \
           )";
    kyomi_core::db_execute!(db, prune_sql, workspace_id, datasource_config_id).map_err(|e| {
        kyomi_core::Error::Internal(format!("failed to prune stale container-liveness rows: {e}"))
    })?;

    if !archived_containers.is_empty() {
        info!(
            workspace_id,
            datasource_config_id,
            archived_containers = archived_containers.len(),
            archived_tables = archived_tables.len(),
            "liveness GC reclaimed containers absent from repeated complete enumerations"
        );
    }

    Ok(ContainerLivenessOutcome {
        archived_containers,
        archived_tables,
    })
}

/// Update the datasource's last_catalog_refresh timestamp.
pub async fn update_datasource_last_refresh(
    db: &DbPool,
    datasource_config_id: &str,
) -> Result<()> {
    let is_pg = db.is_postgres();
    let now_expr = kyomi_core::sql_compat::now(is_pg);
    let sql = format!(
        "UPDATE datasource_configs SET last_catalog_refresh = {now_expr} WHERE id = $1"
    );

    kyomi_core::db_execute!(db, &sql, datasource_config_id)
        .map_err(|e| {
            kyomi_core::Error::Internal(format!(
                "failed to update datasource last_catalog_refresh: {e}"
            ))
        })?;

    Ok(())
}

/// Stamp the datasource's `last_index_started_at` column with `now()`.
///
/// Called at the top of [`CatalogIndexingService::index_datasource`] so that
/// any concurrent caller (scheduler, post-create spawn, manual refresh)
/// can observe that an indexing run is in flight and skip.
///
/// [`CatalogIndexingService::index_datasource`]: (see crate `kyomi-agent`)
pub async fn stamp_last_index_started_at(
    db: &DbPool,
    datasource_config_id: &str,
) -> Result<()> {
    let is_pg = db.is_postgres();
    let now_expr = kyomi_core::sql_compat::now(is_pg);
    let sql = format!(
        "UPDATE datasource_configs SET last_index_started_at = {now_expr} WHERE id = $1"
    );

    kyomi_core::db_execute!(db, &sql, datasource_config_id).map_err(|e| {
        kyomi_core::Error::Internal(format!(
            "failed to stamp datasource last_index_started_at: {e}"
        ))
    })?;

    Ok(())
}

/// Returns `true` if an indexing run for this datasource started within
/// the last `minutes_threshold` minutes.
///
/// Reads `datasource_configs.last_index_started_at`. Returns `false` if:
/// - the column is NULL (never indexed), OR
/// - the stamp is older than the threshold (self-healing — a panicked run's
///   stamp ages out and the next attempt proceeds), OR
/// - the row can't be found or the query errors (fail open; the downstream
///   indexer will produce a clearer error if the datasource is genuinely
///   missing).
///
/// Use in combination with `can_refresh_now` (which guards against
/// "just finished, don't re-index" via `last_catalog_refresh`). This guard
/// protects against "just started, don't double up".
pub async fn index_started_within(
    db: &DbPool,
    datasource_config_id: &str,
    minutes_threshold: i64,
) -> bool {
    #[derive(sqlx::FromRow)]
    struct StartedRow {
        last_index_started_at: Option<chrono::DateTime<Utc>>,
    }

    let row = kyomi_core::db_fetch_optional!(
        db,
        StartedRow,
        "SELECT last_index_started_at FROM datasource_configs WHERE id = $1",
        datasource_config_id
    );

    let Ok(Some(row)) = row else {
        return false; // not found or error → allow caller to proceed
    };

    match row.last_index_started_at {
        None => false,
        Some(ts) => (Utc::now() - ts).num_minutes() < minutes_threshold,
    }
}

/// Parameters for [`cache_table`].
pub struct CacheTableParams<'a> {
    pub db: &'a DbPool,
    pub embedding: &'a EmbeddingService,
    pub ctx: &'a IndexerContext,
    pub project_id: &'a str,
    pub dataset_id: &'a str,
    pub table_name: &'a str,
    pub table_type: &'a str,
    pub columns: &'a [ColumnEntry],
    pub full_table_id: &'a str,
}

/// Cache a table and generate embeddings for its search entries.
///
/// This is the core caching + embedding function used by all indexers.
///
/// Flow:
/// 1. Build table_metadata JSON from columns
/// 2. Check if table exists in cache
/// 3. If exists AND schema unchanged AND embeddings exist → skip (update last_verified)
/// 4. Otherwise → upsert cache entry, delete old embeddings, generate new ones
///
/// Returns `Ok(())` if the table was cached/updated, `Err` naming the
/// underlying failure and `full_table_id` on any error along that path
/// (KYO-364) — every caller must treat a write failure as a table that
/// still needs to count toward its run's `errors`, not a silent no-op.
pub async fn cache_table(params: CacheTableParams<'_>) -> kyomi_core::Result<()> {
    let CacheTableParams {
        db,
        embedding,
        ctx,
        project_id,
        dataset_id,
        table_name,
        table_type,
        columns,
        full_table_id,
    } = params;
    // Build table_metadata JSON
    let columns_json: Vec<Value> = columns
        .iter()
        .map(|c| {
            serde_json::json!({
                "name": c.name,
                "type": c.col_type.as_deref().unwrap_or("unknown"),
                "native_type": c.native_type.as_deref().unwrap_or(""),
                "description": c.description.as_deref().unwrap_or(""),
            })
        })
        .collect();

    let table_metadata = serde_json::json!({
        "table_name": table_name,
        "dataset_id": dataset_id,
        "project_id": project_id,
        "table_type": table_type,
        "columns": columns_json,
    });

    // Check if table already exists in cache
    #[derive(sqlx::FromRow)]
    struct ExistingRow {
        id: i32,
        table_metadata: serde_json::Value,
    }

    let existing = kyomi_core::db_fetch_optional!(
        db,
        ExistingRow,
        r#"
        SELECT id, table_metadata
        FROM datasource_table_cache
        WHERE workspace_id = $1
          AND datasource_config_id = $2
          AND project_id = $3
          AND dataset_id = $4
          AND table_id = $5
        "#,
        &ctx.workspace_id,
        &ctx.datasource_config_id,
        project_id,
        dataset_id,
        table_name
    );

    let existing = match existing {
        Ok(row) => row,
        Err(e) => {
            return Err(kyomi_core::Error::Internal(format!(
                "failed to check existing cache entry for {full_table_id}: {e}"
            )));
        }
    };

    let is_pg = db.is_postgres();
    let now_expr = kyomi_core::sql_compat::now(is_pg);
    let false_val = kyomi_core::sql_compat::bool_false(is_pg);

    if let Some(ref row) = existing {
        let cache_id = row.id;
        let stored_metadata = &row.table_metadata;

        // Compare schema signatures
        let current_sig = compute_schema_signature(columns);
        let stored_sig = extract_schema_signature(stored_metadata);

        if current_sig == stored_sig {
            // Schema unchanged — check if embeddings exist
            let embedding_count: i64 = kyomi_core::db_fetch_scalar!(
                db,
                i64,
                "SELECT COUNT(*) FROM datasource_search_embeddings WHERE table_cache_id = $1",
                cache_id
            )
            .unwrap_or(0);

            if embedding_count > 0 {
                // Schema unchanged AND embeddings exist → just update last_verified
                let sql = format!(
                    "UPDATE datasource_table_cache SET last_verified = {now_expr}, is_archived = {false_val} WHERE id = $1"
                );
                kyomi_core::db_execute!(db, &sql, cache_id).map_err(|e| {
                    kyomi_core::Error::Internal(format!(
                        "failed to touch last_verified for {full_table_id}: {e}"
                    ))
                })?;

                debug!(table = full_table_id, "schema unchanged, skipping re-index");
                return Ok(());
            }
        }

        // Schema changed OR no embeddings → update cache entry and re-embed
        let sql = format!(
            r#"
            UPDATE datasource_table_cache
            SET table_metadata = $1, structure_refreshed_at = {now_expr},
                updated_at = {now_expr}, last_verified = {now_expr}, is_archived = {false_val}
            WHERE id = $2
            "#
        );
        let update_result = kyomi_core::db_execute!(db, &sql, &table_metadata, cache_id);

        if let Err(e) = update_result {
            return Err(kyomi_core::Error::Internal(format!(
                "failed to update cache entry for {full_table_id}: {e}"
            )));
        }

        // Delete old embeddings
        if let Err(e) = delete_embeddings_for_table(db, cache_id).await {
            warn!(table = full_table_id, error = %e, "failed to delete old embeddings");
        }

        // Generate and store new embeddings
        return generate_and_store_embeddings(GenerateEmbeddingsParams {
            db,
            embedding,
            workspace_id: &ctx.workspace_id,
            datasource_config_id: &ctx.datasource_config_id,
            project_id,
            dataset_id,
            table_name,
            columns,
            cache_id,
        })
        .await;
    }

    // Table doesn't exist in cache → insert
    let sql = format!(
        r#"
        INSERT INTO datasource_table_cache
            (workspace_id, datasource_config_id, project_id, dataset_id, table_id,
             table_metadata, is_archived, structure_refreshed_at, last_verified)
        VALUES ($1, $2, $3, $4, $5, $6, {false_val}, {now_expr}, {now_expr})
        RETURNING id
        "#
    );

    #[derive(sqlx::FromRow)]
    struct IdRow {
        id: i32,
    }

    let insert_result = kyomi_core::db_fetch_one!(
        db,
        IdRow,
        &sql,
        &ctx.workspace_id,
        &ctx.datasource_config_id,
        project_id,
        dataset_id,
        table_name,
        &table_metadata
    );

    let cache_id = match insert_result {
        Ok(row) => row.id,
        Err(e) => {
            return Err(kyomi_core::Error::Internal(format!(
                "failed to insert cache entry for {full_table_id}: {e}"
            )));
        }
    };

    generate_and_store_embeddings(GenerateEmbeddingsParams {
        db,
        embedding,
        workspace_id: &ctx.workspace_id,
        datasource_config_id: &ctx.datasource_config_id,
        project_id,
        dataset_id,
        table_name,
        columns,
        cache_id,
    })
    .await
}

/// Extract a schema signature from stored `table_metadata` JSON.
///
/// Parses the `columns` array and produces a sorted signature matching
/// the format from [`compute_schema_signature`].
fn extract_schema_signature(table_metadata: &Value) -> Vec<(String, String, String)> {
    let Some(columns) = table_metadata.get("columns").and_then(|c| c.as_array()) else {
        return Vec::new();
    };

    let mut sig: Vec<(String, String, String)> = columns
        .iter()
        .map(|c| {
            (
                c.get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                c.get("type")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                c.get("description")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
            )
        })
        .collect();
    sig.sort();
    sig
}

struct GenerateEmbeddingsParams<'a> {
    db: &'a DbPool,
    embedding: &'a EmbeddingService,
    workspace_id: &'a str,
    datasource_config_id: &'a str,
    project_id: &'a str,
    dataset_id: &'a str,
    table_name: &'a str,
    columns: &'a [ColumnEntry],
    cache_id: i32,
}

/// Generate search entries, compute embeddings, and store them.
async fn generate_and_store_embeddings(
    params: GenerateEmbeddingsParams<'_>,
) -> kyomi_core::Result<()> {
    let GenerateEmbeddingsParams {
        db,
        embedding,
        workspace_id,
        datasource_config_id,
        project_id,
        dataset_id,
        table_name,
        columns,
        cache_id,
    } = params;
    let entries = create_search_entries(dataset_id, table_name, table_name, columns);

    if entries.is_empty() {
        return Ok(());
    }

    // Collect texts for batch embedding
    let texts: Vec<&str> = entries.iter().map(|e| e.text.as_str()).collect();

    let vectors = match embedding.embed_passages_chunked(&texts).await {
        Ok(vecs) => vecs,
        Err(e) => {
            return Err(kyomi_core::Error::Internal(format!(
                "failed to compute embeddings for {dataset_id}.{table_name}: {e}"
            )));
        }
    };

    if vectors.len() != entries.len() {
        return Err(kyomi_core::Error::Internal(format!(
            "embedding count mismatch for {dataset_id}.{table_name}: expected {}, got {}",
            entries.len(),
            vectors.len()
        )));
    }

    // Build insertion records
    let inserts: Vec<SearchEntryInsert> = entries
        .iter()
        .zip(vectors.iter())
        .map(|(entry, vec)| SearchEntryInsert {
            table_cache_id: cache_id,
            workspace_id: workspace_id.to_string(),
            datasource_config_id: Some(datasource_config_id.to_string()),
            project_id: project_id.to_string(),
            dataset_id: dataset_id.to_string(),
            table_id: table_name.to_string(),
            entry_type: entry.entry_type.clone(),
            text: entry.text.clone(),
            weight: entry.weight,
            column_name: entry.column_name.clone(),
            embedding: Vector::from(vec.clone()),
        })
        .collect();

    if let Err(e) = store_search_embeddings(db, &inserts).await {
        return Err(kyomi_core::Error::Internal(format!(
            "failed to store embeddings for {dataset_id}.{table_name}: {e}"
        )));
    }

    Ok(())
}

// ─── Per-table outcome folding (shared by user_dataset.rs and bigquery_public.rs) ──

/// Cap on how many per-table failures — schema-fetch denials
/// (`TableOutcome::SchemaUnreadable`) and catalog write failures
/// (`TableOutcome::WriteFailed`, KYO-364) alike, sharing this one cap — a
/// single dataset contributes to a run's `errors` (and, for
/// `user_dataset.rs`, the persisted failure reason) before further failures
/// are collapsed into one summary line.
///
/// Every real caller of `UserDatasetIndexer::index_workspace_catalog` passes
/// `max_tables_per_dataset: None` (verified: `catalog_scheduler.rs:489`,
/// `catalog_scheduler.rs:638`, `indexing_service.rs:355/423`,
/// `sql_editor.rs:760` all pass `None`), so nothing upstream bounds how many
/// tables a single dataset can enumerate. Without this cap, a dataset with
/// thousands of tables under a blanket `bigquery.tables.get` denial, or a
/// blanket `cache_table` write failure, would grow both
/// `CatalogIndexResult::errors` and any summarised persisted failure reason
/// to thousands of near-identical lines.
pub const MAX_TABLE_ERRORS_PER_DATASET: usize = 5;

/// What happened to one table while indexing a single dataset.
///
/// Shared by `user_dataset.rs`'s `index_dataset_tables` and
/// `bigquery_public.rs`'s `index_public_dataset_tables` — both fetch a
/// table's schema, then call `cache_table`, and both need the same
/// three-way outcome folded the same way (KYO-365 gave the public indexer
/// the machinery `user_dataset.rs` gained in KYO-324/KYO-364).
pub enum TableOutcome {
    /// Schema read and `cache_table` wrote the catalog row.
    Indexed,
    /// Schema read, but `cache_table` failed to write the row. Carries the
    /// underlying error text (KYO-364) — before `cache_table` returned
    /// `Result`, this state was `NotCached`, a bare `false` with no
    /// attached reason, and was silently dropped from `table_errors`.
    WriteFailed(String),
    /// The table was listed, but its schema could not be read.
    SchemaUnreadable(String),
}

/// The result of indexing every table listed in one dataset.
pub struct DatasetOutcome {
    pub tables_indexed: usize,
    /// Fully-qualified ids of EVERY table the listing returned — readable or
    /// not. `user_dataset.rs`'s archiving keys off this set, so a table
    /// whose schema fetch was denied must still appear here or the run
    /// would evict a table that demonstrably still exists (KYO-324).
    ///
    /// `bigquery_public.rs` has no archiving machinery at all (KYO-365) and
    /// deliberately ignores this field — that is not dead weight, it is the
    /// same fold shared across a caller that needs it and one that doesn't.
    pub seen_table_ids: Vec<String>,
    /// Bounded, formatted per-table failures — schema-fetch denials and
    /// catalog write failures alike (KYO-364).
    pub table_errors: Vec<String>,
}

/// Fold per-table indexing outcomes from one dataset into a
/// [`DatasetOutcome`].
///
/// Mirrors `fold_dataset_outcomes` (KYO-264, `user_dataset.rs`) one level
/// down: every outcome — whether the schema read succeeded, the catalog
/// write failed, or the schema itself could not be read — contributes its
/// `full_table_id` to `seen_table_ids`. That is the archiving invariant
/// KYO-324 (extended by KYO-364) exists to protect for `user_dataset.rs`: a
/// table whose schema fetch was denied, or whose `cache_table` write
/// failed, was still *listed*, so it must not be treated as gone. Both
/// `SchemaUnreadable` and `WriteFailed` contribute to `table_errors`,
/// sharing a single `MAX_TABLE_ERRORS_PER_DATASET` cap with a trailing
/// summary line for anything beyond it — a blanket `cache_table` failure
/// (e.g. the DB connection dropped) fails every table in the dataset
/// exactly like a blanket schema-read denial does, so it needs the same
/// bound.
///
/// Deliberately free of I/O (`outcomes` are already-resolved `TableOutcome`s)
/// so this can be exercised directly by a unit test without an
/// HTTP-mocking dependency — none exists in this workspace.
pub fn fold_table_outcomes(
    dataset_label: &str,
    outcomes: Vec<(String, TableOutcome)>,
) -> DatasetOutcome {
    let mut tables_indexed = 0usize;
    let mut seen_table_ids = Vec::with_capacity(outcomes.len());
    let mut table_errors = Vec::new();
    let mut errors_beyond_cap = 0usize;

    for (full_table_id, outcome) in outcomes {
        seen_table_ids.push(full_table_id.clone());

        let failure_msg = match outcome {
            TableOutcome::Indexed => None,
            TableOutcome::SchemaUnreadable(msg) => Some(msg),
            TableOutcome::WriteFailed(msg) => Some(msg),
        };

        match failure_msg {
            None => tables_indexed += 1,
            Some(msg) => {
                if table_errors.len() < MAX_TABLE_ERRORS_PER_DATASET {
                    table_errors.push(format!("{full_table_id}: {msg}"));
                } else {
                    errors_beyond_cap += 1;
                }
            }
        }
    }

    if errors_beyond_cap > 0 {
        table_errors.push(format!(
            "{dataset_label}: {errors_beyond_cap} further table failure{} not shown",
            if errors_beyond_cap == 1 { "" } else { "s" }
        ));
    }

    DatasetOutcome {
        tables_indexed,
        seen_table_ids,
        table_errors,
    }
}

// ─── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── resolve_final_status (KYO-126) ───────────────────────────────────

    #[test]
    fn errored_and_empty_reports_failed_with_reason() {
        let errors = vec!["Failed to list tables in schema 'public': permission denied".to_string()];
        let (status, reason) = resolve_final_status(true, &errors);
        assert_eq!(status, "failed");
        assert_eq!(
            reason,
            Some("Failed to list tables in schema 'public': permission denied".to_string())
        );
    }

    #[test]
    fn errored_and_empty_with_multiple_errors_reports_count() {
        let errors = vec![
            "Failed to list tables in schema 'a': permission denied".to_string(),
            "Failed to list tables in schema 'b': permission denied".to_string(),
        ];
        let (status, reason) = resolve_final_status(true, &errors);
        assert_eq!(status, "failed");
        assert_eq!(
            reason,
            Some(
                "Failed to list tables in schema 'a': permission denied (+1 more error)"
                    .to_string()
            )
        );
    }

    #[test]
    fn empty_without_errors_reports_idle() {
        // Regression guard (KYO-126): an accessible datasource that
        // genuinely has zero tables (or where the user configured zero
        // containers) must not be reported as failed.
        let (status, reason) = resolve_final_status(true, &[]);
        assert_eq!(status, "idle");
        assert_eq!(reason, None);
    }

    #[test]
    fn not_nothing_found_reports_idle_even_with_partial_errors() {
        // A normal completion where some individual tables/containers
        // errored but at least one table was still indexed is a partial
        // success, not a failure — those errors are already surfaced via
        // `CatalogIndexResult::errors`.
        let errors = vec!["Failed to get columns for public.weird_table: timeout".to_string()];
        let (status, reason) = resolve_final_status(false, &errors);
        assert_eq!(status, "idle");
        assert_eq!(reason, None);
    }

    // ── resolve_run_outcome (KYO-324, extracted for KYO-385) ─────────────
    //
    // `user_dataset.rs` already exercises this decision end-to-end through
    // its own local `resolve_run_outcome` wrapper (which delegates here with
    // `empty_scope_is_expected = false`), so those tests are not repeated.
    // These cover the one input `user_dataset.rs` never exercises:
    // `empty_scope_is_expected = true`, which is new in KYO-385.

    /// Headline KYO-385 case: every table listed, every schema read OK,
    /// every `cache_table` write failed. `seen_any_table` is `true` (every
    /// listed table demonstrably exists) but `tables_indexed == 0` — the
    /// two questions this function exists to keep separate disagree, and
    /// status must follow `tables_indexed`, not `seen_any_table`.
    #[test]
    fn all_listed_all_write_failed_archives_but_reports_failed() {
        let errors = vec!["failed to insert cache entry for t1: db closed".to_string()];
        let outcome = resolve_run_outcome(true, 0, &errors, false);
        assert!(
            outcome.archive,
            "every table was listed — archiving must still run and preserve them"
        );
        assert_eq!(outcome.status, "failed");
        assert_eq!(
            outcome.failure_reason,
            Some("failed to insert cache entry for t1: db closed".to_string())
        );
    }

    #[test]
    fn partial_write_failure_still_archives_and_reports_idle() {
        let errors = vec!["failed to insert cache entry for t2: db closed".to_string()];
        let outcome = resolve_run_outcome(true, 1, &errors, false);
        assert!(outcome.archive);
        assert_eq!(
            outcome.status, "idle",
            "one table indexed successfully is still a partial success, not a failure"
        );
        assert_eq!(outcome.failure_reason, None);
    }

    #[test]
    fn nothing_listed_skips_archiving_and_reports_idle_when_no_errors() {
        let outcome = resolve_run_outcome(false, 0, &[], false);
        assert!(
            !outcome.archive,
            "nothing was listed — archiving must be skipped so the existing catalog is preserved"
        );
        assert_eq!(outcome.status, "idle");
        assert_eq!(outcome.failure_reason, None);
    }

    /// `empty_scope_is_expected = true`: the user configured zero containers
    /// (SQL path) or explicitly cleared the container selection (Connect
    /// path). Nothing was ever listed and nothing was ever attempted, so
    /// this must archive (evict the now-out-of-scope catalog) and report
    /// `"idle"` — never `"failed"` — regardless of `errors`, which will be
    /// empty in practice (discovery is skipped entirely in this state) but
    /// is deliberately not trusted to stay that way by this test.
    #[test]
    fn empty_scope_is_expected_archives_and_reports_idle_even_with_errors() {
        let errors = vec!["this should never be produced when scope is intentionally empty".to_string()];
        let outcome = resolve_run_outcome(false, 0, &errors, true);
        assert!(
            outcome.archive,
            "an intentionally emptied scope must still archive the now-stale catalog"
        );
        assert_eq!(
            outcome.status, "idle",
            "an intentionally emptied scope must never be reported as failed"
        );
        assert_eq!(outcome.failure_reason, None);
    }

    /// `empty_scope_is_expected` is not just "always idle" — confirm it
    /// overrides `seen_any_table`'s archive gate independently of overriding
    /// `tables_indexed`'s status gate, by checking `archive` and `status`
    /// each mutate correctly rather than one masking the other.
    #[test]
    fn empty_scope_is_expected_overrides_archive_and_status_independently() {
        let never_true = resolve_run_outcome(false, 0, &[], false);
        assert!(!never_true.archive);
        assert_eq!(never_true.status, "idle");

        let with_carve_out = resolve_run_outcome(false, 0, &[], true);
        assert!(
            with_carve_out.archive,
            "the carve-out alone must flip archive from false to true"
        );
        assert_eq!(with_carve_out.status, "idle");
    }

    // ── build_progress_envelope (KYO-126) ────────────────────────────────

    #[test]
    fn envelope_puts_error_as_top_level_sibling_of_progress() {
        // This is the exact shape `get_catalog_refresh_status`
        // (kyomi-ui/src/server_fns/sql_editor.rs) and the settings page's
        // refresh poller read: `envelope.get("error")` directly on the
        // whole `catalog_refresh_progress` column value, not nested under
        // `envelope["progress"]["error"]`. Before this field existed, no
        // caller ever populated an "error" key at all, so that lookup
        // always missed regardless of what the caller passed as `progress`.
        let envelope = build_progress_envelope(
            "ds-1",
            Some(&serde_json::json!({"processed": 3})),
            Some("permission denied for schema analytics"),
            &[],
        );

        assert_eq!(
            envelope.get("error").and_then(|v| v.as_str()),
            Some("permission denied for schema analytics")
        );
        assert_eq!(
            envelope.get("progress"),
            Some(&serde_json::json!({"processed": 3}))
        );
        assert_eq!(
            envelope.get("datasource_config_id").and_then(|v| v.as_str()),
            Some("ds-1")
        );
    }

    #[test]
    fn envelope_error_is_null_when_none() {
        let envelope = build_progress_envelope("ds-1", None, None, &[]);
        assert!(envelope.get("error").is_some_and(|v| v.is_null()));
    }

    // ── build_progress_envelope warnings (KYO-327) ───────────────────────

    #[test]
    fn envelope_puts_warnings_as_top_level_sibling_of_progress_and_error() {
        // Same shape requirement as `envelope_puts_error_as_top_level_sibling_of_progress`
        // above, extended to the new `"warnings"` key: `get_catalog_stats`
        // (kyomi-ui/src/server_fns/datasources.rs) reads
        // `envelope.get("warnings")` directly on the whole
        // `catalog_refresh_progress` column value, not nested under
        // `envelope["progress"]["warnings"]`.
        let warnings =
            vec!["Failed to list tables in schema 'restricted': permission denied".to_string()];
        let envelope = build_progress_envelope(
            "ds-1",
            Some(&serde_json::json!({"processed": 3})),
            None,
            &warnings,
        );

        assert_eq!(
            envelope.get("warnings"),
            Some(&serde_json::json!([
                "Failed to list tables in schema 'restricted': permission denied"
            ]))
        );
        assert_eq!(
            envelope.get("progress"),
            Some(&serde_json::json!({"processed": 3}))
        );
    }

    #[test]
    fn envelope_warnings_is_empty_array_not_null_when_none() {
        // Unlike `"error"`, which is `null` when absent, `"warnings"` must
        // always be a present, empty JSON array — readers should never need
        // null-handling to distinguish "no warnings" from "field missing".
        let envelope = build_progress_envelope("ds-1", None, None, &[]);
        assert_eq!(envelope.get("warnings"), Some(&serde_json::json!([])));
        assert!(
            !envelope.get("warnings").is_some_and(|v| v.is_null()),
            "warnings must be an empty array, not null, when the run had no warnings"
        );
    }

    #[test]
    fn extract_schema_signature_from_metadata() {
        let metadata = serde_json::json!({
            "table_name": "users",
            "columns": [
                {"name": "id", "type": "number", "native_type": "INT", "description": ""},
                {"name": "name", "type": "string", "native_type": "VARCHAR", "description": "User name"},
            ]
        });

        let sig = extract_schema_signature(&metadata);
        assert_eq!(sig.len(), 2);
        // Should be sorted by name
        assert_eq!(sig[0].0, "id");
        assert_eq!(sig[1].0, "name");
        assert_eq!(sig[1].2, "User name");
    }

    #[test]
    fn extract_schema_signature_empty_columns() {
        let metadata = serde_json::json!({"table_name": "empty"});
        let sig = extract_schema_signature(&metadata);
        assert!(sig.is_empty());
    }

    #[test]
    fn extract_schema_signature_matches_compute() {
        let columns = vec![
            ColumnEntry {
                name: "a".into(),
                col_type: Some("string".into()),
                native_type: Some("VARCHAR".into()),
                description: Some("desc a".into()),
            },
            ColumnEntry {
                name: "b".into(),
                col_type: Some("number".into()),
                native_type: Some("INT".into()),
                description: None,
            },
        ];

        let computed = compute_schema_signature(&columns);

        // Build the equivalent metadata JSON
        let metadata = serde_json::json!({
            "columns": [
                {"name": "a", "type": "string", "native_type": "VARCHAR", "description": "desc a"},
                {"name": "b", "type": "number", "native_type": "INT", "description": ""},
            ]
        });
        let extracted = extract_schema_signature(&metadata);

        assert_eq!(computed, extracted);
    }

    // ── update_datasource_status concurrency (KYO-267) ───────────────────

    /// Seeds one workspace with two datasource rows, `datasource_config_id`s
    /// `"ds-A-{suffix}"` and `"ds-B-{suffix}"`. Parameterized by suffix so
    /// the two tests below don't collide on primary keys.
    async fn seed_two_datasource_fixture(sq: &sqlx::SqlitePool, suffix: &str) -> (String, String, String) {
        let user_id = format!("u-concurrency-{suffix}");
        let workspace_id = format!("ws-concurrency-{suffix}");
        let ds_a = format!("ds-A-{suffix}");
        let ds_b = format!("ds-B-{suffix}");

        sqlx::query("INSERT INTO users (user_id, email) VALUES (?, ?)")
            .bind(&user_id)
            .bind(format!("{user_id}@test.local"))
            .execute(sq)
            .await
            .expect("insert user");
        sqlx::query("INSERT INTO workspaces (workspace_id, name, owner_user_id) VALUES (?, 'WS', ?)")
            .bind(&workspace_id)
            .bind(&user_id)
            .execute(sq)
            .await
            .expect("insert workspace");
        for (id, slug) in [(&ds_a, "a"), (&ds_b, "b")] {
            sqlx::query(
                "INSERT INTO datasource_configs (id, workspace_id, name, datasource_type, slug) \
                 VALUES (?, ?, ?, 'postgres', ?)",
            )
            .bind(id)
            .bind(&workspace_id)
            .bind(format!("DS-{slug}-{suffix}"))
            .bind(format!("{slug}-{suffix}"))
            .execute(sq)
            .await
            .expect("insert datasource_config");
        }

        (workspace_id, ds_a, ds_b)
    }

    /// Reads back `(catalog_refresh_status, error reason)` for a single
    /// datasource, extracting the reason the same way
    /// `get_catalog_refresh_status`/`get_catalog_stats` do: the top-level
    /// `"error"` key of the stored `catalog_refresh_progress` envelope.
    async fn read_datasource_status(sq: &sqlx::SqlitePool, datasource_config_id: &str) -> (String, Option<String>) {
        #[derive(sqlx::FromRow)]
        struct Row {
            catalog_refresh_status: Option<String>,
            catalog_refresh_progress: Option<String>,
        }

        let row: Row = sqlx::query_as(
            "SELECT catalog_refresh_status, catalog_refresh_progress FROM datasource_configs WHERE id = ?",
        )
        .bind(datasource_config_id)
        .fetch_one(sq)
        .await
        .expect("read datasource status");

        let reason = row
            .catalog_refresh_progress
            .as_deref()
            .and_then(|p| serde_json::from_str::<Value>(p).ok())
            .and_then(|v| v.get("error").and_then(|e| e.as_str()).map(str::to_string));

        (row.catalog_refresh_status.unwrap_or_else(|| "idle".to_string()), reason)
    }

    /// This is the whole point of KYO-267. Before this fix,
    /// `catalog_refresh_status`/`catalog_refresh_progress` lived on
    /// `workspaces` and were shared by every datasource in it — datasource
    /// A failing, then datasource B finishing successfully, meant B's
    /// `"idle"` write silently clobbered A's `"failed"` + reason with no
    /// history of the failure ever having existed (the KYO-126 bug
    /// reintroduced by a different mechanism). Each datasource now owns its
    /// own status/reason pair, so A's failure must survive B's unrelated
    /// success regardless of write order.
    #[tokio::test]
    async fn concurrent_datasources_retain_independent_terminal_status() {
        let db = crate::test_support::test_pool().await;
        let DbPool::Sqlite(sq) = &db else {
            unreachable!("expected sqlite pool");
        };
        let (workspace_id, ds_a, ds_b) = seed_two_datasource_fixture(sq, "order1").await;

        // A fails first...
        update_datasource_status(&db, &workspace_id, &ds_a, "failed", None, Some("permission denied for schema analytics"), &[])
            .await
            .expect("write A failed");
        // ...then B finishes successfully.
        update_datasource_status(&db, &workspace_id, &ds_b, "idle", None, None, &[])
            .await
            .expect("write B idle");

        let (status_a, reason_a) = read_datasource_status(sq, &ds_a).await;
        let (status_b, reason_b) = read_datasource_status(sq, &ds_b).await;

        assert_eq!(status_a, "failed", "B's success must not clobber A's failure");
        assert_eq!(reason_a, Some("permission denied for schema analytics".to_string()));
        assert_eq!(status_b, "idle");
        assert_eq!(reason_b, None);
    }

    /// Companion to the test above with the write order reversed: B
    /// succeeds first, then A fails. A shared-column implementation would
    /// report whichever wrote last (A's `"failed"`) for *both* datasources
    /// here; per-datasource columns must still show each its own outcome
    /// regardless of ordering.
    #[tokio::test]
    async fn concurrent_datasources_retain_independent_terminal_status_reverse_order() {
        let db = crate::test_support::test_pool().await;
        let DbPool::Sqlite(sq) = &db else {
            unreachable!("expected sqlite pool");
        };
        let (workspace_id, ds_a, ds_b) = seed_two_datasource_fixture(sq, "order2").await;

        // B succeeds first...
        update_datasource_status(&db, &workspace_id, &ds_b, "idle", None, None, &[])
            .await
            .expect("write B idle");
        // ...then A fails.
        update_datasource_status(&db, &workspace_id, &ds_a, "failed", None, Some("connection timed out"), &[])
            .await
            .expect("write A failed");

        let (status_a, reason_a) = read_datasource_status(sq, &ds_a).await;
        let (status_b, reason_b) = read_datasource_status(sq, &ds_b).await;

        assert_eq!(status_a, "failed");
        assert_eq!(reason_a, Some("connection timed out".to_string()));
        assert_eq!(status_b, "idle", "A's later failure must not clobber B's earlier success");
        assert_eq!(reason_b, None);
    }

    // ── fold_table_outcomes (KYO-324, moved from user_dataset.rs for KYO-365
    // sharing) ──────────────────────────────────────────────────────────
    //
    // `fold_table_outcomes` is the pure seam this ticket exists to add.
    // `index_dataset_tables` inserted every listed table id into
    // `seen_table_ids` BEFORE fetching its schema, so a schema-fetch denial
    // never wrongly archived the table (see `archive_missing_tables`) — but
    // the denial itself was dropped (`warn!` + `continue`), so a dataset
    // where every schema fetch was denied looked identical to a genuinely
    // empty dataset: `tables_indexed == 0`, `seen_table_ids` non-empty
    // (implying `nothing_found == false`), `errors` empty — which the old
    // single `nothing_found` predicate resolved to `"idle"`.

    fn schema_denied(table: &str) -> TableOutcome {
        TableOutcome::SchemaUnreadable(format!("HTTP 403: permission denied reading {table}"))
    }

    /// A `cache_table` write failure (KYO-364) — the schema read succeeded,
    /// but the catalog write itself (existing-row lookup, UPDATE, INSERT, or
    /// embedding generation/storage) returned `Err`.
    fn write_failed(table: &str) -> TableOutcome {
        TableOutcome::WriteFailed(format!("failed to insert cache entry for {table}: db closed"))
    }

    #[test]
    fn all_tables_indexed_counts_and_seen_ids_match() {
        let outcomes = vec![
            ("proj-1.ds_a.t1".to_string(), TableOutcome::Indexed),
            ("proj-1.ds_a.t2".to_string(), TableOutcome::Indexed),
        ];
        let result = fold_table_outcomes("proj-1.ds_a", outcomes);
        assert_eq!(result.tables_indexed, 2);
        assert_eq!(
            result.seen_table_ids,
            vec!["proj-1.ds_a.t1".to_string(), "proj-1.ds_a.t2".to_string()]
        );
        assert!(result.table_errors.is_empty());
    }

    /// AC3 / the trap this ticket calls out explicitly: every table's
    /// schema fetch is denied, but every one of them was still *listed*.
    /// `seen_table_ids` must contain ALL of them — that set is what
    /// `archive_missing_tables` uses to decide what still exists. If this
    /// regresses (a table dropped from `seen_table_ids` because its schema
    /// fetch failed), a total `bigquery.tables.get` denial would archive
    /// the entire existing catalog instead of just failing the run.
    #[test]
    fn all_schema_unreadable_still_populates_seen_table_ids_completely() {
        let table_ids = ["proj-1.ds_a.t1", "proj-1.ds_a.t2", "proj-1.ds_a.t3"];
        let outcomes: Vec<(String, TableOutcome)> = table_ids
            .iter()
            .map(|t| (t.to_string(), schema_denied(t)))
            .collect();

        let result = fold_table_outcomes("proj-1.ds_a", outcomes);

        assert_eq!(result.tables_indexed, 0);
        assert_eq!(
            result.seen_table_ids.len(),
            table_ids.len(),
            "every listed table must appear in seen_table_ids even when its schema fetch failed"
        );
        for t in &table_ids {
            assert!(
                result.seen_table_ids.contains(&t.to_string()),
                "{t} missing from seen_table_ids — archive_missing_tables would wrongly evict it"
            );
        }
        assert_eq!(result.table_errors.len(), table_ids.len());
        for (t, err) in table_ids.iter().zip(result.table_errors.iter()) {
            assert!(
                err.starts_with(&format!("{t}: ")),
                "expected table-id-prefixed error, got: {err}"
            );
        }
    }

    #[test]
    fn mixed_indexed_unreadable_and_write_failed_counts_and_errors_correctly() {
        let outcomes = vec![
            ("proj-1.ds_a.t1".to_string(), TableOutcome::Indexed),
            ("proj-1.ds_a.t2".to_string(), schema_denied("proj-1.ds_a.t2")),
            ("proj-1.ds_a.t3".to_string(), write_failed("proj-1.ds_a.t3")),
        ];
        let result = fold_table_outcomes("proj-1.ds_a", outcomes);

        assert_eq!(result.tables_indexed, 1);
        assert_eq!(
            result.seen_table_ids.len(),
            3,
            "a write failure must still count as seen — the table demonstrably exists"
        );
        assert_eq!(result.table_errors.len(), 2);
        assert!(result.table_errors[0].starts_with("proj-1.ds_a.t2: "));
        assert!(result.table_errors[1].starts_with("proj-1.ds_a.t3: "));
    }

    /// Criterion 5 (shared cap): a mix of `SchemaUnreadable` and
    /// `WriteFailed` outcomes in one dataset must share the single
    /// `MAX_TABLE_ERRORS_PER_DATASET` cap rather than getting one cap each —
    /// they're both "this table failed" from `table_errors`' point of view.
    #[test]
    fn schema_unreadable_and_write_failed_share_one_cap() {
        // One kind's worth exactly fills the cap, so neither kind alone
        // overflows. Only a *shared* cap overflows on the combined 2×
        // fixture — a per-kind cap would admit all 10 with no summary line,
        // which is precisely what the length assertion below rules out.
        let half = MAX_TABLE_ERRORS_PER_DATASET;
        let mut outcomes: Vec<(String, TableOutcome)> = (0..half)
            .map(|i| {
                let t = format!("proj-1.ds_a.unreadable{i}");
                (t.clone(), schema_denied(&t))
            })
            .collect();
        outcomes.extend((0..half).map(|i| {
            let t = format!("proj-1.ds_a.writefail{i}");
            (t.clone(), write_failed(&t))
        }));
        let total = outcomes.len();
        assert!(
            total > MAX_TABLE_ERRORS_PER_DATASET,
            "test fixture must exceed the cap to be meaningful"
        );

        let result = fold_table_outcomes("proj-1.ds_a", outcomes);

        assert_eq!(result.seen_table_ids.len(), total);
        assert_eq!(
            result.table_errors.len(),
            MAX_TABLE_ERRORS_PER_DATASET + 1,
            "SchemaUnreadable and WriteFailed must share one cap, not one each"
        );
        let summary = result.table_errors.last().expect("summary line present");
        assert!(summary.starts_with("proj-1.ds_a: "));
        assert!(summary.contains(&(total - MAX_TABLE_ERRORS_PER_DATASET).to_string()));
    }

    /// More `SchemaUnreadable` outcomes than `MAX_TABLE_ERRORS_PER_DATASET`:
    /// the individual error list must be bounded, with one trailing summary
    /// line for the rest — but `seen_table_ids` must remain complete, since
    /// archiving must not be affected by the error cap.
    #[test]
    fn table_errors_are_capped_with_a_summary_line_but_seen_ids_stay_complete() {
        let total = MAX_TABLE_ERRORS_PER_DATASET + 3;
        let table_ids: Vec<String> = (0..total)
            .map(|i| format!("proj-1.ds_a.t{i}"))
            .collect();
        let outcomes: Vec<(String, TableOutcome)> = table_ids
            .iter()
            .map(|t| (t.clone(), schema_denied(t)))
            .collect();

        let result = fold_table_outcomes("proj-1.ds_a", outcomes);

        assert_eq!(result.tables_indexed, 0);
        assert_eq!(
            result.seen_table_ids.len(),
            total,
            "the error cap must not drop any table from seen_table_ids"
        );

        // MAX_TABLE_ERRORS_PER_DATASET individual errors, plus one summary line.
        assert_eq!(result.table_errors.len(), MAX_TABLE_ERRORS_PER_DATASET + 1);
        for i in 0..MAX_TABLE_ERRORS_PER_DATASET {
            assert!(
                result.table_errors[i].starts_with(&format!("proj-1.ds_a.t{i}: ")),
                "expected table {i}'s error to be individually listed, got: {}",
                result.table_errors[i]
            );
        }
        let summary = result.table_errors.last().expect("summary line present");
        assert!(
            summary.starts_with("proj-1.ds_a: "),
            "summary line must be labeled with the dataset, got: {summary}"
        );
        assert!(
            summary.contains(&(total - MAX_TABLE_ERRORS_PER_DATASET).to_string()),
            "summary line must name how many further failures were dropped, got: {summary}"
        );
    }

    /// Criterion 4 (cap): more than `MAX_TABLE_ERRORS_PER_DATASET` write
    /// failures in one dataset ⇒ exactly `MAX + 1` entries (the cap plus one
    /// summary line), with correct singular/plural wording and `seen_table_ids`
    /// left uncapped.
    #[test]
    fn write_failures_are_capped_with_a_correctly_pluralized_summary_line() {
        // Exactly one over the cap so the summary line is singular ("1
        // further table failure"), proving the singular/plural branch.
        let total = MAX_TABLE_ERRORS_PER_DATASET + 1;
        let table_ids: Vec<String> = (0..total)
            .map(|i| format!("proj-1.ds_a.t{i}"))
            .collect();
        let outcomes: Vec<(String, TableOutcome)> = table_ids
            .iter()
            .map(|t| (t.clone(), write_failed(t)))
            .collect();

        let result = fold_table_outcomes("proj-1.ds_a", outcomes);

        assert_eq!(result.tables_indexed, 0);
        assert_eq!(
            result.seen_table_ids.len(),
            total,
            "the error cap must not drop any table from seen_table_ids"
        );
        assert_eq!(result.table_errors.len(), MAX_TABLE_ERRORS_PER_DATASET + 1);
        let summary = result.table_errors.last().expect("summary line present");
        assert_eq!(
            summary, "proj-1.ds_a: 1 further table failure not shown",
            "singular wording must be exact for a one-over-cap overflow"
        );

        // Companion case: comfortably over the cap, plural wording.
        let total_plural = MAX_TABLE_ERRORS_PER_DATASET + 3;
        let table_ids_plural: Vec<String> = (0..total_plural)
            .map(|i| format!("proj-1.ds_b.t{i}"))
            .collect();
        let outcomes_plural: Vec<(String, TableOutcome)> = table_ids_plural
            .iter()
            .map(|t| (t.clone(), write_failed(t)))
            .collect();
        let result_plural = fold_table_outcomes("proj-1.ds_b", outcomes_plural);
        let summary_plural = result_plural
            .table_errors
            .last()
            .expect("summary line present");
        assert_eq!(
            summary_plural, "proj-1.ds_b: 3 further table failures not shown",
            "plural wording must be exact for a multi-over-cap overflow"
        );
    }

    // ── ArchiveScope / archive_missing_tables (KYO-614) ──────────────────

    /// Seeds one datasource with cache rows in two containers
    /// (`dataset_id` values). `rows` is `(project_id, dataset_id, table_id)`
    /// triples, all inserted non-archived — `project_id` is explicit (KYO-614
    /// follow-up) rather than a constant baked into this helper, so a test
    /// can seed two different projects sharing a dataset name.
    async fn seed_container_scoped_fixture(
        sq: &sqlx::SqlitePool,
        suffix: &str,
        rows: &[(&str, &str, &str)],
    ) -> (String, String) {
        let user_id = format!("u-cs-{suffix}");
        let workspace_id = format!("ws-cs-{suffix}");
        let datasource_config_id = format!("ds-cs-{suffix}");

        sqlx::query("INSERT INTO users (user_id, email) VALUES (?, ?)")
            .bind(&user_id)
            .bind(format!("{user_id}@test.local"))
            .execute(sq)
            .await
            .expect("insert user");
        sqlx::query("INSERT INTO workspaces (workspace_id, name, owner_user_id) VALUES (?, 'WS', ?)")
            .bind(&workspace_id)
            .bind(&user_id)
            .execute(sq)
            .await
            .expect("insert workspace");
        sqlx::query(
            "INSERT INTO datasource_configs (id, workspace_id, name, datasource_type, slug) \
             VALUES (?, ?, 'DS', 'bigquery', ?)",
        )
        .bind(&datasource_config_id)
        .bind(&workspace_id)
        .bind(format!("ds-{suffix}"))
        .execute(sq)
        .await
        .expect("insert datasource_config");

        // KYO-614 follow-up: rows are `(project_id, dataset_id, table_id)`
        // triples, not just `(dataset_id, table_id)` — every test must be
        // explicit about `project_id`, since the whole point of this
        // fixture shape is to let a test seed two different projects
        // sharing a dataset name.
        for (project_id, dataset_id, table_id) in rows {
            sqlx::query(
                r#"
                INSERT INTO datasource_table_cache
                    (workspace_id, datasource_config_id, project_id, dataset_id, table_id, table_metadata, is_archived)
                VALUES (?, ?, ?, ?, ?, '{}', 0)
                "#,
            )
            .bind(&workspace_id)
            .bind(&datasource_config_id)
            .bind(*project_id)
            .bind(*dataset_id)
            .bind(*table_id)
            .execute(sq)
            .await
            .expect("seed cache row");
        }

        (workspace_id, datasource_config_id)
    }

    async fn archived_state(
        sq: &sqlx::SqlitePool,
        datasource_config_id: &str,
        project_id: &str,
        dataset_id: &str,
        table_id: &str,
    ) -> bool {
        let is_archived: i64 = sqlx::query_scalar(
            "SELECT is_archived FROM datasource_table_cache \
             WHERE datasource_config_id = ? AND project_id = ? AND dataset_id = ? AND table_id = ?",
        )
        .bind(datasource_config_id)
        .bind(project_id)
        .bind(dataset_id)
        .bind(table_id)
        .fetch_one(sq)
        .await
        .expect("read archival state");
        is_archived != 0
    }

    /// Headline regression (KYO-614): a run that only enumerated one dataset
    /// out of two must not touch the OTHER dataset's rows at all, regardless
    /// of `seen_table_ids` — before this fix, `archive_missing_tables` had
    /// no container filter, so every row not present in `seen_table_ids`
    /// (which only ever contained the enumerated dataset's tables) was
    /// archived, including the entire un-enumerated dataset.
    #[tokio::test]
    async fn subset_enumeration_archives_nothing_outside_the_enumerated_set() {
        let db = DbPool::connect("sqlite::memory:")
            .await
            .expect("connect in-memory sqlite");
        let DbPool::Sqlite(sq) = &db else {
            unreachable!("expected sqlite pool");
        };
        let (workspace_id, ds) = seed_container_scoped_fixture(
            sq,
            "subset",
            &[("proj-1", "dataset_a", "t1"), ("proj-1", "dataset_b", "t1")],
        )
        .await;

        // This run enumerated ONLY dataset_a, and found nothing in it (t1
        // does not appear in seen_table_ids) — a real deletion within the
        // enumerated container.
        let scope = ArchiveScope::Containers(HashSet::from([("proj-1".to_string(), "dataset_a".to_string())]));
        let seen_table_ids = HashSet::new();

        let archived = archive_missing_tables(&db, &workspace_id, &ds, &scope, &seen_table_ids, 0)
            .await
            .expect("archive_missing_tables");

        assert_eq!(
            archived,
            vec!["proj-1.dataset_a.t1".to_string()],
            "only the enumerated dataset's unseen table may be archived"
        );
        assert!(
            !archived_state(sq, &ds, "proj-1", "dataset_b", "t1").await,
            "dataset_b was never enumerated this run — its row must be untouched \
             even though it is absent from seen_table_ids"
        );
    }

    /// A fully-enumerated container that genuinely lost one table must still
    /// archive that table — the KYO-614 fix must not regress ordinary
    /// deletions within a container that WAS enumerated (observed against a
    /// real production container, mirrored here by the `proj-1`/`dataset_a`
    /// fixture below).
    #[tokio::test]
    async fn fully_enumerated_container_still_archives_a_genuinely_deleted_table() {
        let db = DbPool::connect("sqlite::memory:")
            .await
            .expect("connect in-memory sqlite");
        let DbPool::Sqlite(sq) = &db else {
            unreachable!("expected sqlite pool");
        };
        let (workspace_id, ds) = seed_container_scoped_fixture(
            sq,
            "deletion",
            &[("proj-1", "dataset_a", "kept"), ("proj-1", "dataset_a", "deleted")],
        )
        .await;

        let scope = ArchiveScope::Containers(HashSet::from([("proj-1".to_string(), "dataset_a".to_string())]));
        let seen_table_ids = HashSet::from(["proj-1.dataset_a.kept".to_string()]);

        let archived = archive_missing_tables(&db, &workspace_id, &ds, &scope, &seen_table_ids, 1)
            .await
            .expect("archive_missing_tables");

        assert_eq!(archived, vec!["proj-1.dataset_a.deleted".to_string()]);
        assert!(!archived_state(sq, &ds, "proj-1", "dataset_a", "kept").await);
        assert!(archived_state(sq, &ds, "proj-1", "dataset_a", "deleted").await);
    }

    /// AC3: a run whose enumeration returns zero containers (as opposed to a
    /// user who deliberately configured zero, which is `EntireDatasource`)
    /// must archive nothing at all, preserving the whole existing catalog.
    #[tokio::test]
    async fn empty_containers_scope_archives_nothing() {
        let db = DbPool::connect("sqlite::memory:")
            .await
            .expect("connect in-memory sqlite");
        let DbPool::Sqlite(sq) = &db else {
            unreachable!("expected sqlite pool");
        };
        let (workspace_id, ds) =
            seed_container_scoped_fixture(sq, "zeroenum", &[("proj-1", "dataset_a", "t1")]).await;

        let scope = ArchiveScope::Containers(HashSet::new());
        let seen_table_ids = HashSet::new();

        let archived = archive_missing_tables(&db, &workspace_id, &ds, &scope, &seen_table_ids, 0)
            .await
            .expect("archive_missing_tables");

        assert!(
            archived.is_empty(),
            "an empty enumerated-containers scope must archive nothing"
        );
        assert!(!archived_state(sq, &ds, "proj-1", "dataset_a", "t1").await);
    }

    /// The explicit-empty-selection carve-out: `ArchiveScope::EntireDatasource`
    /// must apply no container filter at all — every row not in
    /// `seen_table_ids` (empty here, since discovery is skipped entirely for
    /// an intentionally emptied selection) is archived, across every
    /// container.
    #[tokio::test]
    async fn entire_datasource_scope_archives_across_every_container() {
        let db = DbPool::connect("sqlite::memory:")
            .await
            .expect("connect in-memory sqlite");
        let DbPool::Sqlite(sq) = &db else {
            unreachable!("expected sqlite pool");
        };
        let (workspace_id, ds) = seed_container_scoped_fixture(
            sq,
            "entirewipe",
            &[("proj-1", "dataset_a", "t1"), ("proj-1", "dataset_b", "t1")],
        )
        .await;

        let scope = ArchiveScope::EntireDatasource;
        let seen_table_ids = HashSet::new();

        let mut archived = archive_missing_tables(&db, &workspace_id, &ds, &scope, &seen_table_ids, 0)
            .await
            .expect("archive_missing_tables");
        archived.sort();

        assert_eq!(
            archived,
            vec![
                "proj-1.dataset_a.t1".to_string(),
                "proj-1.dataset_b.t1".to_string(),
            ],
            "an intentionally emptied selection must archive every container's rows"
        );
    }

    // ── cross-indexer enumeration logging (KYO-616) ────────────────────────
    //
    // `log_run_enumeration_summary`/`log_container_table_summary` are the
    // one shared implementation all three catalog indexer paths (SQL
    // template, BigQuery user-dataset REST, Connect) call from their own
    // differently-shaped loops — tested directly here rather than through
    // each of the three call sites' own heavier fixtures.

    #[test]
    fn run_enumeration_summary_reports_discovered_vs_enumerated_and_names() {
        let logs = kyomi_test_tracing::capture_tracing();
        let enumerated = HashSet::from([
            ("".to_string(), "public".to_string()),
            ("".to_string(), "analytics".to_string()),
        ]);

        log_run_enumeration_summary("ws-1", "ds-1", "schema", 3, &enumerated);

        let (level, message) = logs
            .events()
            .into_iter()
            .find(|(_, msg)| msg.contains("catalog run enumeration summary"))
            .expect("expected the enumeration-summary log line");
        assert_eq!(level, tracing::Level::INFO);
        assert!(message.contains("container_label=\"schema\""), "got: {message}");
        assert!(message.contains("containers_discovered=3"), "got: {message}");
        assert!(message.contains("containers_enumerated=2"), "got: {message}");
        assert!(message.contains("enumerated_container_names=public, analytics")
            || message.contains("enumerated_container_names=analytics, public"), "got: {message}");
    }

    #[test]
    fn container_table_summary_reports_listed_cached_and_errored() {
        let logs = kyomi_test_tracing::capture_tracing();

        log_container_table_summary("ws-1", "ds-1", "dataset", "proj-1.dataset_a", 5, 3, 2);

        assert!(
            logs.has_message_containing(tracing::Level::DEBUG, "catalog container indexing summary"),
            "captured: {:?}",
            logs.events()
        );
        assert!(
            logs.has_message_containing(tracing::Level::DEBUG, "tables_listed=5"),
            "captured: {:?}",
            logs.events()
        );
        assert!(
            logs.has_message_containing(tracing::Level::DEBUG, "tables_cached=3"),
            "captured: {:?}",
            logs.events()
        );
        assert!(
            logs.has_message_containing(tracing::Level::DEBUG, "tables_errored=2"),
            "captured: {:?}",
            logs.events()
        );
        assert!(
            logs.has_message_containing(tracing::Level::DEBUG, "container=\"proj-1.dataset_a\""),
            "captured: {:?}",
            logs.events()
        );
    }

    // ── archive-decision log line (KYO-616) ───────────────────────────────
    //
    // The production incident this ticket exists to prevent left only
    // `tables_indexed:2 tables_archived:34 errors:0` in the logs —
    // reconstructable afterward only by diffing the database. These pin
    // that the archive *decision*, not just its outcome, is legible: the
    // seen-count, the count about to be archived, and the distinct
    // containers on each side (enumerated vs. about-to-be-archived).

    #[tokio::test]
    async fn archive_decision_log_reports_seen_and_about_to_archive_counts_and_containers() {
        let db = DbPool::connect("sqlite::memory:")
            .await
            .expect("connect in-memory sqlite");
        let DbPool::Sqlite(sq) = &db else {
            unreachable!("expected sqlite pool");
        };
        let (workspace_id, ds) = seed_container_scoped_fixture(
            sq,
            "archivelog",
            &[
                ("proj-1", "dataset_a", "kept"),
                ("proj-1", "dataset_a", "deleted"),
                ("proj-1", "dataset_b", "t1"),
            ],
        )
        .await;

        // Only dataset_a was enumerated this run; "deleted" was listed but
        // not seen this cycle. dataset_b was never enumerated at all and
        // must appear on neither side of the decision.
        let scope = ArchiveScope::Containers(HashSet::from([(
            "proj-1".to_string(),
            "dataset_a".to_string(),
        )]));
        let seen_table_ids = HashSet::from(["proj-1.dataset_a.kept".to_string()]);

        let logs = kyomi_test_tracing::capture_tracing();
        let archived = archive_missing_tables(&db, &workspace_id, &ds, &scope, &seen_table_ids, 1)
            .await
            .expect("archive_missing_tables");
        assert_eq!(archived, vec!["proj-1.dataset_a.deleted".to_string()]);

        let decision_events: Vec<(tracing::Level, String)> = logs
            .events()
            .into_iter()
            .filter(|(_, msg)| {
                msg.contains("evaluating archive scope before applying destructive changes")
            })
            .collect();
        assert_eq!(
            decision_events.len(),
            1,
            "expected exactly one archive-decision log line; captured: {:?}",
            logs.events()
        );
        let (level, message) = &decision_events[0];
        assert_eq!(*level, tracing::Level::INFO);
        assert!(message.contains("archive_scope=\"containers\""), "got: {message}");
        assert!(
            message.contains("seen_count=1"),
            "expected the seen-count in the log line, got: {message}"
        );
        assert!(
            message.contains("candidates_about_to_archive=1"),
            "expected the about-to-archive count, got: {message}"
        );
        assert!(
            message.contains("about_to_archive_containers=1"),
            "expected the distinct about-to-archive container count, got: {message}"
        );
        assert!(
            message.contains("about_to_archive_container_names=proj-1.dataset_a"),
            "expected the about-to-archive container's name, got: {message}"
        );
        assert!(
            message.contains("enumerated_containers=1"),
            "expected the enumerated-container count from the ArchiveScope, got: {message}"
        );
        assert!(
            message.contains("enumerated_container_names=proj-1.dataset_a"),
            "expected the enumerated container's name, got: {message}"
        );
        assert!(
            !message.contains("dataset_b"),
            "dataset_b was never enumerated this run and must not appear on either side \
             of the archive decision, got: {message}"
        );
    }

    /// `ArchiveScope::EntireDatasource` has no bounded enumerated-container
    /// set — the log line must say so via `archive_scope="entire_datasource"`
    /// rather than fabricating an `enumerated_containers` count for a scope
    /// that has none.
    #[tokio::test]
    async fn archive_decision_log_reports_entire_datasource_scope_label() {
        let db = DbPool::connect("sqlite::memory:")
            .await
            .expect("connect in-memory sqlite");
        let DbPool::Sqlite(sq) = &db else {
            unreachable!("expected sqlite pool");
        };
        let (workspace_id, ds) =
            seed_container_scoped_fixture(sq, "archivelogwipe", &[("proj-1", "dataset_a", "t1")])
                .await;

        let logs = kyomi_test_tracing::capture_tracing();
        let _ = archive_missing_tables(
            &db,
            &workspace_id,
            &ds,
            &ArchiveScope::EntireDatasource,
            &HashSet::new(),
            0,
        )
        .await
        .expect("archive_missing_tables");

        assert!(
            logs.has_message_containing(
                tracing::Level::INFO,
                "archive_scope=\"entire_datasource\""
            ),
            "expected the entire-datasource scope label in the archive-decision log; \
             captured: {:?}",
            logs.events()
        );
    }

    // ── disproportionate-archive WARN (KYO-616) ───────────────────────────
    //
    // Purely observational (see `DISPROPORTIONATE_ARCHIVE_RATIO`'s doc
    // comment) — a run can trip this WARN even with FULL container
    // coverage, which the material-shortfall WARN above cannot see at all:
    // that is exactly the gap this section exists to close (a run that
    // enumerates everything and still archives far more than it indexed
    // must not come out silent just because coverage looked clean).

    #[tokio::test]
    async fn disproportionate_archive_warns_on_the_confirmed_incident_shape() {
        // 34 tables archived against 2 indexed — the confirmed production
        // incident shape (17x ratio), reproduced with `EntireDatasource` so
        // every seeded row is a genuine archive candidate.
        let rows: Vec<(String, String, String)> = (0..34)
            .map(|i| ("proj-1".to_string(), "dataset_a".to_string(), format!("t{i}")))
            .collect();
        let rows_ref: Vec<(&str, &str, &str)> =
            rows.iter().map(|(p, d, t)| (p.as_str(), d.as_str(), t.as_str())).collect();

        let db = DbPool::connect("sqlite::memory:")
            .await
            .expect("connect in-memory sqlite");
        let (workspace_id, ds) = {
            let DbPool::Sqlite(sq) = &db else {
                unreachable!("expected sqlite pool");
            };
            seed_container_scoped_fixture(sq, "dispro34", &rows_ref).await
        };

        let logs = kyomi_test_tracing::capture_tracing();
        let archived = archive_missing_tables(
            &db,
            &workspace_id,
            &ds,
            &ArchiveScope::EntireDatasource,
            &HashSet::new(),
            2,
        )
        .await
        .expect("archive_missing_tables");
        assert_eq!(archived.len(), 34);

        let warn_events = logs.events_at(tracing::Level::WARN);
        assert!(
            warn_events.iter().any(|(_, msg)| {
                msg.contains("tables_indexed=2")
                    && msg.contains("candidates_about_to_archive=34")
                    && msg.contains("archive_to_indexed_ratio=17.0x")
            }),
            "expected a WARN naming the disproportionate archive-to-indexed ratio; \
             captured: {:?}",
            logs.events()
        );
    }

    /// The negative case: a run that archives only a small, proportionate
    /// multiple of what it indexed (well below `DISPROPORTIONATE_ARCHIVE_RATIO`)
    /// must not emit the disproportionate-archive WARN at all.
    #[tokio::test]
    async fn small_proportionate_archive_does_not_warn() {
        let db = DbPool::connect("sqlite::memory:")
            .await
            .expect("connect in-memory sqlite");
        let (workspace_id, ds) = {
            let DbPool::Sqlite(sq) = &db else {
                unreachable!("expected sqlite pool");
            };
            seed_container_scoped_fixture(
                sq,
                "dispronoWarn",
                &[
                    ("proj-1", "dataset_a", "t1"),
                    ("proj-1", "dataset_a", "t2"),
                    ("proj-1", "dataset_a", "t3"),
                ],
            )
            .await
        };

        let logs = kyomi_test_tracing::capture_tracing();
        // 3 archived against 1 indexed — a 3x ratio, routine, well below
        // the 5x threshold.
        let archived = archive_missing_tables(
            &db,
            &workspace_id,
            &ds,
            &ArchiveScope::EntireDatasource,
            &HashSet::new(),
            1,
        )
        .await
        .expect("archive_missing_tables");
        assert_eq!(archived.len(), 3);

        assert!(
            !logs.has_message_containing(tracing::Level::WARN, "disproportionately large"),
            "a small, proportionate archive must not trip the disproportionate-archive \
             WARN; captured: {:?}",
            logs.events()
        );
    }

    /// The zero-indexed decision (documented on `is_disproportionate_archive`):
    /// a run that indexed nothing at all but still archives rows — even via
    /// a deliberate `ArchiveScope::EntireDatasource` wipe — must still warn,
    /// rather than skip the check as "undefined" or divide by zero.
    #[tokio::test]
    async fn zero_indexed_run_that_archives_anything_still_warns() {
        let db = DbPool::connect("sqlite::memory:")
            .await
            .expect("connect in-memory sqlite");
        let (workspace_id, ds) = {
            let DbPool::Sqlite(sq) = &db else {
                unreachable!("expected sqlite pool");
            };
            seed_container_scoped_fixture(sq, "dispro0idx", &[("proj-1", "dataset_a", "t1")]).await
        };

        let logs = kyomi_test_tracing::capture_tracing();
        let archived = archive_missing_tables(
            &db,
            &workspace_id,
            &ds,
            &ArchiveScope::EntireDatasource,
            &HashSet::new(),
            0,
        )
        .await
        .expect("archive_missing_tables");
        assert_eq!(archived.len(), 1);

        assert!(
            logs.has_message_containing(
                tracing::Level::WARN,
                "archive_to_indexed_ratio=undefined (zero indexed)"
            ),
            "a zero-indexed run that archives anything must still warn, not divide by \
             zero or skip the check; captured: {:?}",
            logs.events()
        );
    }

    #[test]
    fn is_disproportionate_archive_ignores_a_zero_archive_count() {
        // Nothing archived is never disproportionate, regardless of
        // tables_indexed (including the zero/zero case).
        assert!(!is_disproportionate_archive(0, 0));
        assert!(!is_disproportionate_archive(5, 0));
    }

    #[test]
    fn is_disproportionate_archive_boundary_is_inclusive_at_the_ratio() {
        assert!(
            is_disproportionate_archive(2, 10),
            "exactly 5x must count as disproportionate"
        );
        assert!(
            !is_disproportionate_archive(2, 9),
            "just under 5x must not"
        );
    }

    // ── material-shortfall WARN (KYO-616) ──────────────────────────────────
    //
    // Item 3 of KYO-616 reuses `is_material_shortfall`'s existing
    // determination (`coverage.material`) as the sole trigger for this WARN
    // — no second, independently-tuned threshold.

    #[tokio::test]
    async fn material_shortfall_emits_a_warn_naming_the_coverage_counts() {
        let db = DbPool::connect("sqlite::memory:")
            .await
            .expect("connect in-memory sqlite");
        let (workspace_id, ds) = {
            let DbPool::Sqlite(sq) = &db else {
                unreachable!("expected sqlite pool");
            };
            seed_container_scoped_fixture(
                sq,
                "covwarn",
                &[
                    ("proj-1", "dataset_a", "t1"),
                    ("proj-1", "dataset_b", "t1"),
                    ("proj-1", "dataset_c", "t1"),
                    ("proj-1", "dataset_d", "t1"),
                ],
            )
            .await
        };
        let enumerated = HashSet::from([("proj-1".to_string(), "dataset_a".to_string())]);

        let logs = kyomi_test_tracing::capture_tracing();
        let coverage = check_container_coverage(&db, &workspace_id, &ds, &enumerated)
            .await
            .expect("check_container_coverage");
        assert!(coverage.material, "1 of 4 enumerated must be material");

        let warn_events = logs.events_at(tracing::Level::WARN);
        assert!(
            warn_events.iter().any(|(_, msg)| {
                msg.contains("enumerated_containers=1")
                    && msg.contains("live_containers=4")
                    && msg.contains("missing_containers=3")
            }),
            "expected a WARN naming the exact coverage counts driving the material-shortfall \
             determination; captured: {:?}",
            logs.events()
        );
    }

    /// The negative case: a run with full (non-material) coverage must not
    /// emit the material-shortfall WARN at all — proving the WARN is gated
    /// on `coverage.material`, not unconditional.
    #[tokio::test]
    async fn non_material_shortfall_does_not_emit_the_material_warn() {
        let db = DbPool::connect("sqlite::memory:")
            .await
            .expect("connect in-memory sqlite");
        let (workspace_id, ds) = {
            let DbPool::Sqlite(sq) = &db else {
                unreachable!("expected sqlite pool");
            };
            seed_container_scoped_fixture(
                sq,
                "covnowarn",
                &[("proj-1", "dataset_a", "t1"), ("proj-1", "dataset_b", "t1")],
            )
            .await
        };
        let enumerated = HashSet::from([
            ("proj-1".to_string(), "dataset_a".to_string()),
            ("proj-1".to_string(), "dataset_b".to_string()),
        ]);

        let logs = kyomi_test_tracing::capture_tracing();
        let coverage = check_container_coverage(&db, &workspace_id, &ds, &enumerated)
            .await
            .expect("check_container_coverage");
        assert!(!coverage.material, "full coverage must not be material");

        assert!(
            !logs.has_message_containing(tracing::Level::WARN, "materially short"),
            "full coverage must not emit the material-shortfall WARN; captured: {:?}",
            logs.events()
        );
    }

    // ── is_material_shortfall (KYO-614) ───────────────────────────────────

    #[test]
    fn material_shortfall_fires_on_the_confirmed_incident_shape() {
        // 1 of 4 datasets enumerated — the confirmed production incident.
        assert!(is_material_shortfall(1, 4));
    }

    #[test]
    fn material_shortfall_does_not_fire_on_one_genuine_deletion_out_of_ten() {
        assert!(!is_material_shortfall(9, 10));
    }

    #[test]
    fn material_shortfall_boundary_is_inclusive_at_exactly_half() {
        assert!(
            is_material_shortfall(2, 4),
            "enumerating exactly half must count as material"
        );
        assert!(!is_material_shortfall(3, 4));
    }

    // ── check_container_coverage (KYO-614) ────────────────────────────────

    #[tokio::test]
    async fn zero_live_containers_reports_no_shortfall() {
        let db = DbPool::connect("sqlite::memory:")
            .await
            .expect("connect in-memory sqlite");
        let (workspace_id, ds) = {
            let DbPool::Sqlite(sq) = &db else {
                unreachable!("expected sqlite pool");
            };
            seed_container_scoped_fixture(sq, "covzero", &[]).await
        };

        let coverage = check_container_coverage(&db, &workspace_id, &ds, &HashSet::new())
            .await
            .expect("check_container_coverage");

        assert!(coverage.warning.is_none());
        assert!(!coverage.material);
    }

    #[tokio::test]
    async fn full_coverage_reports_no_shortfall() {
        let db = DbPool::connect("sqlite::memory:")
            .await
            .expect("connect in-memory sqlite");
        let (workspace_id, ds) = {
            let DbPool::Sqlite(sq) = &db else {
                unreachable!("expected sqlite pool");
            };
            seed_container_scoped_fixture(
                sq,
                "covfull",
                &[("proj-1", "dataset_a", "t1"), ("proj-1", "dataset_b", "t1")],
            )
            .await
        };

        let enumerated = HashSet::from([
            ("proj-1".to_string(), "dataset_a".to_string()),
            ("proj-1".to_string(), "dataset_b".to_string()),
        ]);
        let coverage = check_container_coverage(&db, &workspace_id, &ds, &enumerated)
            .await
            .expect("check_container_coverage");

        assert!(coverage.warning.is_none());
        assert!(!coverage.material);
    }

    /// Reproduces the confirmed incident shape end to end: 4 live datasets,
    /// only 1 enumerated. Must report both a warning naming the missing
    /// three and `material == true`.
    #[tokio::test]
    async fn material_shortfall_end_to_end_matches_confirmed_incident() {
        let db = DbPool::connect("sqlite::memory:")
            .await
            .expect("connect in-memory sqlite");
        let (workspace_id, ds) = {
            let DbPool::Sqlite(sq) = &db else {
                unreachable!("expected sqlite pool");
            };
            seed_container_scoped_fixture(
                sq,
                "covincident",
                &[
                    ("proj-1", "dataset_a", "t1"),
                    ("proj-1", "dataset_b", "t1"),
                    ("proj-1", "dataset_c", "t1"),
                    ("proj-1", "dataset_d", "t1"),
                ],
            )
            .await
        };

        let enumerated = HashSet::from([("proj-1".to_string(), "dataset_a".to_string())]);
        let coverage = check_container_coverage(&db, &workspace_id, &ds, &enumerated)
            .await
            .expect("check_container_coverage");

        assert!(coverage.material, "1 of 4 enumerated must be material");
        let warning = coverage.warning.expect("warning must be present");
        assert!(warning.contains("1 of 4"), "got: {warning}");
        for missing in ["dataset_b", "dataset_c", "dataset_d"] {
            assert!(
                warning.contains(missing),
                "warning must name {missing}, got: {warning}"
            );
        }
    }

    /// A small, non-material shortfall still produces a warning (so the
    /// user can see exactly which container wasn't re-verified) but must
    /// not be flagged `material`.
    #[tokio::test]
    async fn non_material_shortfall_still_warns_but_is_not_material() {
        let rows: Vec<(String, String, String)> = (0..10)
            .map(|i| ("proj-1".to_string(), format!("dataset_{i}"), "t1".to_string()))
            .collect();
        let rows_ref: Vec<(&str, &str, &str)> = rows
            .iter()
            .map(|(p, d, t)| (p.as_str(), d.as_str(), t.as_str()))
            .collect();

        let db = DbPool::connect("sqlite::memory:")
            .await
            .expect("connect in-memory sqlite");
        let (workspace_id, ds) = {
            let DbPool::Sqlite(sq) = &db else {
                unreachable!("expected sqlite pool");
            };
            seed_container_scoped_fixture(sq, "covsmall", &rows_ref).await
        };

        let enumerated: HashSet<ContainerKey> = (0..9)
            .map(|i| ("proj-1".to_string(), format!("dataset_{i}")))
            .collect();
        let coverage = check_container_coverage(&db, &workspace_id, &ds, &enumerated)
            .await
            .expect("check_container_coverage");

        assert!(
            coverage.warning.is_some(),
            "one un-enumerated container out of ten must still warn"
        );
        assert!(
            !coverage.material,
            "9 of 10 enumerated must not be material"
        );
    }

    /// The missing-container list is capped at
    /// `MAX_MISSING_CONTAINERS_IN_WARNING` with a trailing "(+N more)"
    /// suffix, mirroring `fold_table_outcomes`'s summary-line idiom.
    #[tokio::test]
    async fn missing_container_list_is_capped_with_a_remainder_suffix() {
        let total = MAX_MISSING_CONTAINERS_IN_WARNING + 3;
        let rows: Vec<(String, String, String)> = (0..total)
            .map(|i| ("proj-1".to_string(), format!("dataset_{i}"), "t1".to_string()))
            .collect();
        let rows_ref: Vec<(&str, &str, &str)> = rows
            .iter()
            .map(|(p, d, t)| (p.as_str(), d.as_str(), t.as_str()))
            .collect();

        let db = DbPool::connect("sqlite::memory:")
            .await
            .expect("connect in-memory sqlite");
        let (workspace_id, ds) = {
            let DbPool::Sqlite(sq) = &db else {
                unreachable!("expected sqlite pool");
            };
            seed_container_scoped_fixture(sq, "covcapped", &rows_ref).await
        };

        let coverage = check_container_coverage(&db, &workspace_id, &ds, &HashSet::new())
            .await
            .expect("check_container_coverage");

        let warning = coverage.warning.expect("warning must be present");
        assert!(
            warning.contains(&format!("(+{} more)", total - MAX_MISSING_CONTAINERS_IN_WARNING)),
            "got: {warning}"
        );
    }

    // ── apply_container_coverage (KYO-614) ────────────────────────────────

    /// AC4 core logic, exercised without any DB or HTTP dependency: a
    /// material shortfall on an otherwise-`"idle"` run must flip it to
    /// `"failed"` with the shortfall's warning as the reason, and the
    /// warning must also land in `errors`.
    #[test]
    fn material_shortfall_overrides_idle_status() {
        let mut status = "idle";
        let mut failure_reason = None;
        let mut errors = Vec::new();
        let coverage = ContainerCoverage {
            warning: Some("Catalog refresh enumerated 1 of 4 known container(s)".to_string()),
            material: true,
        };

        apply_container_coverage(coverage, &mut status, &mut failure_reason, &mut errors);

        assert_eq!(status, "failed");
        assert_eq!(
            failure_reason,
            Some("Catalog refresh enumerated 1 of 4 known container(s)".to_string())
        );
        assert_eq!(
            errors,
            vec!["Catalog refresh enumerated 1 of 4 known container(s)".to_string()]
        );
    }

    /// A run already `"failed"` for a real, unrelated reason must keep that
    /// reason — a material shortfall must not overwrite a genuine failure
    /// with the generic shortfall message, only add its warning to `errors`.
    #[test]
    fn material_shortfall_does_not_overwrite_an_existing_failure_reason() {
        let mut status = "failed";
        let mut failure_reason = Some("permission denied listing schema 'restricted'".to_string());
        let mut errors = vec!["permission denied listing schema 'restricted'".to_string()];
        let coverage = ContainerCoverage {
            warning: Some("Catalog refresh enumerated 1 of 4 known container(s)".to_string()),
            material: true,
        };

        apply_container_coverage(coverage, &mut status, &mut failure_reason, &mut errors);

        assert_eq!(status, "failed");
        assert_eq!(
            failure_reason,
            Some("permission denied listing schema 'restricted'".to_string()),
            "the original failure reason must survive, not be replaced by the shortfall's"
        );
        assert_eq!(
            errors,
            vec![
                "permission denied listing schema 'restricted'".to_string(),
                "Catalog refresh enumerated 1 of 4 known container(s)".to_string(),
            ],
            "the shortfall warning must still be appended to errors"
        );
    }

    /// A non-material shortfall must still warn (appended to `errors`) but
    /// must leave an `"idle"` status untouched.
    #[test]
    fn non_material_shortfall_warns_without_overriding_status() {
        let mut status = "idle";
        let mut failure_reason = None;
        let mut errors = Vec::new();
        let coverage = ContainerCoverage {
            warning: Some("Catalog refresh enumerated 9 of 10 known container(s)".to_string()),
            material: false,
        };

        apply_container_coverage(coverage, &mut status, &mut failure_reason, &mut errors);

        assert_eq!(status, "idle");
        assert_eq!(failure_reason, None);
        assert_eq!(
            errors,
            vec!["Catalog refresh enumerated 9 of 10 known container(s)".to_string()]
        );
    }

    /// No shortfall at all (`warning: None`) must be a complete no-op.
    #[test]
    fn no_shortfall_is_a_no_op() {
        let mut status = "idle";
        let mut failure_reason = None;
        let mut errors = Vec::new();
        let coverage = ContainerCoverage {
            warning: None,
            material: false,
        };

        apply_container_coverage(coverage, &mut status, &mut failure_reason, &mut errors);

        assert_eq!(status, "idle");
        assert_eq!(failure_reason, None);
        assert!(errors.is_empty());
    }

    // ── reconcile_container_liveness (KYO-622) ────────────────────────────
    //
    // Reuses `seed_container_scoped_fixture`/`archived_state` from the
    // `archive_missing_tables` (KYO-614) section above.

    /// Reads back `(missed_runs, last_seen_at IS NOT NULL)` for one
    /// container's liveness row, or `None` if it has no liveness row at all
    /// (either never seen/missed, or already pruned/deleted).
    async fn read_liveness_row(
        sq: &sqlx::SqlitePool,
        datasource_config_id: &str,
        project_id: &str,
        dataset_id: &str,
    ) -> Option<(i64, bool)> {
        #[derive(sqlx::FromRow)]
        struct Row {
            missed_runs: i64,
            last_seen_at: Option<String>,
        }
        let row: Option<Row> = sqlx::query_as(
            "SELECT missed_runs, last_seen_at FROM datasource_container_cache \
             WHERE datasource_config_id = ? AND project_id = ? AND dataset_id = ?",
        )
        .bind(datasource_config_id)
        .bind(project_id)
        .bind(dataset_id)
        .fetch_optional(sq)
        .await
        .expect("read liveness row");
        row.map(|r| (r.missed_runs, r.last_seen_at.is_some()))
    }

    /// **The most important test in this change (AC2).** A container that is
    /// absent from `enumerated_containers` but whose run is *incomplete*
    /// looks, from the archiver's point of view, exactly like a genuinely
    /// deleted container — the only difference is the `run_complete` flag.
    /// This asserts the flag is actually load-bearing: no matter how many
    /// incomplete runs pass (looped well past the archive threshold), the
    /// container's rows must never be archived, because an incomplete run
    /// can never prove the container is actually gone.
    #[tokio::test]
    async fn an_unreachable_container_is_never_archived_however_many_incomplete_runs_pass() {
        let db = DbPool::connect("sqlite::memory:")
            .await
            .expect("connect in-memory sqlite");
        let (workspace_id, ds) = {
            let DbPool::Sqlite(sq) = &db else {
                unreachable!("expected sqlite pool");
            };
            seed_container_scoped_fixture(sq, "unreachable", &[("proj-1", "dataset_a", "t1")])
                .await
        };

        // Never enumerated, and every run that doesn't enumerate it is
        // incomplete — this container is simply unreachable this run.
        let enumerated = HashSet::new();

        for i in 0..(MISSED_COMPLETE_RUNS_BEFORE_ARCHIVE * 3) {
            let outcome =
                reconcile_container_liveness(&db, &workspace_id, &ds, &enumerated, false)
                    .await
                    .unwrap_or_else(|e| panic!("reconcile_container_liveness (iteration {i}): {e}"));
            assert!(
                outcome.archived_containers.is_empty(),
                "iteration {i}: an incomplete run must never archive anything"
            );
            assert!(outcome.archived_tables.is_empty());
        }

        let DbPool::Sqlite(sq) = &db else {
            unreachable!("expected sqlite pool");
        };
        assert!(
            !archived_state(sq, &ds, "proj-1", "dataset_a", "t1").await,
            "an unreachable container must never be archived, however many \
             incomplete runs pass"
        );
    }

    /// A `run_complete = false` call is a strict no-op, not merely one that
    /// declines to archive — `missed_runs` itself must not move.
    #[tokio::test]
    async fn incomplete_run_leaves_missed_runs_unchanged() {
        let db = DbPool::connect("sqlite::memory:")
            .await
            .expect("connect in-memory sqlite");
        let (workspace_id, ds) = {
            let DbPool::Sqlite(sq) = &db else {
                unreachable!("expected sqlite pool");
            };
            seed_container_scoped_fixture(sq, "noopstreak", &[("proj-1", "dataset_a", "t1")]).await
        };
        let enumerated = HashSet::new();

        // One real complete miss establishes missed_runs = 1.
        reconcile_container_liveness(&db, &workspace_id, &ds, &enumerated, true)
            .await
            .expect("reconcile (complete)");

        let DbPool::Sqlite(sq) = &db else {
            unreachable!("expected sqlite pool");
        };
        assert_eq!(
            read_liveness_row(sq, &ds, "proj-1", "dataset_a").await,
            Some((1, false))
        );

        // Several incomplete runs must not advance it any further.
        for _ in 0..5 {
            reconcile_container_liveness(&db, &workspace_id, &ds, &enumerated, false)
                .await
                .expect("reconcile (incomplete)");
        }

        assert_eq!(
            read_liveness_row(sq, &ds, "proj-1", "dataset_a").await,
            Some((1, false)),
            "an incomplete run must leave missed_runs exactly as a complete run left it"
        );
    }

    /// A genuinely deleted container is archived, and only after EXACTLY
    /// `MISSED_COMPLETE_RUNS_BEFORE_ARCHIVE` consecutive complete runs never
    /// enumerated it — the boundary matters as much as the direction.
    #[tokio::test]
    async fn genuinely_deleted_container_archives_after_exactly_the_threshold() {
        let db = DbPool::connect("sqlite::memory:")
            .await
            .expect("connect in-memory sqlite");
        let (workspace_id, ds) = {
            let DbPool::Sqlite(sq) = &db else {
                unreachable!("expected sqlite pool");
            };
            seed_container_scoped_fixture(sq, "boundary", &[("proj-1", "dataset_a", "t1")]).await
        };
        let enumerated = HashSet::new();

        // MISSED_COMPLETE_RUNS_BEFORE_ARCHIVE - 1 complete misses: must not
        // archive yet.
        for i in 0..(MISSED_COMPLETE_RUNS_BEFORE_ARCHIVE - 1) {
            let outcome = reconcile_container_liveness(&db, &workspace_id, &ds, &enumerated, true)
                .await
                .unwrap_or_else(|e| panic!("reconcile (miss {i}): {e}"));
            assert!(
                outcome.archived_containers.is_empty(),
                "must not archive before reaching the threshold (miss {i})"
            );
        }
        {
            let DbPool::Sqlite(sq) = &db else {
                unreachable!("expected sqlite pool");
            };
            assert!(
                !archived_state(sq, &ds, "proj-1", "dataset_a", "t1").await,
                "must still be unarchived at threshold - 1"
            );
            assert_eq!(
                read_liveness_row(sq, &ds, "proj-1", "dataset_a").await,
                Some((MISSED_COMPLETE_RUNS_BEFORE_ARCHIVE - 1, false))
            );
        }

        // The Nth complete miss reaches the threshold and archives.
        let outcome = reconcile_container_liveness(&db, &workspace_id, &ds, &enumerated, true)
            .await
            .expect("reconcile (threshold miss)");
        assert_eq!(
            outcome.archived_containers,
            vec![("proj-1".to_string(), "dataset_a".to_string())]
        );
        assert_eq!(outcome.archived_tables, vec!["proj-1.dataset_a.t1".to_string()]);

        let DbPool::Sqlite(sq) = &db else {
            unreachable!("expected sqlite pool");
        };
        assert!(archived_state(sq, &ds, "proj-1", "dataset_a", "t1").await);
        assert_eq!(
            read_liveness_row(sq, &ds, "proj-1", "dataset_a").await,
            None,
            "the liveness row must be deleted once its container is archived, \
             so a later reappearance starts clean"
        );
    }

    /// `missed_runs` resets to 0 the moment a container reappears — a gap
    /// followed by a sighting must not leave any residue that a later gap
    /// could build on.
    #[tokio::test]
    async fn missed_runs_resets_on_reappearance_and_does_not_accumulate_across_a_gap() {
        let db = DbPool::connect("sqlite::memory:")
            .await
            .expect("connect in-memory sqlite");
        let (workspace_id, ds) = {
            let DbPool::Sqlite(sq) = &db else {
                unreachable!("expected sqlite pool");
            };
            seed_container_scoped_fixture(sq, "resetgap", &[("proj-1", "dataset_a", "t1")]).await
        };
        let absent = HashSet::new();
        let present =
            HashSet::from([("proj-1".to_string(), "dataset_a".to_string())]);

        // Two complete misses.
        for _ in 0..2 {
            reconcile_container_liveness(&db, &workspace_id, &ds, &absent, true)
                .await
                .expect("reconcile (miss)");
        }
        {
            let DbPool::Sqlite(sq) = &db else {
                unreachable!("expected sqlite pool");
            };
            assert_eq!(
                read_liveness_row(sq, &ds, "proj-1", "dataset_a").await,
                Some((2, false))
            );
        }

        // It reappears — streak must reset to 0.
        reconcile_container_liveness(&db, &workspace_id, &ds, &present, true)
            .await
            .expect("reconcile (seen)");
        {
            let DbPool::Sqlite(sq) = &db else {
                unreachable!("expected sqlite pool");
            };
            assert_eq!(
                read_liveness_row(sq, &ds, "proj-1", "dataset_a").await,
                Some((0, true)),
                "reappearing must reset missed_runs to 0 and stamp last_seen_at"
            );
        }

        // Two more misses after the gap — if the reset didn't really
        // happen, this would already be at the threshold (2 + 2 = 4 >= 3).
        // It must instead read as only 2.
        for _ in 0..2 {
            let outcome = reconcile_container_liveness(&db, &workspace_id, &ds, &absent, true)
                .await
                .expect("reconcile (miss after gap)");
            assert!(
                outcome.archived_containers.is_empty(),
                "the reset must not let misses accumulate across the gap"
            );
        }
        let DbPool::Sqlite(sq) = &db else {
            unreachable!("expected sqlite pool");
        };
        assert_eq!(
            read_liveness_row(sq, &ds, "proj-1", "dataset_a").await,
            Some((2, true)),
            "missed_runs must reflect only the misses since the reappearance"
        );
        assert!(!archived_state(sq, &ds, "proj-1", "dataset_a", "t1").await);
    }

    /// Only the deleted container's rows are archived — a sibling container
    /// in the same datasource that keeps being enumerated every run must be
    /// completely untouched.
    #[tokio::test]
    async fn sibling_live_container_is_untouched_by_a_neighbors_archival() {
        let db = DbPool::connect("sqlite::memory:")
            .await
            .expect("connect in-memory sqlite");
        let (workspace_id, ds) = {
            let DbPool::Sqlite(sq) = &db else {
                unreachable!("expected sqlite pool");
            };
            seed_container_scoped_fixture(
                sq,
                "sibling",
                &[("proj-1", "dataset_deleted", "t1"), ("proj-1", "dataset_live", "t1")],
            )
            .await
        };
        let live_only = HashSet::from([("proj-1".to_string(), "dataset_live".to_string())]);

        let mut last_outcome = None;
        for _ in 0..MISSED_COMPLETE_RUNS_BEFORE_ARCHIVE {
            last_outcome = Some(
                reconcile_container_liveness(&db, &workspace_id, &ds, &live_only, true)
                    .await
                    .expect("reconcile"),
            );
        }
        let outcome = last_outcome.expect("at least one run happened");

        assert_eq!(
            outcome.archived_containers,
            vec![("proj-1".to_string(), "dataset_deleted".to_string())]
        );
        assert_eq!(outcome.archived_tables, vec!["proj-1.dataset_deleted.t1".to_string()]);

        let DbPool::Sqlite(sq) = &db else {
            unreachable!("expected sqlite pool");
        };
        assert!(archived_state(sq, &ds, "proj-1", "dataset_deleted", "t1").await);
        assert!(
            !archived_state(sq, &ds, "proj-1", "dataset_live", "t1").await,
            "the live sibling container must be completely untouched"
        );
        assert_eq!(
            read_liveness_row(sq, &ds, "proj-1", "dataset_live").await,
            Some((0, true)),
            "the live container's own liveness row must reflect it being seen every run"
        );
    }

    /// AC3 end to end: once the liveness GC archives a genuinely deleted
    /// container, `check_container_coverage`'s live-container denominator
    /// must drop to match — the whole point of running the GC before the
    /// coverage check in the same run.
    #[tokio::test]
    async fn archiving_a_deleted_container_drops_the_live_count_check_container_coverage_uses() {
        let db = DbPool::connect("sqlite::memory:")
            .await
            .expect("connect in-memory sqlite");
        let (workspace_id, ds) = {
            let DbPool::Sqlite(sq) = &db else {
                unreachable!("expected sqlite pool");
            };
            seed_container_scoped_fixture(
                sq,
                "ac3",
                &[("proj-1", "dataset_a", "t1"), ("proj-1", "dataset_b", "t1")],
            )
            .await
        };
        let enumerated_b_only = HashSet::from([("proj-1".to_string(), "dataset_b".to_string())]);

        // BEFORE the GC has ever run: both containers still count as live —
        // dataset_a's row hasn't been reclaimed yet, so coverage sees 2.
        let before = check_container_coverage(&db, &workspace_id, &ds, &enumerated_b_only)
            .await
            .expect("check_container_coverage (before)");
        assert!(
            before.warning.as_deref().is_some_and(|w| w.contains("1 of 2")),
            "got: {:?}",
            before.warning
        );

        // Run the GC enough complete times for dataset_a to cross the
        // threshold and be reclaimed.
        for _ in 0..MISSED_COMPLETE_RUNS_BEFORE_ARCHIVE {
            reconcile_container_liveness(&db, &workspace_id, &ds, &enumerated_b_only, true)
                .await
                .expect("reconcile");
        }

        // AFTER: dataset_a's row is archived, so it drops out of coverage's
        // live count entirely — full coverage with dataset_b alone.
        let after = check_container_coverage(&db, &workspace_id, &ds, &enumerated_b_only)
            .await
            .expect("check_container_coverage (after)");
        assert!(
            after.warning.is_none(),
            "the reclaimed container must drop out of the live-container count \
             entirely, got: {:?}",
            after.warning
        );
        assert!(!after.material);
    }
}
