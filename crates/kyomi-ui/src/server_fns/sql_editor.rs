// SPDX-License-Identifier: AGPL-3.0-or-later

//! Server functions for SQL Editor dry-run validation, history, catalog, and chart generation.

use leptos::prelude::*;
use serde::{Deserialize, Serialize};

#[cfg(feature = "ssr")]
use super::{AuthenticatedContext, IntoServerFnErrorCore, IntoServerFnErrorSqlx};
#[cfg(feature = "ssr")]
use kyomi_types::Permission;
#[cfg(feature = "ssr")]
use kyomi_core::json_utils::bigquery_include_public;

use crate::pages::sql_editor::types::{CatalogNode, QueryHistoryEntry};
#[cfg(feature = "ssr")]
use crate::pages::sql_editor::types::CatalogNodeType;

// ---------------------------------------------------------------------------
// Response types (server-fn-specific — not shared with client-side state)
// ---------------------------------------------------------------------------

/// Result from dry-run query validation.
///
/// Returned by the `dry_run_sql` server function. Includes error location
/// information for inline editor markers.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DryRunResult {
    /// `true` if the query is syntactically valid.
    pub valid: bool,
    /// Provider-formatted message to display in the status bar.
    pub message: String,
    /// Error line number (1-indexed). `None` if valid or location unavailable.
    pub line: Option<u32>,
    /// Error column number. `None` if valid or location unavailable.
    pub column: Option<u32>,
    /// Bytes that would be processed (BigQuery only). `None` for other providers.
    pub bytes_processed: Option<u64>,
}

// ---------------------------------------------------------------------------
// dry_run_sql — SQL validation without execution
// ---------------------------------------------------------------------------

/// Validate SQL syntax without executing the query.
///
/// Uses database-native mechanisms (e.g., BigQuery `dryRun: true`, PostgreSQL
/// `EXPLAIN`, SQL Server `SET NOEXEC ON`) to check syntax and estimate cost.
///
/// Providers are cached for 60 seconds keyed on `(user_id, workspace_id,
/// datasource_slug)` to avoid the 300-500ms setup cost on every keystroke.
/// The cache manages provider lifecycle — `close()` is intentionally not
/// called here because the cached `Arc` must remain valid for future calls.
#[server(prefix = "/leptos-api")]
pub async fn dry_run_sql(
    datasource_slug: String,
    sql: String,
) -> Result<DryRunResult, ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;

    let provider = if let Some(cached) = super::provider_cache::get_cached(
        &ac.auth.user_id,
        &ac.ws_id,
        &datasource_slug,
    ) {
        cached
    } else {
        let (_ds, boxed) = super::datasources::create_query_provider(
            &ac.ctx,
            &ac.auth,
            &ac.ws_id,
            &datasource_slug,
        )
        .await?;
        super::provider_cache::insert(&ac.auth.user_id, &ac.ws_id, &datasource_slug, boxed)
    };

    let result = match tokio::time::timeout(
        kyomi_datasource_server::DATASOURCE_TIMEOUT_DRY_RUN,
        provider.dry_run(&sql),
    )
    .await
    {
        Ok(Ok(dr)) => DryRunResult {
            valid: dr.valid,
            message: dr.message,
            line: dr.line,
            column: dr.column,
            // bytes_processed is not part of the driver DryRunResult;
            // BigQuery returns it in the message string. Future enhancement
            // could parse it out, but for now we leave it as None.
            bytes_processed: None,
        },
        Ok(Err(e)) => DryRunResult {
            valid: false,
            message: format!("Validation failed: {e}"),
            line: None,
            column: None,
            bytes_processed: None,
        },
        Err(_) => DryRunResult {
            valid: false,
            message: "SQL validation timed out".to_string(),
            line: None,
            column: None,
            bytes_processed: None,
        },
    };

    // No provider.close() — the cache manages provider lifecycle.

    Ok(result)
}

// ===========================================================================
// SQL history server functions
// ===========================================================================

// ---------------------------------------------------------------------------
// list_query_history — paginated query history with search/filter
// ---------------------------------------------------------------------------

/// List query history for the current user.
///
/// Supports text search on query_text, filtering to saved-only, and
/// pagination via limit/offset.
#[server(prefix = "/leptos-api")]
pub async fn list_query_history(
    search: Option<String>,
    saved_only: Option<bool>,
    limit: u32,
    offset: u32,
) -> Result<Vec<QueryHistoryEntry>, ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;

    let records = kyomi_auth::sql_history_service::list_query_history(
        ac.db(),
        &ac.ws_id,
        &ac.auth.user_id,
        limit.clamp(1, 1000) as i64,
        offset as i64,
        saved_only.unwrap_or(false),
        search.as_deref(),
    )
    .await
    .into_sfn_core()?;

    let entries: Vec<QueryHistoryEntry> = records
        .into_iter()
        .map(|(h, slug)| QueryHistoryEntry {
            id: h.query_id,
            query_text: h.query_text,
            execution_time_ms: h.execution_time_ms,
            bytes_processed: h.bytes_processed,
            row_count: h.row_count,
            status: h.status,
            error_message: h.error_message,
            datasource: slug,
            is_saved: h.is_saved,
            created_at: h.executed_at.to_rfc3339(),
        })
        .collect();

    Ok(entries)
}

// ---------------------------------------------------------------------------
// save_query_history — create a new history entry
// ---------------------------------------------------------------------------

/// Save a new query history entry.
///
/// If `datasource` slug is provided, resolves it to a datasource_config_id.
/// Returns the new query_id.
#[server(prefix = "/leptos-api")]
pub async fn save_query_history(
    query_text: String,
    execution_time_ms: Option<i32>,
    bytes_processed: Option<i64>,
    row_count: Option<i32>,
    status: String,
    error_message: Option<String>,
    datasource: Option<String>,
) -> Result<String, ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;

    // Resolve datasource slug to ID if provided.
    let datasource_config_id = if let Some(ref slug) = datasource {
        let ds = kyomi_auth::datasource_service::get_datasource_by_slug(
            ac.db(),
            slug,
            &ac.ws_id,
        )
        .await
        .into_sfn_core()?;
        ds.map(|d| d.id)
    } else {
        None
    };

    let record = kyomi_auth::sql_history_service::create_query_history(
        ac.db(),
        &ac.ws_id,
        &ac.auth.user_id,
        datasource_config_id.as_deref(),
        &query_text,
        execution_time_ms,
        bytes_processed,
        row_count,
        &status,
        error_message.as_deref(),
    )
    .await
    .into_sfn_core()?;

    Ok(record.query_id)
}

// ---------------------------------------------------------------------------
// update_query_history — toggle saved status
// ---------------------------------------------------------------------------

/// Update a query history entry (e.g., toggle saved/bookmark).
#[server(prefix = "/leptos-api")]
pub async fn update_query_history(
    query_id: String,
    is_saved: Option<bool>,
) -> Result<(), ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;

    let result = kyomi_auth::sql_history_service::update_query_history(
        ac.db(),
        &query_id,
        &ac.ws_id,
        &ac.auth.user_id,
        is_saved,
        None, // query_name
        None, // tags
    )
    .await
    .into_sfn_core()?;

    if result.is_none() {
        return Err(ServerFnError::new(format!(
            "Query history '{query_id}' not found"
        )));
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// delete_query_history — remove a history entry
// ---------------------------------------------------------------------------

/// Delete a query history entry.
#[server(prefix = "/leptos-api")]
pub async fn delete_query_history(
    query_id: String,
) -> Result<(), ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;

    let deleted = kyomi_auth::sql_history_service::delete_query_history(
        ac.db(),
        &query_id,
        &ac.ws_id,
        &ac.auth.user_id,
    )
    .await
    .into_sfn_core()?;

    if !deleted {
        return Err(ServerFnError::new(format!(
            "Query history '{query_id}' not found"
        )));
    }

    Ok(())
}

