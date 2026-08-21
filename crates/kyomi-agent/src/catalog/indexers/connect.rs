// SPDX-License-Identifier: AGPL-3.0-or-later

//! Connect catalog indexer.
//!
//! Uses the Kyomi Connect binary's `discover_catalog` command to enumerate
//! schemas, tables, and columns from databases behind firewalls or VPNs.
//! The Connect binary does the heavy lifting — this indexer just maps the
//! result into the shared caching pipeline.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use kyomi_core::DbPool;
use kyomi_embed::EmbeddingService;
use serde_json::Value;
use std::collections::HashSet;
use tracing::{info, warn};

use crate::catalog::traits::CatalogIndexer;
use kyomi_auth::catalog::helpers::{
    archive_missing_tables, cache_table, resolve_run_outcome, update_datasource_last_refresh,
    update_datasource_status, CacheTableParams, IndexerContext,
};
use kyomi_auth::catalog::types::{CatalogIndexResult, ColumnEntry};
use kyomi_core::connect_protocol::{CatalogResult, DiscoverCatalogParams};
use kyomi_datasource_server::ConnectRegistry;

/// `connection_config` keys under which a Connect datasource's selected
/// containers may be stored, depending on its underlying datasource type
/// (postgres/redshift/… → `catalog_schemas`, mysql/clickhouse/snowflake →
/// `catalog_databases`, databricks → `catalog_catalogs`). The UI writes exactly
/// one of these via `catalog_config_key_for_type`, so the indexer reads
/// whichever is present. Mirrors the direct path's `container_config_key()`.
const CONNECT_CONTAINER_KEYS: &[&str] =
    &["catalog_schemas", "catalog_databases", "catalog_catalogs"];

/// The set of containers a Connect refresh should index.
#[derive(Debug, Clone, PartialEq)]
enum ContainerScope {
    /// No scope configured — index every container the agent can see.
    All,
    /// Index only these containers. An empty list means "index nothing"
    /// (the user explicitly cleared the selection).
    Only(Vec<String>),
}

/// Resolve the container scope from a Connect datasource's `connection_config`.
///
/// Returns [`ContainerScope::All`] when no container key is set, is null, or
/// holds an unexpected type; otherwise [`ContainerScope::Only`] with the listed
/// names (possibly empty). Mirrors `get_catalog_containers`' handling on the
/// direct path so the two paths agree on what a given config means.
fn connect_container_scope(connection_config: &Value) -> ContainerScope {
    for key in CONNECT_CONTAINER_KEYS {
        match connection_config.get(*key) {
            Some(Value::Array(arr)) => {
                let names = arr
                    .iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect();
                return ContainerScope::Only(names);
            }
            // Absent, null, or an unexpected type for this key → keep looking.
            _ => continue,
        }
    }
    ContainerScope::All
}

/// Defensively drop any container the agent returned that isn't in `scope`.
///
/// The agent-side filter already honors the scope, but an un-upgraded agent
/// ignores it and returns everything — this client-side pass guarantees a
/// scoped refresh only ever caches the selected containers regardless of the
/// agent's version. A `None` scope (index all) leaves the result untouched.
///
/// Only `containers` is filtered — `errors` (the per-container/per-table
/// discovery failures the agent recorded, kyomi-connect-protocol 1.4.1) is
/// passed through untouched. Those errors describe containers that failed
/// *during discovery*, before they could ever appear here; scope filtering
/// has nothing to say about them, and dropping them would silently defeat
/// `process_discovered_catalog`'s `resolve_final_status` decision.
fn filter_catalog_to_scope(mut catalog: CatalogResult, scope: Option<&[String]>) -> CatalogResult {
    if let Some(scope) = scope {
        catalog
            .containers
            .retain(|c| scope.iter().any(|s| s.eq_ignore_ascii_case(&c.name)));
    }
    catalog
}

pub struct ConnectIndexer {
    registry: ConnectRegistry,
}

impl ConnectIndexer {
    pub fn new(registry: ConnectRegistry) -> Self {
        Self { registry }
    }
}