// ===========================================================================
// Task 1.7: Catalog server functions
// ===========================================================================

/// Result from the catalog tree endpoint.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CatalogTreeResult {
    pub tree: Vec<CatalogNode>,
    pub datasource_type: String,
    pub table_count: usize,
}

// ---------------------------------------------------------------------------
// Catalog row projections (KYO-447)
//
// `datasource_table_cache.table_metadata` is a JSON blob averaging ~1.6 KB
// and reaching ~45 KB per row — measured against production, a full fetch
// of the BigQuery public-dataset sentinel (9,783 rows) is 16 MB, of which
// `table_metadata` alone is 15 MB (94%). The tree only needs
// `project_id`/`dataset_id`/`table_id` plus a table/view classification
// for every row, and the full `columns` array only when the caller asked
// for it (`include_columns`). Catalog search never shows columns, so it
// never needs the blob either. These narrow row types let the SQL do the
// projection instead of fetching every column and discarding most of it.
// ---------------------------------------------------------------------------

/// Columns needed to classify a table/view node without the `columns`
/// array — used for the catalog tree when `include_columns` is false, and
/// always for catalog search (which never shows column children).
#[cfg(feature = "ssr")]
#[derive(Debug, Clone, sqlx::FromRow)]
struct CatalogTableSummaryRow {
    project_id: String,
    dataset_id: String,
    table_id: String,
    /// Extracted in SQL from `table_metadata->>'table_type'` — the full
    /// blob is never selected.
    table_type: Option<String>,
}

/// Columns needed to build column children — used for the catalog tree
/// when `include_columns` is true.
#[cfg(feature = "ssr")]
#[derive(Debug, Clone, sqlx::FromRow)]
struct CatalogTableWithColumnsRow {
    project_id: String,
    dataset_id: String,
    table_id: String,
    table_metadata: serde_json::Value,
}

/// A table normalized for catalog-tree building, regardless of whether it
/// came from a summary row or a with-columns row.
#[cfg(feature = "ssr")]
struct TreeTable {
    project_id: String,
    dataset_id: String,
    table_id: String,
    table_type: Option<String>,
    columns: Vec<CatalogNode>,
}

#[cfg(feature = "ssr")]
impl From<CatalogTableSummaryRow> for TreeTable {
    fn from(row: CatalogTableSummaryRow) -> Self {
        Self {
            project_id: row.project_id,
            dataset_id: row.dataset_id,
            table_id: row.table_id,
            table_type: row.table_type,
            columns: Vec::new(),
        }
    }
}

#[cfg(feature = "ssr")]
impl From<CatalogTableWithColumnsRow> for TreeTable {
    fn from(row: CatalogTableWithColumnsRow) -> Self {
        let table_type = row
            .table_metadata
            .get("table_type")
            .and_then(|v| v.as_str())
            .map(str::to_string);

        let columns = row
            .table_metadata
            .get("columns")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|col| {
                        let col_name = col.get("name")?.as_str()?;
                        let col_type = col
                            .get("type")
                            .and_then(|v| v.as_str())
                            .unwrap_or("unknown")
                            .to_string();
                        let col_full = if row.project_id.is_empty() {
                            format!("{}.{}.{}", row.dataset_id, row.table_id, col_name)
                        } else {
                            format!(
                                "{}.{}.{}.{}",
                                row.project_id, row.dataset_id, row.table_id, col_name
                            )
                        };
                        Some(CatalogNode {
                            name: col_name.to_string(),
                            node_type: CatalogNodeType::Column(col_type),
                            children: Vec::new(),
                            full_name: Some(col_full),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();

        Self {
            project_id: row.project_id,
            dataset_id: row.dataset_id,
            table_id: row.table_id,
            table_type,
            columns,
        }
    }
}

/// Fetch `datasource_table_cache` rows for the catalog tree, projected to
/// only the columns needed for the requested `include_columns` mode.
/// `filter_column` is `"datasource_config_id"` for the caller's own
/// datasource or `"workspace_id"` for the BigQuery public-dataset sentinel
/// — both share the same shape, just a different scoping column.
#[cfg(feature = "ssr")]
async fn fetch_tree_tables(
    db: &kyomi_core::DbPool,
    is_pg: bool,
    filter_column: &str,
    filter_value: &str,
    include_columns: bool,
) -> Result<Vec<TreeTable>, sqlx::Error> {
    let bf = kyomi_core::sql_compat::bool_false(is_pg);

    if include_columns {
        let sql = format!(
            "SELECT project_id, dataset_id, table_id, table_metadata \
             FROM datasource_table_cache \
             WHERE {filter_column} = $1 AND is_archived = {bf}"
        );
        let rows: Vec<CatalogTableWithColumnsRow> =
            kyomi_core::db_fetch_all!(db, CatalogTableWithColumnsRow, &sql, filter_value)?;
        Ok(rows.into_iter().map(TreeTable::from).collect())
    } else {
        let table_type_expr =
            kyomi_core::sql_compat::json_extract_text(is_pg, "table_metadata", "table_type");
        let sql = format!(
            "SELECT project_id, dataset_id, table_id, {table_type_expr} AS table_type \
             FROM datasource_table_cache \
             WHERE {filter_column} = $1 AND is_archived = {bf}"
        );
        let rows: Vec<CatalogTableSummaryRow> =
            kyomi_core::db_fetch_all!(db, CatalogTableSummaryRow, &sql, filter_value)?;
        Ok(rows.into_iter().map(TreeTable::from).collect())
    }
}

/// Fetch `datasource_table_cache` rows matching a table-name substring
/// search, projected the same way as `fetch_tree_tables`'s
/// `include_columns = false` branch — search never shows column children,
/// so it never needs the `table_metadata` blob.
#[cfg(feature = "ssr")]
async fn fetch_search_tables(
    db: &kyomi_core::DbPool,
    is_pg: bool,
    filter_column: &str,
    filter_value: &str,
    search_pattern: &str,
) -> Result<Vec<CatalogTableSummaryRow>, sqlx::Error> {
    let bf = kyomi_core::sql_compat::bool_false(is_pg);
    let ilike = kyomi_core::sql_compat::ilike(is_pg, "table_id", "$2");
    let table_type_expr =
        kyomi_core::sql_compat::json_extract_text(is_pg, "table_metadata", "table_type");

    let sql = format!(
        "SELECT project_id, dataset_id, table_id, {table_type_expr} AS table_type \
         FROM datasource_table_cache \
         WHERE {filter_column} = $1 AND is_archived = {bf} AND {ilike} \
         ORDER BY table_id \
         LIMIT 50"
    );

    kyomi_core::db_fetch_all!(db, CatalogTableSummaryRow, &sql, filter_value, search_pattern)
}

/// Build the hierarchical catalog tree from projected table rows.
///
/// Pure function (no I/O) — pulled out of `get_catalog_tree` so the
/// tree-shape logic can be unit tested directly against hand-built rows,
/// independent of the SQL projection that produced them.
#[cfg(feature = "ssr")]
fn build_catalog_tree(
    cached_tables: Vec<TreeTable>,
    meta: &kyomi_core::datasource_registry::DatasourceTypeMetadata,
) -> Vec<CatalogNode> {
    use std::collections::BTreeMap;

    // Build tree: {project_id: {dataset_id: [table_nodes]}}
    let mut tree_dict: BTreeMap<String, BTreeMap<String, Vec<CatalogNode>>> = BTreeMap::new();

    for table in cached_tables {
        let TreeTable {
            project_id,
            dataset_id,
            table_id,
            table_type,
            columns,
        } = table;

        // Build fully-qualified table name.
        let full_name = if project_id.is_empty() {
            format!("{dataset_id}.{table_id}")
        } else {
            format!("{project_id}.{dataset_id}.{table_id}")
        };

        // Determine table vs view from the extracted/embedded table_type.
        let table_type_str = table_type.as_deref().unwrap_or("TABLE");
        let node_type = if table_type_str.to_uppercase().contains("VIEW") {
            CatalogNodeType::View
        } else {
            CatalogNodeType::Table
        };

        let project_map = tree_dict.entry(project_id).or_default();
        let table_list = project_map.entry(dataset_id).or_default();
        table_list.push(CatalogNode {
            name: table_id,
            node_type,
            children: columns,
            full_name: Some(full_name),
        });
    }

    // Convert tree_dict to CatalogNode structure using registry metadata.
    let level2_type = match meta.tree_level2_type {
        "dataset" => CatalogNodeType::Dataset,
        "schema" => CatalogNodeType::Schema,
        "database" => CatalogNodeType::Database,
        _ => CatalogNodeType::Schema, // fallback
    };

    let level1_type = match meta.tree_level1_type {
        "project" => CatalogNodeType::Project,
        "database" => CatalogNodeType::Database,
        "catalog" => CatalogNodeType::Database,
        _ => CatalogNodeType::Project, // fallback
    };

    let mut tree: Vec<CatalogNode> = Vec::new();

    for (project_id, datasets) in &tree_dict {
        let mut dataset_nodes: Vec<CatalogNode> = Vec::new();

        for (dataset_id, tables) in datasets {
            let ds_full = if project_id.is_empty() {
                dataset_id.clone()
            } else {
                format!("{project_id}.{dataset_id}")
            };

            let mut sorted_tables = tables.clone();
            sorted_tables.sort_by(|a, b| a.name.cmp(&b.name));

            dataset_nodes.push(CatalogNode {
                name: dataset_id.clone(),
                node_type: level2_type.clone(),
                children: sorted_tables,
                full_name: Some(ds_full),
            });
        }

        dataset_nodes.sort_by(|a, b| a.name.cmp(&b.name));

        let skip_wrapper = (meta.skip_empty_project_wrapper && project_id.is_empty())
            || (meta.skip_single_project_wrapper && tree_dict.len() == 1);

        if skip_wrapper {
            tree.extend(dataset_nodes);
        } else {
            tree.push(CatalogNode {
                name: project_id.clone(),
                node_type: level1_type.clone(),
                children: dataset_nodes,
                full_name: Some(project_id.clone()),
            });
        }
    }

    tree
}

// ---------------------------------------------------------------------------
// get_catalog_tree — hierarchical catalog tree from cache
// ---------------------------------------------------------------------------

/// Fetch the catalog tree for a datasource.
///
/// Replicates the tree-building logic from `GET /{identifier}/catalog/tree`
/// in the REST handler. Builds a hierarchical tree from the
/// `datasource_table_cache` table: project > dataset/schema > table > column.
#[server(prefix = "/leptos-api")]
pub async fn get_catalog_tree(
    datasource_slug: String,
    include_columns: bool,
) -> Result<CatalogTreeResult, ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;

    // Resolve datasource.
    let datasource = kyomi_auth::datasource_service::resolve_datasource(
        ac.db(),
        &datasource_slug,
        &ac.ws_id,
        false,
    )
    .await
    .into_sfn_core()?;

    // Fetch all non-archived cached tables for this datasource, projected
    // to only what the tree needs for this `include_columns` mode (see
    // "Catalog row projections (KYO-447)" above).
    //
    // Note: sample datasources used to live in a shared sentinel workspace
    // (`SAMPLE_DATA_WORKSPACE_ID`), so this query had an `is_sample` branch
    // that read from there. `onboarding::add_sample_datasource` switched to
    // the generic per-workspace indexer but this read site was missed, so
    // samples appeared empty in the UI. Query by `datasource_config_id`
    // uniformly — works for every datasource type including samples.
    let is_pg = ac.db().is_postgres();
    let mut cached_tables = fetch_tree_tables(
        ac.db(),
        is_pg,
        "datasource_config_id",
        &datasource.id,
        include_columns,
    )
    .await
    .into_sfn_sqlx()?;

    // BigQuery public datasets: include if enabled (absent key defaults to
    // disabled — see kyomi_core::json_utils::bigquery_include_public).
    if datasource.datasource_type == kyomi_core::DatasourceType::Bigquery {
        let include_public = bigquery_include_public(&datasource.connection_config);

        if include_public {
            let public_tables = fetch_tree_tables(
                ac.db(),
                is_pg,
                "workspace_id",
                kyomi_auth::catalog::indexers::bigquery_public::PUBLIC_DATA_WORKSPACE_ID,
                include_columns,
            )
            .await
            .into_sfn_sqlx()?;
            cached_tables.extend(public_tables);
        }
    }

    let table_count = cached_tables.len();

    let meta = kyomi_core::datasource_registry::get_metadata_by_str(
        datasource.datasource_type.as_ref(),
    )
    .ok_or_else(|| {
        ServerFnError::new(format!(
            "Unknown datasource type: '{}'",
            datasource.datasource_type.as_ref()
        ))
    })?;

    let tree = build_catalog_tree(cached_tables, meta);

    Ok(CatalogTreeResult {
        tree,
        datasource_type: datasource.datasource_type.as_ref().to_string(),
        table_count,
    })
}

// ---------------------------------------------------------------------------
// search_catalog — simple text search on table names
// ---------------------------------------------------------------------------

/// Search the catalog for tables matching a substring query.
///
/// Returns a flat list of matching table nodes (no hierarchy).
#[server(prefix = "/leptos-api")]
pub async fn search_catalog(
    datasource_slug: String,
    query: String,
) -> Result<Vec<CatalogNode>, ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;

    let datasource = kyomi_auth::datasource_service::resolve_datasource(
        ac.db(),
        &datasource_slug,
        &ac.ws_id,
        false,
    )
    .await
    .into_sfn_core()?;

    // Sample datasources used to live in a shared sentinel workspace and
    // required an `is_sample` branch here to read from `SAMPLE_DATA_WORKSPACE_ID`.
    // Samples now index into the user's workspace via the generic indexer
    // (see `onboarding::add_sample_datasource`), so we query by
    // `datasource_config_id` uniformly.
    //
    // Search never shows column children, so the query is projected to
    // avoid `table_metadata` entirely (see "Catalog row projections
    // (KYO-447)" above) — `table_type` is extracted as a scalar just to
    // classify table vs view.
    let is_pg = ac.db().is_postgres();
    let search_pattern = format!("%{query}%");

    let mut cached_tables = fetch_search_tables(
        ac.db(),
        is_pg,
        "datasource_config_id",
        &datasource.id,
        &search_pattern,
    )
    .await
    .into_sfn_sqlx()?;

    // BigQuery public datasets: include matching tables if enabled (absent
    // key defaults to disabled — see
    // kyomi_core::json_utils::bigquery_include_public).
    if datasource.datasource_type == kyomi_core::DatasourceType::Bigquery {
        let include_public = bigquery_include_public(&datasource.connection_config);

        if include_public {
            let public_tables = fetch_search_tables(
                ac.db(),
                is_pg,
                "workspace_id",
                kyomi_auth::catalog::indexers::bigquery_public::PUBLIC_DATA_WORKSPACE_ID,
                &search_pattern,
            )
            .await
            .into_sfn_sqlx()?;
            cached_tables.extend(public_tables);
        }
    }

    // Cap combined results (primary + public datasets) to 50
    cached_tables.truncate(50);

    let results: Vec<CatalogNode> = cached_tables
        .into_iter()
        .map(|table| {
            let full_name = if table.project_id.is_empty() {
                format!("{}.{}", table.dataset_id, table.table_id)
            } else {
                format!("{}.{}.{}", table.project_id, table.dataset_id, table.table_id)
            };

            let table_type_str = table.table_type.as_deref().unwrap_or("TABLE");
            let node_type = if table_type_str.to_uppercase().contains("VIEW") {
                CatalogNodeType::View
            } else {
                CatalogNodeType::Table
            };

            CatalogNode {
                name: table.table_id,
                node_type,
                children: Vec::new(),
                full_name: Some(full_name),
            }
        })
        .collect();

    Ok(results)
}

// ---------------------------------------------------------------------------
// refresh_catalog — trigger manual catalog refresh
// ---------------------------------------------------------------------------

/// Trigger a manual catalog refresh for a datasource.
///
/// Only workspace admins can trigger refreshes. The actual table indexing
/// can take minutes for large catalogs, which is far past Cloudflare's
/// ~100s request timeout, so this function only does cheap synchronous
/// work — permission check, credential resolution/refresh, and a live
/// `test_connection()` validation — before backgrounding the slow
/// `CatalogIndexingService::index_datasource()` call (the single catalog
/// indexing pipeline used by all callers: manual refresh, scheduler,
/// post-create spawn) in a `tokio::spawn`. The spawned task updates
/// `datasource_configs.catalog_refresh_status` on its own (see
/// `update_datasource_status` in `kyomi-auth/src/catalog/helpers.rs`) —
/// poll it via `get_catalog_refresh_status`.
///
/// Returns immediately once validation passes, before indexing starts.
#[server(prefix = "/leptos-api")]
pub async fn refresh_catalog(
    datasource_slug: String,
) -> Result<String, ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;