#[async_trait]
impl CatalogIndexer for ConnectIndexer {
    async fn index_catalog(
        &self,
        ctx: &IndexerContext,
        db: &DbPool,
        embedding: &EmbeddingService,
        _user_email: Option<&str>,
        _credentials: Option<&Value>,
        _max_tables_per_dataset: Option<usize>,
    ) -> CatalogIndexResult {
        let start_time = Utc::now();

        let provider = kyomi_datasource_server::ConnectProvider::with_timeout(
            self.registry.clone(),
            ctx.datasource_config_id.clone(),
            std::time::Duration::from_secs(120),
        );

        // Test connection first.
        use kyomi_datasource_server::provider::DatasourceProvider as _;
        if let Err(e) = provider.test_connection().await {
            warn!(
                datasource_config_id = ctx.datasource_config_id,
                error = %e,
                "Connection test failed during Connect catalog indexing"
            );
            let reason = "Connection test failed — is the Connect binary running?";
            let _ = update_datasource_status(
                db, &ctx.workspace_id, &ctx.datasource_config_id, "failed", None, Some(reason),
                &[],
            ).await;
            return CatalogIndexResult::error(reason)
                .with_times(&start_time.to_rfc3339(), &Utc::now().to_rfc3339())
                .with_ids(&ctx.datasource_config_id, &ctx.workspace_id);
        }

        let _ = update_datasource_status(
            db, &ctx.workspace_id, &ctx.datasource_config_id, "running", None, None, &[],
        ).await;

        // Resolve the configured container scope (KYO-162). An explicit empty
        // selection means "index nothing": skip discovery entirely and let the
        // archival pass remove any previously-cached tables.
        let scope = connect_container_scope(&ctx.connection_config);
        let explicit_empty = matches!(&scope, ContainerScope::Only(v) if v.is_empty());

        let catalog_result = if explicit_empty {
            info!(
                datasource_config_id = ctx.datasource_config_id,
                "no containers selected for Connect indexing — archiving existing catalog"
            );
            CatalogResult {
                containers: Vec::new(),
                errors: Vec::new(),
            }
        } else {
            let scoped_containers = match &scope {
                ContainerScope::Only(names) => Some(names.clone()),
                ContainerScope::All => None,
            };
            let params = DiscoverCatalogParams {
                containers: scoped_containers.clone(),
                include_public_datasets: None,
                containers_only: false,
            };
            match provider.discover_catalog(params).await {
                // Defense-in-depth: filter client-side too, so scope is honored
                // even against an agent that predates the wire-protocol change.
                Ok(cr) => filter_catalog_to_scope(cr, scoped_containers.as_deref()),
                Err(e) => {
                    warn!(
                        datasource_config_id = ctx.datasource_config_id,
                        error = %e,
                        "discover_catalog command failed"
                    );
                    let msg = format!("Catalog discovery failed: {e}");
                    let _ = update_datasource_status(
                        db, &ctx.workspace_id, &ctx.datasource_config_id, "failed", None,
                        Some(&msg), &[],
                    ).await;
                    return CatalogIndexResult::error(&msg)
                        .with_times(&start_time.to_rfc3339(), &Utc::now().to_rfc3339())
                        .with_ids(&ctx.datasource_config_id, &ctx.workspace_id);
                }
            }
        };

        process_discovered_catalog(ProcessDiscoveredCatalogParams {
            db,
            embedding,
            ctx,
            catalog_result,
            explicit_empty,
            start_time,
        })
        .await
    }
}

/// Parameters for [`process_discovered_catalog`].
struct ProcessDiscoveredCatalogParams<'a> {
    db: &'a DbPool,
    embedding: &'a EmbeddingService,
    ctx: &'a IndexerContext,
    catalog_result: CatalogResult,
    explicit_empty: bool,
    start_time: DateTime<Utc>,
}