    ac.require(
        Permission::RefreshCatalog,
        "Only workspace admins can trigger catalog refresh",
    )?;

    let datasource = kyomi_auth::datasource_service::resolve_datasource(
        ac.db(),
        &datasource_slug,
        &ac.ws_id,
        false,
    )
    .await
    .into_sfn_core()?;

    let encryption_key = ac.encryption_key()?;

    // Guard check → decrypt/refresh credentials → live connection
    // validation, all in one shared orchestration function (kept out of
    // this server_fn body so it stays under the `check-server-fns.sh`
    // service-layer-callout limit). See `prepare_manual_catalog_refresh`
    // for the full step-by-step behavior — it preserves the same
    // "already running" guard semantics and the same synchronous
    // validation error behavior this function had inline before.
    let decision = kyomi_auth::datasource_service::prepare_manual_catalog_refresh(
        kyomi_auth::datasource_service::PrepareManualRefreshParams {
            db: ac.db(),
            user_id: &ac.auth.user_id,
            email: ac.auth.email.clone(),
            ws_id: &ac.ws_id,
            datasource: &datasource,
            encryption_key: &encryption_key,
            connect_registry: ac.ctx.connect_registry.as_ref(),
            google_oauth_client_id: ac.ctx.config.google_oauth_client_id.as_deref(),
            google_oauth_client_secret: ac.ctx.config.google_oauth_client_secret.as_deref(),
            guard_minutes: kyomi_agent::catalog::indexing_service::CONCURRENT_RUN_GUARD_MINUTES,
            connect_timeout: kyomi_datasource_server::DATASOURCE_TIMEOUT_CONNECT,
        },
    )
    .await
    .into_sfn_core()?;

    let credentials = match decision {
        kyomi_auth::datasource_service::ManualRefreshDecision::AlreadyRunning => {
            return Ok("A catalog refresh is already running".to_string());
        }
        kyomi_auth::datasource_service::ManualRefreshDecision::Ready { credentials } => credentials,
    };

    // Validation passed. Background the slow indexing so the request can
    // return well within Cloudflare's timeout. `force: false` — we already
    // checked the concurrent-run guard above; leaving it `false` here (not
    // `true` as before) means `index_datasource`'s internal guard is still
    // a live backstop against a second click racing this same request.
    let db = ac.ctx.db.clone();
    let embedding = ac.ctx.embedding.clone();
    let connect_registry = ac.ctx.connect_registry.clone();
    let workspace_id = ac.ws_id.clone();
    let datasource_id = datasource.id.clone();
    let user_email = ac.auth.email.clone();

    tokio::spawn(async move {
        let embed = match embedding.wait_ready().await {
            Ok(e) => e,
            Err(e) => {
                tracing::warn!(
                    datasource_id = %datasource_id,
                    workspace_id = %workspace_id,
                    error = %e,
                    "Embedding model not ready, skipping catalog refresh"
                );
                return;
            }
        };

        let result =
            kyomi_agent::catalog::indexing_service::CatalogIndexingService::index_datasource(
                kyomi_agent::catalog::indexing_service::IndexDatasourceParams {
                    db: &db,
                    encryption_key,
                    embedding: embed,
                    workspace_id: &workspace_id,
                    datasource_config_id: &datasource_id,
                    user_email: Some(&user_email),
                    credentials: Some(&credentials),
                    max_tables_per_dataset: None,
                    force: false,
                    connect_registry: connect_registry.as_ref(),
                },
            )
            .await;

        tracing::info!(
            datasource_id = %datasource_id,
            workspace_id = %workspace_id,
            status = ?result.status,
            tables = result.tables_indexed,
            "Manual catalog refresh completed"
        );
    });

    Ok("Catalog refresh started — this can take a few minutes for large catalogs.".to_string())
}

// ---------------------------------------------------------------------------
// get_catalog_refresh_status — poll catalog refresh progress
// ---------------------------------------------------------------------------

/// Response for `get_catalog_refresh_status`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CatalogRefreshStatusResponse {
    /// One of "idle", "running", "failed".
    pub status: String,
    /// Progress details written by the indexing pipeline, if any.
    pub progress: Option<serde_json::Value>,
}

/// Fetch the current catalog refresh status for a datasource.
///
/// Status is tracked per-datasource (`datasource_configs.catalog_refresh_status`,
/// KYO-267) — `datasource_slug` both confirms the caller has access to this
/// datasource in the workspace and identifies which datasource's status to
/// return.
#[server(prefix = "/leptos-api")]
pub async fn get_catalog_refresh_status(
    datasource_slug: String,
) -> Result<CatalogRefreshStatusResponse, ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;

    // Resolve the slug to confirm workspace access and get the datasource id.
    let datasource = kyomi_auth::datasource_service::resolve_datasource(
        ac.db(),
        &datasource_slug,
        &ac.ws_id,
        false,
    )
    .await
    .into_sfn_core()?;

    #[derive(sqlx::FromRow)]
    struct DatasourceCatalogStatusRow {
        catalog_refresh_status: Option<kyomi_core::enums::CatalogRefreshStatus>,
        catalog_refresh_progress: Option<serde_json::Value>,
    }

    let row = kyomi_core::db_fetch_optional!(
        ac.db(),
        DatasourceCatalogStatusRow,
        "SELECT catalog_refresh_status, catalog_refresh_progress FROM datasource_configs \
         WHERE id = $1 AND workspace_id = $2",
        &datasource.id,
        &ac.ws_id
    )
    .into_sfn_sqlx()?;

    let (status, progress) = match row {
        Some(row) => (
            row.catalog_refresh_status
                .map(|s| s.as_ref().to_string())
                .unwrap_or_else(|| "idle".to_string()),
            row.catalog_refresh_progress,
        ),
        None => ("idle".to_string(), None),
    };

    Ok(CatalogRefreshStatusResponse { status, progress })
}

// ---------------------------------------------------------------------------
// get_table_info — table metadata from cache
// ---------------------------------------------------------------------------

/// Typed response for `get_table_info`, including metadata, descriptions,
/// and cache timestamps.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TableInfoResponse {
    /// The table_metadata JSON (contains columns array, table_name, etc.)
    pub table_metadata: serde_json::Value,
    /// Per-column descriptions map: { "column_name": "description" }
    pub column_descriptions: Option<serde_json::Value>,
    /// When the table structure was last refreshed from the datasource
    pub structure_refreshed_at: Option<String>,
    /// Fully qualified table ID for display
    pub table_id: String,
}

/// Get detailed table metadata (columns, descriptions, etc.) for a
/// single table from the `datasource_table_cache`.
///
/// Requires `datasource_slug` to verify the caller has workspace access
/// to the datasource that owns this table. Without this check, any
/// authenticated user could enumerate table metadata across workspaces.
#[server(prefix = "/leptos-api")]
pub async fn get_table_info(
    datasource_slug: String,
    table_id: String,
) -> Result<TableInfoResponse, ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;

    // Resolve datasource to verify workspace access.
    let datasource = kyomi_auth::datasource_service::resolve_datasource(
        ac.db(),
        &datasource_slug,
        &ac.ws_id,
        false,
    )
    .await
    .into_sfn_core()?;

    // Parse the table_id (e.g., "project.dataset.table" or "dataset.table").
    let parts: Vec<&str> = table_id.splitn(3, '.').collect();
    let (project_id, dataset_id, table_name) = match parts.len() {
        3 => (parts[0], parts[1], parts[2]),
        2 => ("", parts[0], parts[1]),
        _ => {
            return Err(ServerFnError::new(format!(
                "Invalid table_id format: '{table_id}'. Expected 'dataset.table' or 'project.dataset.table'"
            )));
        }
    };

    let is_pg = ac.db().is_postgres();
    let bf = kyomi_core::sql_compat::bool_false(is_pg);

    // Query matching rows from table cache, filtered by datasource_config_id
    // to ensure the table belongs to the resolved datasource.
    let sql = if project_id.is_empty() {
        format!(
            "SELECT id, workspace_id, datasource_config_id, project_id, dataset_id, table_id, \
             table_metadata, column_descriptions, created_at, updated_at, \
             structure_refreshed_at, descriptions_refreshed_at, is_archived, last_verified \
             FROM datasource_table_cache \
             WHERE datasource_config_id = $1 AND dataset_id = $2 AND table_id = $3 AND is_archived = {bf} \
             LIMIT 1"
        )
    } else {
        format!(
            "SELECT id, workspace_id, datasource_config_id, project_id, dataset_id, table_id, \
             table_metadata, column_descriptions, created_at, updated_at, \
             structure_refreshed_at, descriptions_refreshed_at, is_archived, last_verified \
             FROM datasource_table_cache \
             WHERE datasource_config_id = $1 AND project_id = $2 AND dataset_id = $3 AND table_id = $4 AND is_archived = {bf} \
             LIMIT 1"
        )
    };

    let table: Option<kyomi_core::models::table_cache::DatasourceTableCache> = if project_id
        .is_empty()
    {
        match ac.db() {
            kyomi_core::db::DbPool::Postgres(pg) => {
                sqlx::query_as::<_, kyomi_core::models::table_cache::DatasourceTableCache>(&sql)
                    .bind(&datasource.id)
                    .bind(dataset_id)
                    .bind(table_name)
                    .fetch_optional(pg)
                    .await
                    .into_sfn_sqlx()?
            }
            kyomi_core::db::DbPool::Sqlite(sq) => {
                sqlx::query_as::<_, kyomi_core::models::table_cache::DatasourceTableCache>(&sql)
                    .bind(&datasource.id)
                    .bind(dataset_id)
                    .bind(table_name)
                    .fetch_optional(sq)
                    .await
                    .into_sfn_sqlx()?
            }
        }
    } else {
        match ac.db() {
            kyomi_core::db::DbPool::Postgres(pg) => {
                sqlx::query_as::<_, kyomi_core::models::table_cache::DatasourceTableCache>(&sql)
                    .bind(&datasource.id)
                    .bind(project_id)
                    .bind(dataset_id)
                    .bind(table_name)
                    .fetch_optional(pg)
                    .await
                    .into_sfn_sqlx()?
            }
            kyomi_core::db::DbPool::Sqlite(sq) => {
                sqlx::query_as::<_, kyomi_core::models::table_cache::DatasourceTableCache>(&sql)
                    .bind(&datasource.id)
                    .bind(project_id)
                    .bind(dataset_id)
                    .bind(table_name)
                    .fetch_optional(sq)
                    .await
                    .into_sfn_sqlx()?
            }
        }
    };

    match table {
        Some(t) => Ok(TableInfoResponse {
            table_metadata: t.table_metadata,
            column_descriptions: t.column_descriptions,
            structure_refreshed_at: t.structure_refreshed_at.map(|dt| dt.to_rfc3339()),
            table_id: table_id.clone(),
        }),
        None => Err(ServerFnError::new(format!(
            "Table '{table_id}' not found in catalog"
        ))),
    }
}

// ---------------------------------------------------------------------------
// Helper: SQL for listing tables in a container (per datasource type)
// ---------------------------------------------------------------------------

// ===========================================================================
// Task 1.8: Chart generation server function
// ===========================================================================

/// Result from the chart generation endpoint.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GeneratedChart {
    pub chartml_yaml: String,
    pub title: Option<String>,
}

// ---------------------------------------------------------------------------
// generate_chart_from_results — rule-based ChartML from SQL results
// ---------------------------------------------------------------------------

/// Generate a ChartML visualization from SQL query results.
///
/// Uses rule-based inference (same logic as the REST handler in
/// `chart_generate.rs`): analyzes column types and cardinality to pick
/// the best chart type, then builds a ChartML YAML spec.
///
/// `input = server_fn::codec::Json` (KYO-459): `sample_rows` is
/// `Vec<Vec<serde_json::Value>>` — `results_container.rs` builds each leaf
/// via `s.parse::<f64>().map(|n| serde_json::json!(n))`, so a numeric
/// column genuinely arrives here as `Value::Number`. Under the default
/// `PostUrl` encoding, `serde_qs` deserializes every leaf of a
/// self-describing type as a JSON string regardless of what was
/// serialized client-side — the same root cause KYO-428 fixed for
/// `connection_config`/`credentials` in `datasources.rs`. Here it
/// silently corrupts `analyze_chart_column`'s `is_numeric` check below
/// (`values.iter().all(|v| v.is_number())` is always `false` once every
/// leaf decodes as a string), so every numeric column is misclassified as
/// categorical and the user gets a worse or empty chart with no error.
/// JSON preserves the original `Value::Number`/`Value::String` leaves,
/// matching `create_datasource_modal` (datasources.rs).
#[server(prefix = "/leptos-api", input = server_fn::codec::Json)]
pub async fn generate_chart_from_results(
    columns: Vec<String>,
    sample_rows: Vec<Vec<serde_json::Value>>,
    sql: String,
    datasource_slug: String,
) -> Result<GeneratedChart, ServerFnError> {
    // Auth check — must be logged in.
    let _ac = AuthenticatedContext::extract().await?;

    if columns.is_empty() {
        return Err(ServerFnError::new("No columns provided"));
    }

    let chart_yaml = generate_chartml_with_rules(&sql, &columns, &sample_rows, &datasource_slug)?;

    // Extract title from the generated YAML.
    let title = serde_yaml::from_str::<serde_yaml::Value>(&chart_yaml)
        .ok()
        .and_then(|v| v.get("title")?.as_str().map(String::from));

    Ok(GeneratedChart {
        chartml_yaml: chart_yaml,
        title,
    })
}

// ---------------------------------------------------------------------------
// Rule-based chart generation helpers (inlined from chart_generate.rs)
// ---------------------------------------------------------------------------