/// Cache the tables in a discovered Connect catalog, archive whatever's no
/// longer present, and decide the run's final datasource status.
///
/// Split out from [`ConnectIndexer::index_catalog`] so this logic — which
/// includes the KYO-126/KYO-385 `resolve_run_outcome` decision below — is
/// unit-testable against a plain [`CatalogResult`] value and an in-memory DB, without
/// needing a live Connect WebSocket connection through [`ConnectRegistry`].
/// `discover_catalog` itself (and its `Err` handling, which already maps a
/// discovery failure to `"failed"` with the agent's real message) stays in
/// `index_catalog` — this function only runs once discovery has already
/// succeeded.
async fn process_discovered_catalog(params: ProcessDiscoveredCatalogParams<'_>) -> CatalogIndexResult {
    let ProcessDiscoveredCatalogParams {
        db,
        embedding,
        ctx,
        mut catalog_result,
        explicit_empty,
        start_time,
    } = params;

    let total_tables: usize = catalog_result.containers.iter().map(|c| c.tables.len()).sum();
    info!(
        datasource_config_id = ctx.datasource_config_id,
        containers = catalog_result.containers.len(),
        total_tables,
        "Connect catalog discovered"
    );

    let mut tables_indexed = 0usize;
    let mut seen_table_ids = HashSet::new();

    for container in &catalog_result.containers {
        for table in &container.tables {
            let columns: Vec<ColumnEntry> = table
                .columns
                .iter()
                .map(|col| ColumnEntry {
                    name: col.name.clone(),
                    col_type: Some(col.native_type.clone()),
                    native_type: Some(col.native_type.clone()),
                    description: col.description.clone(),
                })
                .collect();

            let project_id = "";
            let dataset_id = container.name.as_str();
            let table_name = table.name.as_str();
            let table_type = table.native_type.as_deref().unwrap_or("TABLE");
            let full_table_id = format!("{}.{}", container.name, table.name);
            let archive_id = kyomi_core::build_full_table_name(project_id, dataset_id, table_name);
            seen_table_ids.insert(archive_id);

            match cache_table(CacheTableParams {
                db,
                embedding,
                ctx,
                project_id,
                dataset_id,
                table_name,
                table_type,
                columns: &columns,
                full_table_id: &full_table_id,
            })
            .await
            {
                Ok(()) => tables_indexed += 1,
                Err(e) => {
                    let msg = format!("Failed to cache table {full_table_id}: {e}");
                    warn!("{msg}");
                    catalog_result.errors.push(msg);
                }
            }
        }
    }

    // Resolve the archive/status decision (KYO-385). These are two
    // different questions and must not be answered by one predicate:
    //
    // - Archiving keys off "was anything *listed*" (`seen_table_ids`) — a
    //   listed-but-unwritable table (schema read OK, `cache_table` write
    //   failed) demonstrably exists and must not be evicted.
    // - Status keys off "was anything *usable*" (`tables_indexed`).
    //   Conflating the two — as the single `nothing_found` predicate this
    //   replaces did — is the KYO-385 bug: `seen_table_ids` is populated
    //   before `cache_table` runs, so a run where every table listed fine
    //   but every write failed always looked like `nothing_found == false`
    //   and silently reported `"idle"`, no matter how many
    //   `catalog_result.errors` accumulated.
    //
    // As on the direct SQL path, `explicit_empty` is the one exception: the
    // user intentionally cleared the container selection, so an empty
    // result is expected — archive the existing catalog (stale tables
    // should be evicted, not preserved) and never report a failure for a
    // run that found nothing because nothing was ever selected. See
    // `resolve_run_outcome`'s doc comment for the full reasoning.
    let outcome = resolve_run_outcome(
        !seen_table_ids.is_empty(),
        tables_indexed,
        &catalog_result.errors,
        explicit_empty,
    );

    let tables_archived = if outcome.archive {
        archive_missing_tables(
            db,
            &ctx.workspace_id,
            &ctx.datasource_config_id,
            &seen_table_ids,
        )
        .await
        .unwrap_or_default()
        .len()
    } else {
        warn!(
            datasource_config_id = ctx.datasource_config_id,
            "No tables found via Connect — preserving existing catalog"
        );
        0
    };

    let _ = update_datasource_last_refresh(db, &ctx.datasource_config_id).await;

    // KYO-268 phase 2: a discovery denial no longer aborts the whole crawl
    // (kyomi-connect-protocol 1.4.1, kyomi-connect PR #17) — one container
    // or table failing is recorded in `catalog_result.errors` and simply
    // omitted from `containers`, while `discover_catalog` still returns
    // `Ok` and the rest of the crawl proceeds normally. A *total* denial is
    // unaffected by this and still unchanged: it surfaces as a real `Err`
    // from `discover_catalog`, caught above in `index_catalog`'s discovery
    // match arm, and this function is never reached in that case.
    //
    // `outcome.status` (via `resolve_final_status`, shared with the direct
    // SQL path) is what tells the partial-denial cases apart: zero tables
    // usable with at least one recorded error maps to `"failed"` with a
    // reason built from that error; zero tables usable with no errors is a
    // genuinely empty, fully-accessible catalog and still reports `"idle"`
    // (the original KYO-126 case this path was fixed for). A non-empty
    // result (some tables actually indexed) always reports `"idle"`
    // regardless of `errors` — a 9-of-10-accessible datasource must not
    // carry a permanent red alert over one denied container. Those errors
    // aren't dropped in that case; they're carried on
    // `CatalogIndexResult::errors` below instead.
    let _ = update_datasource_status(
        db,
        &ctx.workspace_id,
        &ctx.datasource_config_id,
        outcome.status,
        None,
        outcome.failure_reason.as_deref(),
        &catalog_result.errors,
    )
    .await;

    let end_time = Utc::now();

    info!(
        datasource_config_id = ctx.datasource_config_id,
        tables_indexed,
        tables_archived,
        archived = outcome.archive,
        status = outcome.status,
        discovery_errors = catalog_result.errors.len(),
        elapsed_secs = (end_time - start_time).num_seconds(),
        "Connect catalog indexing complete"
    );

    let mut result = CatalogIndexResult::completed(tables_indexed, tables_archived)
        .with_times(&start_time.to_rfc3339(), &end_time.to_rfc3339())
        .with_ids(&ctx.datasource_config_id, &ctx.workspace_id);

    // Surface partial-discovery failures on the result even when the run
    // otherwise succeeded (KYO-268 acceptance criterion 1: a denial must be
    // "surfaced as a visible, attributable error reason", not just silently
    // dropped once tables were found elsewhere). Mirrors the direct SQL
    // path (`index_catalog_sql`), which does the same regardless of status.
    //
    // KYO-327: these same errors are also passed to `update_datasource_status`
    // above as the persisted `warnings` array (on both the `"idle"` and
    // `"failed"` outcomes `resolve_final_status` can produce), which is what
    // the settings page's refresh poller and its persistent Catalog-tab
    // notice actually render. `CatalogIndexResult::errors` set here reaches
    // logs and any direct in-process caller of this return value — it is a
    // separate channel from the persisted envelope, not the one the UI reads.
    if !catalog_result.errors.is_empty() {
        result.errors = Some(catalog_result.errors);
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use kyomi_core::connect_protocol::CatalogContainer;
    use serde_json::json;

    #[test]
    fn connect_indexer_requires_registry() {
        // ConnectIndexer can only be created with a registry — no Default.
        // This test verifies the struct exists and fields are correct.
        let _: fn(ConnectRegistry) -> ConnectIndexer = ConnectIndexer::new;
    }

    // ── connect_container_scope (KYO-162) ────────────────────────────────

    #[test]
    fn scope_absent_is_all() {
        assert_eq!(connect_container_scope(&json!({})), ContainerScope::All);
    }

    #[test]
    fn scope_null_is_all() {
        assert_eq!(
            connect_container_scope(&json!({ "catalog_schemas": null })),
            ContainerScope::All
        );
    }

    #[test]
    fn scope_reads_catalog_schemas() {
        assert_eq!(
            connect_container_scope(&json!({ "catalog_schemas": ["public", "analytics"] })),
            ContainerScope::Only(vec!["public".into(), "analytics".into()])
        );
    }

    #[test]
    fn scope_reads_catalog_databases_for_non_schema_types() {
        // A Connect MySQL/ClickHouse datasource stores its selection under
        // `catalog_databases`; the indexer must pick it up too.
        assert_eq!(
            connect_container_scope(&json!({ "catalog_databases": ["shop"] })),
            ContainerScope::Only(vec!["shop".into()])
        );
    }

    #[test]
    fn scope_empty_array_is_index_nothing() {
        assert_eq!(
            connect_container_scope(&json!({ "catalog_schemas": [] })),
            ContainerScope::Only(vec![])
        );
    }

    // ── filter_catalog_to_scope (KYO-162) ────────────────────────────────

    fn catalog(names: &[&str]) -> CatalogResult {
        CatalogResult {
            containers: names
                .iter()
                .map(|n| CatalogContainer {
                    name: (*n).to_string(),
                    tables: Vec::new(),
                })
                .collect(),
            errors: Vec::new(),
        }
    }

    fn container_names(c: &CatalogResult) -> Vec<String> {
        c.containers.iter().map(|c| c.name.clone()).collect()
    }

    #[test]
    fn filter_none_scope_keeps_everything() {
        let c = filter_catalog_to_scope(catalog(&["public", "staging"]), None);
        assert_eq!(container_names(&c), vec!["public", "staging"]);
    }

    #[test]
    fn filter_keeps_only_scoped_case_insensitive() {
        let scope = vec!["PUBLIC".to_string()];
        let c = filter_catalog_to_scope(catalog(&["public", "staging"]), Some(&scope));
        assert_eq!(container_names(&c), vec!["public"]);
    }

    #[test]
    fn filter_drops_all_when_scope_matches_nothing() {
        let scope = vec!["missing".to_string()];
        let c = filter_catalog_to_scope(catalog(&["public"]), Some(&scope));
        assert!(c.containers.is_empty());
    }

    /// KYO-268 phase 2: scope filtering must never drop the discovery
    /// errors the agent recorded on the way in — those describe containers
    /// that failed *before* this function ever sees them, so scope has
    /// nothing to say about them. Dropping them here would silently defeat
    /// `process_discovered_catalog`'s `resolve_final_status` decision below.
    #[test]
    fn filter_preserves_errors_untouched() {
        let mut c = catalog(&["public", "staging"]);
        c.errors = vec!["container 'restricted' permission denied".to_string()];
        let scope = vec!["public".to_string()];

        let filtered = filter_catalog_to_scope(c, Some(&scope));

        assert_eq!(container_names(&filtered), vec!["public"]);
        assert_eq!(
            filtered.errors,
            vec!["container 'restricted' permission denied".to_string()],
            "filter_catalog_to_scope must pass errors through untouched"
        );
    }

    // ── process_discovered_catalog (KYO-126, KYO-268 phase 2) ──────────────
    //
    // A *total* permission denial on the Connect path is still surfaced by
    // `discover_catalog` returning a real `Err` (kyomi-connect PR #16 made
    // denials surface as `Err` at all; PR #17 narrowed that to "every
    // container failed") — handled, unchanged, in `index_catalog`'s
    // discovery match arm, and not exercised here since it requires a live
    // Connect WebSocket connection through `ConnectRegistry` (Redis-backed),
    // not a plain value these tests can construct.
    //
    // What's new here (KYO-268 phase 2, kyomi-connect-protocol 1.4.1): a
    // *partial* denial — one container or table failing while others
    // succeed — no longer aborts discovery at all. It arrives as `Ok` with
    // the failure recorded in `CatalogResult::errors` and the failing
    // container/table simply absent from `containers`. These tests
    // construct that shape directly and lock in `process_discovered_catalog`
    // folding it into `resolve_final_status`:
    // - tables found elsewhere + errors -> `"idle"`, errors preserved on the
    //   returned `CatalogIndexResult`
    // - zero tables + errors -> `"failed"`, reason drawn from the real error
    // - zero tables + no errors -> `"idle"` (the original KYO-126 case)

    async fn seed_connect_fixture(sq: &sqlx::SqlitePool, suffix: &str) -> IndexerContext {
        let user_id = format!("u-connect-{suffix}");
        let workspace_id = format!("ws-connect-{suffix}");
        let datasource_config_id = format!("ds-connect-{suffix}");

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
             VALUES (?, ?, 'Connect DS', 'postgres', ?)",
        )
        .bind(&datasource_config_id)
        .bind(&workspace_id)
        .bind(format!("connect-{suffix}"))
        .execute(sq)
        .await
        .expect("insert datasource_config");

        IndexerContext {
            workspace_id,
            datasource_config_id,
            connection_config: json!({}),
            encryption_key: std::sync::Arc::new([0u8; 32]),
        }
    }

    async fn datasource_status(sq: &sqlx::SqlitePool, datasource_config_id: &str) -> String {
        sqlx::query_scalar("SELECT catalog_refresh_status FROM datasource_configs WHERE id = ?")
            .bind(datasource_config_id)
            .fetch_one(sq)
            .await
            .expect("read datasource status")
    }

    /// Read back the full persisted `catalog_refresh_progress` envelope as
    /// parsed JSON (KYO-327), so tests can assert on the structured
    /// `"warnings"` array rather than the `"error"` string.
    async fn datasource_progress_envelope(
        sq: &sqlx::SqlitePool,
        datasource_config_id: &str,
    ) -> Value {
        let raw: String = sqlx::query_scalar(
            "SELECT catalog_refresh_progress FROM datasource_configs WHERE id = ?",
        )
        .bind(datasource_config_id)
        .fetch_one(sq)
        .await
        .expect("read progress envelope");
        serde_json::from_str(&raw).expect("progress envelope must be valid JSON")
    }

    /// KYO-126, second pass: a Connect catalog that discovers zero tables
    /// (not an explicit empty selection) must report `"idle"`, not
    /// `"failed"`. Before this fix, `connect_resolve_status` mapped this
    /// unconditionally to `"failed"` — indistinguishable from a real
    /// permission error, which now has its own `Err` path with a real
    /// message instead.
    #[tokio::test]
    async fn nothing_found_reports_idle_not_failed() {
        let db = DbPool::connect("sqlite::memory:")
            .await
            .expect("connect in-memory sqlite");
        let DbPool::Sqlite(sq) = &db else {
            unreachable!("expected sqlite pool");
        };
        let ctx = seed_connect_fixture(sq, "empty").await;
        let embedding = EmbeddingService::new().expect("load embedding model");

        let result = process_discovered_catalog(ProcessDiscoveredCatalogParams {
            db: &db,
            embedding: &embedding,
            ctx: &ctx,
            catalog_result: CatalogResult {
                containers: Vec::new(),
                errors: Vec::new(),
            },
            explicit_empty: false,
            start_time: Utc::now(),
        })
        .await;

        assert_eq!(result.status, "completed");
        assert_eq!(result.tables_indexed, 0);
        assert!(
            result.errors.is_none(),
            "no discovery errors were recorded, so none should appear on the result"
        );

        assert_eq!(
            datasource_status(sq, &ctx.datasource_config_id).await,
            "idle",
            "a genuinely empty (but reachable) Connect catalog must not be reported as failed"
        );
    }

    /// AC2 headline (KYO-385): a table is discovered and its schema read
    /// fine, but the `cache_table` write itself fails for every table this
    /// run — on this in-memory SQLite pool, that's *every* table with at
    /// least one column, since `store_search_embeddings` unconditionally
    /// errors (pgvector unsupported). Zero discovery errors here, so this
    /// is the cleanest form of the bug: no denied container muddying
    /// whether the zero-tables outcome came from discovery or from caching.
    ///
    /// Before KYO-385 this asserted `"idle"` — the test's original name
    /// (`tables_found_are_cached_and_reports_idle`) claimed the table "gets
    /// cached", but `cache_table` returns `Err` here (see the cached-row
    /// assertion below for what actually happens), so `tables_indexed`
    /// stays `0` and the run silently reported a healthy datasource with an
    /// empty catalog. `seen_table_ids` (non-empty — `orders` was listed)
    /// masked that from the old single `nothing_found` predicate; the
    /// status decision must follow `tables_indexed`, not `seen_table_ids`.
    #[tokio::test]
    async fn all_tables_listed_all_writes_failed_resolves_to_failed() {
        let db = DbPool::connect("sqlite::memory:")
            .await
            .expect("connect in-memory sqlite");
        let DbPool::Sqlite(sq) = &db else {
            unreachable!("expected sqlite pool");
        };
        let ctx = seed_connect_fixture(sq, "withtables").await;
        let embedding = EmbeddingService::new().expect("load embedding model");

        let catalog_result = CatalogResult {
            containers: vec![kyomi_core::connect_protocol::CatalogContainer {
                name: "public".to_string(),
                tables: vec![kyomi_core::connect_protocol::CatalogTable {
                    name: "orders".to_string(),
                    native_type: Some("BASE TABLE".to_string()),
                    columns: vec![kyomi_core::connect_protocol::CatalogColumn {
                        name: "id".to_string(),
                        native_type: "int4".to_string(),
                        description: None,
                    }],
                }],
            }],
            errors: Vec::new(),
        };

        let result = process_discovered_catalog(ProcessDiscoveredCatalogParams {
            db: &db,
            embedding: &embedding,
            ctx: &ctx,
            catalog_result,
            explicit_empty: false,
            start_time: Utc::now(),
        })
        .await;

        // `CatalogIndexResult::status` is always `"completed"` on this path
        // (unlike the SQL path, `process_discovered_catalog` has no
        // early-return-`error()` branch) — the persisted
        // `catalog_refresh_status` column below is what the settings page
        // badge and `get_catalog_stats` actually read, and is what this
        // test exists to pin.
        assert_eq!(result.status, "completed");
        assert_eq!(
            result.errors.as_deref().map(<[String]>::len),
            Some(1),
            "the cache_table write failure must reach the caller-visible result"
        );

        // The cache row IS written — `cache_table`'s INSERT commits before
        // it ever calls `store_search_embeddings` — so `tables_indexed`
        // (which only increments on a full `cache_table` `Ok`) undercounts
        // relative to what's physically in the cache table. Assert on the
        // row directly instead (same reasoning as
        // `all_tables_listed_all_writes_failed_resolves_to_failed` in
        // `catalog::traits::tests`, which hits the same environment
        // limitation).
        let cached_rows: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM datasource_table_cache WHERE datasource_config_id = ? AND is_archived = 0",
        )
        .bind(&ctx.datasource_config_id)
        .fetch_one(sq)
        .await
        .expect("count cached tables");
        assert_eq!(
            cached_rows, 1,
            "the listed table must still be cached/preserved (KYO-324 invariant) even though the run is reported failed"
        );

        assert_eq!(
            datasource_status(sq, &ctx.datasource_config_id).await,
            "failed",
            "zero usable tables with a real write-failure error must surface as failed, not idle"
        );

        let progress: String = sqlx::query_scalar(
            "SELECT catalog_refresh_progress FROM datasource_configs WHERE id = ?",
        )
        .bind(&ctx.datasource_config_id)
        .fetch_one(sq)
        .await
        .expect("read progress envelope");
        assert!(
            progress.contains("orders") && progress.contains("pgvector"),
            "failure reason must name the real cache_table write error, not a generic message, got: {progress}"
        );
    }

    /// KYO-268 phase 2 originally introduced this as "the load-bearing
    /// case": a denied container plus one table found elsewhere, staying
    /// `"idle"` because *some* container was accessible. KYO-385 corrects
    /// the premise — the one table this fixture finds also fails its
    /// `cache_table` write on this SQLite pool (see
    /// `all_tables_listed_all_writes_failed_resolves_to_failed` above), so
    /// `tables_indexed` is `0` here too: nothing was actually usable this
    /// run, and with two real errors recorded (the denial and the write
    /// failure) the correct status is `"failed"`, not `"idle"`. What this
    /// test still proves, and the reason it stays distinct from
    /// `discovery_denial_plus_all_writes_failed_persists_failed_with_warnings`
    /// below: both failures reach the in-process `CatalogIndexResult::errors`
    /// return value, not just the persisted envelope that function checks —
    /// a separate channel (see that function's own doc comment).
    #[tokio::test]
    async fn discovery_denial_plus_all_writes_failed_reports_failed_and_preserves_errors() {
        let db = DbPool::connect("sqlite::memory:")
            .await
            .expect("connect in-memory sqlite");
        let DbPool::Sqlite(sq) = &db else {
            unreachable!("expected sqlite pool");
        };
        let ctx = seed_connect_fixture(sq, "partialok").await;
        let embedding = EmbeddingService::new().expect("load embedding model");

        let catalog_result = CatalogResult {
            containers: vec![kyomi_core::connect_protocol::CatalogContainer {
                name: "public".to_string(),
                tables: vec![kyomi_core::connect_protocol::CatalogTable {
                    name: "orders".to_string(),
                    native_type: Some("BASE TABLE".to_string()),
                    columns: vec![kyomi_core::connect_protocol::CatalogColumn {
                        name: "id".to_string(),
                        native_type: "int4".to_string(),
                        description: None,
                    }],
                }],
            }],
            errors: vec!["container 'restricted': permission denied".to_string()],
        };

        let result = process_discovered_catalog(ProcessDiscoveredCatalogParams {
            db: &db,
            embedding: &embedding,
            ctx: &ctx,
            catalog_result,
            explicit_empty: false,
            start_time: Utc::now(),
        })
        .await;

        // Two errors, not one: the denied-container error PLUS a
        // `cache_table` write failure for `orders` (KYO-364) — pgvector
        // storage is unsupported on the in-memory SQLite pool this test runs
        // against (see the cache-row comment below), so `cache_table`
        // always returns `Err` here, and that `Err` is now correctly folded
        // into `catalog_result.errors` instead of being silently dropped.
        let errors = result
            .errors
            .as_deref()
            .expect("both failures must reach the caller-visible result, not just logs");
        assert_eq!(errors.len(), 2, "got: {errors:?}");
        assert!(
            errors.contains(&"container 'restricted': permission denied".to_string()),
            "denied-container error must be present, got: {errors:?}"
        );
        assert!(
            errors.iter().any(|e| e.contains("orders")),
            "cache_table write failure for orders must be present, got: {errors:?}"
        );

        // See `all_tables_listed_all_writes_failed_resolves_to_failed` above
        // for why the cache row is asserted directly instead of
        // `result.tables_indexed`.
        let cached_rows: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM datasource_table_cache WHERE datasource_config_id = ? AND is_archived = 0",
        )
        .bind(&ctx.datasource_config_id)
        .fetch_one(sq)
        .await
        .expect("count cached tables");
        assert_eq!(
            cached_rows, 1,
            "the listed table must still be cached/preserved (KYO-324 invariant) even though the run is reported failed"
        );

        assert_eq!(
            datasource_status(sq, &ctx.datasource_config_id).await,
            "failed",
            "zero usable tables with two real errors recorded must surface as failed, not idle"
        );
    }

    /// KYO-268 phase 2: zero tables found *and* a recorded discovery error
    /// must report `"failed"` with a reason drawn from the real error text
    /// — not the generic "no tables found" message, and not silently
    /// `"idle"` (which would hide a genuine permission problem behind a
    /// healthy-looking status).
    #[tokio::test]
    async fn errors_with_zero_tables_reports_failed_with_real_reason() {
        let db = DbPool::connect("sqlite::memory:")
            .await
            .expect("connect in-memory sqlite");
        let DbPool::Sqlite(sq) = &db else {
            unreachable!("expected sqlite pool");
        };
        let ctx = seed_connect_fixture(sq, "alldenied").await;
        let embedding = EmbeddingService::new().expect("load embedding model");

        let catalog_result = CatalogResult {
            containers: Vec::new(),
            errors: vec!["container 'public': permission denied".to_string()],
        };

        let result = process_discovered_catalog(ProcessDiscoveredCatalogParams {
            db: &db,
            embedding: &embedding,
            ctx: &ctx,
            catalog_result,
            explicit_empty: false,
            start_time: Utc::now(),
        })
        .await;

        assert_eq!(
            result.errors.as_deref(),
            Some(&["container 'public': permission denied".to_string()][..])
        );

        assert_eq!(
            datasource_status(sq, &ctx.datasource_config_id).await,
            "failed",
            "zero tables caused by a recorded discovery error must surface as failed, not idle"
        );

        // The reason must be the real error text, not a generic message —
        // read it back off the persisted progress envelope, the exact shape
        // `get_catalog_refresh_status` returns to the frontend.
        let progress: String = sqlx::query_scalar(
            "SELECT catalog_refresh_progress FROM datasource_configs WHERE id = ?",
        )
        .bind(&ctx.datasource_config_id)
        .fetch_one(sq)
        .await
        .expect("read progress envelope");
        assert!(
            progress.contains("container 'public': permission denied"),
            "failure reason must be the real discovery error, not a generic message, got: {progress}"
        );
    }

    // ── persisted `warnings` array (KYO-327) ─────────────────────────────
    //
    // KYO-327's premise correction: before this fix, `resolve_final_status`
    // folding a non-clean run (tables found elsewhere, or some containers
    // denied) down to a terminal status also discarded the individual error
    // strings — they were never written to the persisted envelope at all,
    // so the settings page had nothing to read to explain the status. These
    // two tests lock in the terminal write's `warnings` parameter on both
    // sides of that behavior: non-empty on a failed/partial run, empty on a
    // clean one.

    /// The load-bearing regression: a run with one denied container AND a
    /// `cache_table` write failure for the only table found elsewhere must
    /// persist a non-empty `warnings` array carrying both real errors — not
    /// silently dropped the way `resolve_final_status`'s own `error` field
    /// used to be for a comparable case (see
    /// `not_nothing_found_reports_idle_even_with_partial_errors` in
    /// `kyomi_auth::catalog::helpers`, which covers the genuinely-partial,
    /// `tables_indexed > 0` case and is unaffected by this change).
    ///
    /// KYO-385: `catalog_refresh_status` itself is `"failed"` here, not
    /// `"idle"` — this fixture is partial on the *discovery* axis (one
    /// container denied, one listed fine) but 0-of-N on the *write* axis
    /// (`cache_table` fails for `public.orders` on this SQLite pool, see
    /// `all_tables_listed_all_writes_failed_resolves_to_failed` above), so
    /// zero tables were actually usable this run. Before this fix,
    /// `seen_table_ids` (populated before the write) masked that from the
    /// old `nothing_found` predicate and this test asserted `"idle"` — the
    /// KYO-364 bug surviving on this path, which KYO-364 fixed only for the
    /// BigQuery REST path. The `warnings` array itself is unaffected: it's
    /// built from `catalog_result.errors` regardless of which status they
    /// resolve to.
    #[tokio::test]
    async fn discovery_denial_plus_all_writes_failed_persists_failed_with_warnings() {
        let db = DbPool::connect("sqlite::memory:")
            .await
            .expect("connect in-memory sqlite");
        let DbPool::Sqlite(sq) = &db else {
            unreachable!("expected sqlite pool");
        };
        let ctx = seed_connect_fixture(sq, "partialwarn").await;
        let embedding = EmbeddingService::new().expect("load embedding model");

        let catalog_result = CatalogResult {
            containers: vec![kyomi_core::connect_protocol::CatalogContainer {
                name: "public".to_string(),
                tables: vec![kyomi_core::connect_protocol::CatalogTable {
                    name: "orders".to_string(),
                    native_type: Some("BASE TABLE".to_string()),
                    columns: vec![kyomi_core::connect_protocol::CatalogColumn {
                        name: "id".to_string(),
                        native_type: "int4".to_string(),
                        description: None,
                    }],
                }],
            }],
            errors: vec!["container 'restricted': permission denied".to_string()],
        };

        process_discovered_catalog(ProcessDiscoveredCatalogParams {
            db: &db,
            embedding: &embedding,
            ctx: &ctx,
            catalog_result,
            explicit_empty: false,
            start_time: Utc::now(),
        })
        .await;

        assert_eq!(
            datasource_status(sq, &ctx.datasource_config_id).await,
            "failed",
            "zero usable tables with two real errors recorded must surface as failed, not idle"
        );

        let envelope = datasource_progress_envelope(sq, &ctx.datasource_config_id).await;
        let warnings = envelope
            .get("warnings")
            .and_then(|w| w.as_array())
            .expect("warnings must be a present JSON array");
        // Two warnings, not one. The denied container is the one this test
        // was written for; the second is a real `cache_table` failure for
        // `public.orders`, because `store_search_embeddings` is unsupported
        // on this in-memory SQLite pool. Before KYO-364 `cache_table`
        // returned a bare `false` and that failure was silently dropped, so
        // this assertion used to see only the denial. Asserting the exact
        // vector (rather than "contains the denial") is deliberate: it still
        // proves nothing *else* is persisted.
        assert_eq!(
            warnings,
            &vec![
                serde_json::json!("container 'restricted': permission denied"),
                serde_json::json!(
                    "Failed to cache table public.orders: internal: failed to store embeddings for public.orders: internal: pgvector embeddings are not supported on SQLite"
                ),
            ],
            "the denied container's error and the real cache_table write failure must both be persisted, got: {envelope}"
        );
    }

    /// Companion regression guard (ticket AC3): a run with zero discovery
    /// errors must persist an empty `warnings` array — always present, never
    /// missing/null, so `get_catalog_stats` never needs null-handling — and
    /// must not surface a warning to the user.
    ///
    /// The container deliberately lists **no tables**, because on this
    /// in-memory SQLite pool a genuinely clean run is the only run with no
    /// tables in it: `store_search_embeddings` returns "pgvector embeddings
    /// are not supported on SQLite" for every real table, so any table here
    /// makes `cache_table` fail for real. Since KYO-364 that failure is
    /// correctly surfaced as a warning instead of being swallowed as a bare
    /// `false`, so a one-table fixture no longer *is* a clean run and cannot
    /// test this property. Zero tables keeps the assertion honest: the
    /// terminal write still runs (`resolve_run_outcome` skips archiving but
    /// not the status write), and it must still persist `warnings: []`.
    #[tokio::test]
    async fn clean_run_persists_empty_warnings_array() {
        let db = DbPool::connect("sqlite::memory:")
            .await
            .expect("connect in-memory sqlite");
        let DbPool::Sqlite(sq) = &db else {
            unreachable!("expected sqlite pool");
        };
        let ctx = seed_connect_fixture(sq, "cleanwarn").await;
        let embedding = EmbeddingService::new().expect("load embedding model");

        let catalog_result = CatalogResult {
            containers: vec![kyomi_core::connect_protocol::CatalogContainer {
                name: "public".to_string(),
                tables: Vec::new(),
            }],
            errors: Vec::new(),
        };

        process_discovered_catalog(ProcessDiscoveredCatalogParams {
            db: &db,
            embedding: &embedding,
            ctx: &ctx,
            catalog_result,
            explicit_empty: false,
            start_time: Utc::now(),
        })
        .await;

        assert_eq!(
            datasource_status(sq, &ctx.datasource_config_id).await,
            "idle"
        );

        let envelope = datasource_progress_envelope(sq, &ctx.datasource_config_id).await;
        let warnings = envelope
            .get("warnings")
            .and_then(|w| w.as_array())
            .expect("warnings must be a present JSON array, not missing/null");
        assert!(
            warnings.is_empty(),
            "a clean run (zero discovery errors) must persist an empty warnings array, got: {envelope}"
        );
    }
}