/// Column analysis result for chart type inference.
#[cfg(feature = "ssr")]
struct ChartColumnAnalysis {
    name: String,
    is_numeric: bool,
    is_date: bool,
    cardinality: usize,
}

/// Check whether a JSON value looks like a date/datetime string.
#[cfg(feature = "ssr")]
fn is_date_value(v: &serde_json::Value) -> bool {
    let s = match v.as_str() {
        Some(s) => s,
        None => return false,
    };
    if chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").is_ok() {
        return true;
    }
    if chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S").is_ok() {
        return true;
    }
    if chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S%.f").is_ok() {
        return true;
    }
    if chrono::DateTime::parse_from_rfc3339(s).is_ok() {
        return true;
    }
    false
}

/// Analyze a single column across the provided rows.
#[cfg(feature = "ssr")]
fn analyze_chart_column(
    col_name: &str,
    rows: &[Vec<serde_json::Value>],
    columns: &[String],
) -> ChartColumnAnalysis {
    let col_index = match columns.iter().position(|c| c == col_name) {
        Some(i) => i,
        None => {
            return ChartColumnAnalysis {
                name: col_name.to_string(),
                is_numeric: false,
                is_date: false,
                cardinality: 0,
            };
        }
    };

    let values: Vec<&serde_json::Value> = rows
        .iter()
        .filter_map(|row| row.get(col_index))
        .filter(|v| !v.is_null())
        .collect();

    if values.is_empty() {
        return ChartColumnAnalysis {
            name: col_name.to_string(),
            is_numeric: false,
            is_date: false,
            cardinality: 0,
        };
    }

    let is_numeric = values.iter().all(|v| v.is_number());
    let is_date = values.iter().all(|v| is_date_value(v));
    let cardinality = values
        .iter()
        .map(|v| v.to_string())
        .collect::<std::collections::HashSet<_>>()
        .len();

    ChartColumnAnalysis {
        name: col_name.to_string(),
        is_numeric,
        is_date,
        cardinality,
    }
}

/// Infer the best chart type from column analyses.
#[cfg(feature = "ssr")]
fn infer_chart_type(analyses: &[ChartColumnAnalysis]) -> &'static str {
    if analyses.iter().any(|a| a.is_date) {
        return "line";
    }
    if let Some(cat) = analyses.iter().find(|a| !a.is_numeric && !a.is_date)
        && cat.cardinality <= 20
    {
        return "bar";
    }
    "table"
}

/// Infer x and y axes from column analyses.
#[cfg(feature = "ssr")]
fn infer_axes<'a>(
    analyses: &'a [ChartColumnAnalysis],
    columns: &'a [String],
) -> (&'a str, &'a str) {
    let x_col = analyses
        .iter()
        .find(|a| a.is_date)
        .or_else(|| analyses.iter().find(|a| !a.is_numeric))
        .or_else(|| analyses.first());

    let x_name = x_col.map(|a| a.name.as_str()).unwrap_or(&columns[0]);

    let y_col = analyses
        .iter()
        .find(|a| a.is_numeric && a.name != x_name)
        .or_else(|| {
            if analyses.len() > 1 {
                Some(&analyses[1])
            } else {
                analyses.first()
            }
        });

    let y_name = y_col.map(|a| a.name.as_str()).unwrap_or_else(|| {
        if columns.len() > 1 {
            &columns[1]
        } else {
            &columns[0]
        }
    });

    (x_name, y_name)
}

/// Shorthand: create a `serde_yaml::Value::String`.
#[cfg(feature = "ssr")]
fn yaml_str(s: &str) -> serde_yaml::Value {
    serde_yaml::Value::String(s.to_string())
}

/// Build the `data:` section of the ChartML spec.
#[cfg(feature = "ssr")]
fn build_chart_data_section(datasource_slug: &str, sql_text: &str) -> serde_yaml::Mapping {
    let mut data = serde_yaml::Mapping::new();
    data.insert(yaml_str("datasource"), yaml_str(datasource_slug));
    data.insert(yaml_str("sql"), yaml_str(sql_text));
    data
}

/// Generate a metric card spec for single-value results.
#[cfg(feature = "ssr")]
fn generate_metric_card(column_name: &str, datasource_slug: &str, sql_text: &str) -> String {
    let mut spec = serde_yaml::Mapping::new();
    spec.insert(yaml_str("type"), yaml_str("chart"));
    spec.insert(yaml_str("version"), serde_yaml::Value::Number(1.into()));
    spec.insert(yaml_str("title"), yaml_str(column_name));
    spec.insert(
        yaml_str("data"),
        serde_yaml::Value::Mapping(build_chart_data_section(datasource_slug, sql_text)),
    );
    let mut vis = serde_yaml::Mapping::new();
    vis.insert(yaml_str("type"), yaml_str("metric"));
    vis.insert(yaml_str("value"), yaml_str(column_name));
    vis.insert(yaml_str("label"), yaml_str(column_name));
    spec.insert(yaml_str("visualize"), serde_yaml::Value::Mapping(vis));
    serde_yaml::to_string(&spec).unwrap_or_default()
}

/// Generate a table fallback spec.
#[cfg(feature = "ssr")]
fn generate_table_fallback(datasource_slug: &str, sql_text: &str, columns: &[String]) -> String {
    let mut spec = serde_yaml::Mapping::new();
    spec.insert(yaml_str("type"), yaml_str("chart"));
    spec.insert(yaml_str("version"), serde_yaml::Value::Number(1.into()));
    spec.insert(yaml_str("title"), yaml_str("Query Results"));
    spec.insert(
        yaml_str("data"),
        serde_yaml::Value::Mapping(build_chart_data_section(datasource_slug, sql_text)),
    );
    let mut vis = serde_yaml::Mapping::new();
    vis.insert(yaml_str("type"), yaml_str("table"));
    vis.insert(
        yaml_str("columns"),
        serde_yaml::Value::Sequence(columns.iter().map(|c| yaml_str(c)).collect()),
    );
    spec.insert(yaml_str("visualize"), serde_yaml::Value::Mapping(vis));
    serde_yaml::to_string(&spec).unwrap_or_default()
}

/// Rule-based ChartML generation.
///
/// Ported from `generate_with_rules()` in the REST route `chart_generate.rs`,
/// deleted wholesale in the React→Leptos migration (KYO-73, #183); this is
/// now the only implementation.
#[cfg(feature = "ssr")]
fn generate_chartml_with_rules(
    sql_text: &str,
    columns: &[String],
    rows: &[Vec<serde_json::Value>],
    datasource_slug: &str,
) -> Result<String, ServerFnError> {
    let analyses: Vec<ChartColumnAnalysis> = columns
        .iter()
        .map(|col| analyze_chart_column(col, rows, columns))
        .collect();

    // Single value -> metric card.
    if columns.len() == 1 && rows.len() == 1 {
        return Ok(generate_metric_card(&columns[0], datasource_slug, sql_text));
    }

    let chart_type = infer_chart_type(&analyses);
    let (x_axis, mut y_axis) = infer_axes(&analyses, columns);

    // Same column for both axes -> try picking a different y.
    if x_axis == y_axis && columns.len() > 1 {
        if let Some(alt) = analyses
            .iter()
            .find(|a| a.name != x_axis && a.is_numeric)
            .or_else(|| analyses.iter().find(|a| a.name != x_axis))
        {
            y_axis = &alt.name;
        } else {
            return Ok(generate_table_fallback(datasource_slug, sql_text, columns));
        }
    }

    let title = format!("{y_axis} by {x_axis}");

    let mut spec = serde_yaml::Mapping::new();
    spec.insert(yaml_str("type"), yaml_str("chart"));
    spec.insert(yaml_str("version"), serde_yaml::Value::Number(1.into()));
    spec.insert(yaml_str("title"), yaml_str(&title));
    spec.insert(
        yaml_str("data"),
        serde_yaml::Value::Mapping(build_chart_data_section(datasource_slug, sql_text)),
    );
    let mut vis = serde_yaml::Mapping::new();
    vis.insert(yaml_str("type"), yaml_str(chart_type));
    vis.insert(yaml_str("columns"), yaml_str(x_axis));
    vis.insert(yaml_str("rows"), yaml_str(y_axis));
    spec.insert(yaml_str("visualize"), serde_yaml::Value::Mapping(vis));

    Ok(serde_yaml::to_string(&spec).unwrap_or_default())
}

// ---------------------------------------------------------------------------
// Catalog row projection tests (KYO-447)
//
// `build_catalog_tree` is a pure function pulled out of `get_catalog_tree`
// specifically so the tree-shape logic can be exercised here without a
// database or a Leptos/Axum request context — see `overlay_credentials`
// in `server_fns/datasources.rs` for the established precedent of testing
// a server_fn's extracted helper directly via `use super::*`.
//
// These tests prove the KYO-447 projection change (fetching `table_type`
// as a scalar instead of the full `table_metadata` blob when
// `include_columns` is false) did not change the tree the user sees.
// ---------------------------------------------------------------------------
#[cfg(all(test, feature = "ssr"))]
mod catalog_tree_projection_tests {
    use super::{
        build_catalog_tree, CatalogTableSummaryRow, CatalogTableWithColumnsRow, TreeTable,
    };
    use crate::pages::sql_editor::types::{CatalogNode, CatalogNodeType};
    use serde_json::json;

    /// The registry's BigQuery metadata: `project` > `dataset`, no wrapper
    /// skipping — a plain two-level tree, exercised via the real registry
    /// rather than a hand-built fixture.
    fn bigquery_meta() -> &'static kyomi_core::datasource_registry::DatasourceTypeMetadata {
        kyomi_core::datasource_registry::get_metadata_by_str("bigquery")
            .expect("bigquery metadata must be registered")
    }

    fn find_project<'a>(tree: &'a [CatalogNode], name: &str) -> &'a CatalogNode {
        tree.iter()
            .find(|n| n.name == name)
            .unwrap_or_else(|| panic!("project node '{name}' not found in tree: {tree:?}"))
    }

    fn find_child<'a>(node: &'a CatalogNode, name: &str) -> &'a CatalogNode {
        node.children
            .iter()
            .find(|n| n.name == name)
            .unwrap_or_else(|| panic!("child node '{name}' not found under '{}': {node:?}", node.name))
    }

    /// `include_columns = false` is modeled by rows fetched with
    /// `CatalogTableSummaryRow` (no `table_metadata`, just an extracted
    /// `table_type`). The tree must still classify a view as a view — the
    /// projection must not silently fall back to "TABLE" for every row.
    #[test]
    fn summary_row_with_view_type_classifies_as_view_with_no_children() {
        let rows = vec![TreeTable::from(CatalogTableSummaryRow {
            project_id: "proj".to_string(),
            dataset_id: "ds".to_string(),
            table_id: "my_view".to_string(),
            table_type: Some("VIEW".to_string()),
        })];

        let tree = build_catalog_tree(rows, bigquery_meta());

        let project = find_project(&tree, "proj");
        let dataset = find_child(project, "ds");
        let table = find_child(dataset, "my_view");

        assert_eq!(table.node_type, CatalogNodeType::View);
        assert!(
            table.children.is_empty(),
            "include_columns = false must never populate column children, got {:?}",
            table.children
        );
        assert_eq!(table.full_name, Some("proj.ds.my_view".to_string()));
    }

    /// `include_columns = false` for an ordinary table (no `table_type` in
    /// the projected column, e.g. legacy rows) falls back to "TABLE" —
    /// unchanged from the pre-projection default.
    #[test]
    fn summary_row_with_no_table_type_defaults_to_table() {
        let rows = vec![TreeTable::from(CatalogTableSummaryRow {
            project_id: "proj".to_string(),
            dataset_id: "ds".to_string(),
            table_id: "legacy_row".to_string(),
            table_type: None,
        })];

        let tree = build_catalog_tree(rows, bigquery_meta());
        let table = find_child(find_child(find_project(&tree, "proj"), "ds"), "legacy_row");

        assert_eq!(table.node_type, CatalogNodeType::Table);
        assert!(table.children.is_empty());
    }

    /// `include_columns = true` is modeled by `CatalogTableWithColumnsRow`
    /// (full `table_metadata`). Column children must be built from the
    /// `columns` array, with fully-qualified names, and the view
    /// classification must still be read correctly out of the blob.
    #[test]
    fn with_columns_row_yields_column_children_and_view_classification() {
        let rows = vec![TreeTable::from(CatalogTableWithColumnsRow {
            project_id: "proj".to_string(),
            dataset_id: "ds".to_string(),
            table_id: "my_view".to_string(),
            table_metadata: json!({
                "table_type": "VIEW",
                "columns": [
                    { "name": "id", "type": "INT64" },
                    { "name": "name", "type": "STRING" },
                ],
            }),
        })];

        let tree = build_catalog_tree(rows, bigquery_meta());
        let table = find_child(find_child(find_project(&tree, "proj"), "ds"), "my_view");

        assert_eq!(table.node_type, CatalogNodeType::View);
        assert_eq!(table.children.len(), 2, "expected both columns as children");

        let id_col = find_child(table, "id");
        assert_eq!(id_col.node_type, CatalogNodeType::Column("INT64".to_string()));
        assert_eq!(id_col.full_name, Some("proj.ds.my_view.id".to_string()));

        let name_col = find_child(table, "name");
        assert_eq!(
            name_col.node_type,
            CatalogNodeType::Column("STRING".to_string())
        );
    }

    /// The core KYO-447 regression guard: for a row with no columns to
    /// show, the tree built from the narrow `include_columns = false`
    /// projection must be byte-for-byte identical to the tree built from
    /// the full `include_columns = true` projection of the *same*
    /// underlying row. This is what "projection, not a behavior change"
    /// means — the two SQL shapes must be interchangeable whenever the
    /// caller doesn't need column children.
    #[test]
    fn same_row_produces_identical_tree_from_either_projection() {
        let via_summary = vec![TreeTable::from(CatalogTableSummaryRow {
            project_id: "proj".to_string(),
            dataset_id: "ds".to_string(),
            table_id: "orders".to_string(),
            table_type: Some("TABLE".to_string()),
        })];
        let via_full_metadata = vec![TreeTable::from(CatalogTableWithColumnsRow {
            project_id: "proj".to_string(),
            dataset_id: "ds".to_string(),
            table_id: "orders".to_string(),
            table_metadata: json!({ "table_type": "TABLE", "columns": [] }),
        })];

        let tree_from_summary = build_catalog_tree(via_summary, bigquery_meta());
        let tree_from_full = build_catalog_tree(via_full_metadata, bigquery_meta());

        assert_eq!(
            tree_from_summary, tree_from_full,
            "narrow (include_columns=false) and full (include_columns=true, no \
             columns) projections of the same row must produce the same tree"
        );
    }

    /// Two tables in the same dataset must stay ordered by name regardless
    /// of fetch order, and projects/datasets are ordered lexically —
    /// confirms the `BTreeMap`-keyed ordering in `build_catalog_tree`
    /// survived the refactor.
    #[test]
    fn tables_within_a_dataset_are_sorted_by_name() {
        let rows = vec![
            TreeTable::from(CatalogTableSummaryRow {
                project_id: "proj".to_string(),
                dataset_id: "ds".to_string(),
                table_id: "zzz_table".to_string(),
                table_type: Some("TABLE".to_string()),
            }),
            TreeTable::from(CatalogTableSummaryRow {
                project_id: "proj".to_string(),
                dataset_id: "ds".to_string(),
                table_id: "aaa_table".to_string(),
                table_type: Some("TABLE".to_string()),
            }),
        ];

        let tree = build_catalog_tree(rows, bigquery_meta());
        let dataset = find_child(find_project(&tree, "proj"), "ds");
        let names: Vec<&str> = dataset.children.iter().map(|n| n.name.as_str()).collect();

        assert_eq!(names, vec!["aaa_table", "zzz_table"]);
    }
}

// ── JSON input codec on a self-describing Vec<Vec<Value>> (KYO-459) ───────

#[cfg(all(test, feature = "ssr"))]
mod chart_json_codec_tests {
    //! KYO-459: `generate_chart_from_results` takes `sample_rows:
    //! Vec<Vec<serde_json::Value>>` — the caller
    //! (`crates/kyomi-ui/src/pages/sql_editor/results_container.rs:321-355`)
    //! builds each leaf via `s.parse::<f64>().map(|n| serde_json::json!(n))`,
    //! so a numeric column genuinely arrives here as `Value::Number`, not a
    //! string dressed up as JSON.
    //!
    //! `serde_json::Value` is self-describing, so its `Deserialize` impl
    //! defers entirely to the format doing the decoding. Under the
    //! `#[server]` macro's default input codec (`PostUrl` — `serde_qs` over
    //! `application/x-www-form-urlencoded`), every leaf decodes as a JSON
    //! *string*, because `serde_qs` has no type information beyond "this
    //! looks like text in a form field". This is the identical root cause
    //! KYO-428 fixed for `connection_config`/`credentials` in
    //! `datasources.rs` — confirmed here empirically (not just assumed from
    //! the ticket) by round-tripping a `GenerateChartFromResults`-shaped
    //! payload with numeric `sample_rows` leaves through the real
    //! `FromReq<PostUrl, ...>` codec `server_fn` uses, and observing that
    //! `analyze_chart_column`'s `is_numeric` check
    //! (`values.iter().all(|v| v.is_number())`, above in this file) then
    //! misclassifies the numeric column as categorical.
    //!
    //! The fix is `#[server(prefix = "/leptos-api", input =
    //! server_fn::codec::Json)]`, matching the existing precedent at
    //! `datasources.rs`'s `create_datasource_modal` (KYO-428). The second
    //! test below (`generate_chart_from_results_uses_the_json_input_codec`)
    //! follows that same file's `json_input_codec_tests` shape and is the
    //! one that actually guards the fix: it inspects the macro-generated
    //! `ServerFn::Protocol` for this function and fails if `input = ...` is
    //! ever removed or edited back to the default. The first test below,
    //! unlike the second, invokes the `PostUrl` codec directly rather than
    //! through this function's configured `Protocol`, so on its own it
    //! cannot detect a codec regression — its purpose is solely to
    //! establish, empirically, that `PostUrl` really does corrupt this
    //! data shape (and its downstream `is_numeric` consequence), which is
    //! why it is kept even though the second test is the actual guard.
    //!
    //! Verified by mutation: temporarily deleting `input =
    //! server_fn::codec::Json` from `generate_chart_from_results` turns its
    //! `Protocol`'s input slot back into `server_fn::codec::url::PostUrl`,
    //! and `generate_chart_from_results_uses_the_json_input_codec` fails
    //! with exactly the message below. The attribute was restored
    //! immediately after and this test file was confirmed to pass again.

    use super::{analyze_chart_column, GenerateChartFromResults};
    use axum::body::Body;
    use axum::http::Request;
    use leptos::prelude::ServerFnError;
    use leptos::server_fn::codec::{FromReq, PostUrl};
    use leptos::server_fn::ServerFn;

    /// A `PostUrl` (`serde_qs`) body a real browser client would have sent
    /// for `generate_chart_from_results` before this fix, for two rows of
    /// `(region: string, revenue: number)`. Captured from
    /// `serde_qs::to_string` against a `GenerateChartFromResults`-shaped
    /// struct — field order `columns`, `sample_rows`, `sql`,
    /// `datasource_slug` matches what `#[server]` generates for this
    /// function's args struct — with `sample_rows` leaves
    /// `Value::String("us-east")`, `Value::Number(1234.5)`,
    /// `Value::String("us-west")`, `Value::Number(998)`.
    const POST_URL_BODY: &str = "columns[0]=region&columns[1]=revenue&\
         sample_rows[0][0]=us-east&sample_rows[0][1]=1234.5&\
         sample_rows[1][0]=us-west&sample_rows[1][1]=998&\
         sql=select+region%2C+revenue+from+sales&datasource_slug=warehouse";

    #[tokio::test]
    async fn post_url_codec_stringifies_numeric_sample_row_leaves_and_analyze_chart_column_misclassifies_them(
    ) {
        let req = Request::post("/leptos-api/generate_chart_from_results")
            .header("content-type", "application/x-www-form-urlencoded")
            .body(Body::from(POST_URL_BODY))
            .expect("request must build");

        let decoded =
            <GenerateChartFromResults as FromReq<PostUrl, Request<Body>, ServerFnError>>::from_req(
                req,
            )
            .await
            .expect("serde_qs must decode a well-formed PostUrl body");

        assert_eq!(decoded.columns, vec!["region".to_string(), "revenue".to_string()]);
        assert_eq!(
            decoded.sample_rows[0][1],
            serde_json::Value::String("1234.5".to_string()),
            "PostUrl/serde_qs must have stringified the numeric revenue \
             leaf 1234.5 — if this now decodes as Value::Number, the \
             premise this test exists to pin no longer holds and this test \
             module should be reconsidered"
        );
        assert_eq!(
            decoded.sample_rows[1][1],
            serde_json::Value::String("998".to_string())
        );

        let revenue_analysis =
            analyze_chart_column("revenue", &decoded.sample_rows, &decoded.columns);
        assert!(
            !revenue_analysis.is_numeric,
            "this is the exact KYO-459 consequence: a genuinely numeric \
             column decodes as strings under PostUrl, so \
             analyze_chart_column's is_numeric check \
             (values.iter().all(|v| v.is_number())) is false and the \
             column is misclassified as categorical"
        );
    }

    /// Extract the type name of the *first* generic argument of
    /// `server_fn::Http<Input, Output>` from a full `type_name::<Protocol>()`
    /// string — mirrors `datasources.rs`'s `json_input_codec_tests` helper
    /// of the same name; duplicated here rather than shared because this
    /// ticket's scope is limited to this file (KYO-459).
    fn input_encoding_of(protocol_type_name: &str) -> &str {
        protocol_type_name
            .split_once("Http<")
            .and_then(|(_, rest)| rest.split_once(','))
            .map(|(input, _)| input)
            .unwrap_or_else(|| {
                panic!(
                    "expected `{protocol_type_name}` to be a server_fn::Http<Input, Output> \
                     protocol with a comma-separated generic argument list"
                )
            })
    }

    #[test]
    fn generate_chart_from_results_uses_the_json_input_codec() {
        let protocol =
            std::any::type_name::<<GenerateChartFromResults as ServerFn>::Protocol>();
        let input_encoding = input_encoding_of(protocol);
        assert!(
            !input_encoding.contains("PostUrl"),
            "expected a JSON input codec, but {protocol} still uses the \
             default form-urlencoded PostUrl codec — this is the exact \
             KYO-459 regression: serde_json::Value leaves in sample_rows \
             that are numbers silently decode as strings under PostUrl, \
             misclassifying every numeric column as categorical in \
             analyze_chart_column"
        );
        assert!(
            input_encoding.contains("JsonEncoding"),
            "expected the input encoding to be server_fn::codec::Json \
             (JsonEncoding), got {input_encoding} in protocol {protocol}"
        );
    }
}
