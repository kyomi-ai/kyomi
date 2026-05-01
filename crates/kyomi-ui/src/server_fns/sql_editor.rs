// SPDX-License-Identifier: AGPL-3.0-or-later

//! Server functions for SQL Editor dry-run validation, history, catalog, and chart generation.

use leptos::prelude::*;
use serde::{Deserialize, Serialize};

#[cfg(feature = "ssr")]
use super::{AuthenticatedContext, IntoServerFnError};

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
#[server(prefix = "/leptos-api")]
pub async fn dry_run_sql(
    datasource_slug: String,
    sql: String,
) -> Result<DryRunResult, ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;

    let (_ds, provider) =
        super::datasources::create_query_provider(&ac.ctx, &ac.auth, &ac.ws_id, &datasource_slug).await?;

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

    provider.close().await;

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
    .into_sfn()?;

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
        .into_sfn()?;
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
    .into_sfn()?;

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
    .into_sfn()?;

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
    .into_sfn()?;

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
    use std::collections::BTreeMap;

    let ac = AuthenticatedContext::extract().await?;

    // Resolve datasource.
    let datasource = kyomi_auth::datasource_service::resolve_datasource(
        ac.db(),
        &datasource_slug,
        &ac.ws_id,
        false,
    )
    .await
    .into_sfn()?;

    // Fetch all non-archived cached tables for this datasource.
    //
    // Note: sample datasources used to live in a shared sentinel workspace
    // (`SAMPLE_DATA_WORKSPACE_ID`), so this query had an `is_sample` branch
    // that read from there. `onboarding::add_sample_datasource` switched to
    // the generic per-workspace indexer but this read site was missed, so
    // samples appeared empty in the UI. Query by `datasource_config_id`
    // uniformly — works for every datasource type including samples.
    let is_pg = ac.db().is_postgres();
    let bf = kyomi_core::sql_compat::bool_false(is_pg);
    let mut cached_tables: Vec<kyomi_core::models::table_cache::DatasourceTableCache> =
        kyomi_core::db_fetch_all!(
            ac.db(), kyomi_core::models::table_cache::DatasourceTableCache,
            &format!(
                "SELECT id, workspace_id, datasource_config_id, project_id, dataset_id, table_id, \
                 table_metadata, column_descriptions, created_at, updated_at, \
                 structure_refreshed_at, descriptions_refreshed_at, is_archived, last_verified \
                 FROM datasource_table_cache \
                 WHERE datasource_config_id = $1 AND is_archived = {bf}"
            ),
            &datasource.id
        )
        .into_sfn()?;

    // BigQuery public datasets: include if enabled (defaults to true).
    if datasource.datasource_type == kyomi_core::DatasourceType::Bigquery {
        let include_public = datasource
            .connection_config
            .get("include_public_datasets")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);

        if include_public {
            let public_tables: Vec<kyomi_core::models::table_cache::DatasourceTableCache> =
                kyomi_core::db_fetch_all!(
                    ac.db(), kyomi_core::models::table_cache::DatasourceTableCache,
                    &format!(
                        "SELECT id, workspace_id, datasource_config_id, project_id, dataset_id, table_id, \
                         table_metadata, column_descriptions, created_at, updated_at, \
                         structure_refreshed_at, descriptions_refreshed_at, is_archived, last_verified \
                         FROM datasource_table_cache \
                         WHERE workspace_id = $1 AND is_archived = {bf}"
                    ),
                    kyomi_auth::catalog::indexers::bigquery_public::PUBLIC_DATA_WORKSPACE_ID
                )
                .into_sfn()?;
            cached_tables.extend(public_tables);
        }
    }

    let table_count = cached_tables.len();

    // Build tree: {project_id: {dataset_id: [table_nodes]}}
    let mut tree_dict: BTreeMap<String, BTreeMap<String, Vec<CatalogNode>>> = BTreeMap::new();

    for table in &cached_tables {
        let project = &table.project_id;
        let dataset = &table.dataset_id;
        let table_name = &table.table_id;

        let project_map = tree_dict.entry(project.clone()).or_default();
        let table_list = project_map.entry(dataset.clone()).or_default();

        // Build fully-qualified table name.
        let full_name = if project.is_empty() {
            format!("{dataset}.{table_name}")
        } else {
            format!("{project}.{dataset}.{table_name}")
        };

        // Build column children if requested.
        let children = if include_columns {
            let columns = table
                .table_metadata
                .get("columns")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();

            let col_nodes: Vec<CatalogNode> = columns
                .iter()
                .filter_map(|col| {
                    let col_name = col.get("name")?.as_str()?;
                    let col_type = col
                        .get("type")
                        .and_then(|v| v.as_str())
                        .unwrap_or("unknown")
                        .to_string();
                    let col_full = if project.is_empty() {
                        format!("{dataset}.{table_name}.{col_name}")
                    } else {
                        format!("{project}.{dataset}.{table_name}.{col_name}")
                    };
                    Some(CatalogNode {
                        name: col_name.to_string(),
                        node_type: CatalogNodeType::Column(col_type),
                        children: Vec::new(),
                        full_name: Some(col_full),
                    })
                })
                .collect();

            col_nodes
        } else {
            Vec::new()
        };

        // Determine table vs view from metadata.
        let table_type_str = table
            .table_metadata
            .get("table_type")
            .and_then(|v| v.as_str())
            .unwrap_or("TABLE");
        let node_type = if table_type_str.to_uppercase().contains("VIEW") {
            CatalogNodeType::View
        } else {
            CatalogNodeType::Table
        };

        table_list.push(CatalogNode {
            name: table_name.clone(),
            node_type,
            children,
            full_name: Some(full_name),
        });
    }

    // Convert tree_dict to CatalogNode structure using registry metadata.
    let meta = kyomi_core::datasource_registry::get_metadata_by_str(
        datasource.datasource_type.as_ref(),
    )
    .ok_or_else(|| {
        ServerFnError::new(format!(
            "Unknown datasource type: '{}'",
            datasource.datasource_type.as_ref()
        ))
    })?;

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
    .into_sfn()?;

    // Sample datasources used to live in a shared sentinel workspace and
    // required an `is_sample` branch here to read from `SAMPLE_DATA_WORKSPACE_ID`.
    // Samples now index into the user's workspace via the generic indexer
    // (see `onboarding::add_sample_datasource`), so we query by
    // `datasource_config_id` uniformly.
    let is_pg = ac.db().is_postgres();
    let bf = kyomi_core::sql_compat::bool_false(is_pg);
    let ilike = kyomi_core::sql_compat::ilike(is_pg, "table_id", "$2");

    let search_pattern = format!("%{query}%");

    let sql = format!(
        "SELECT id, workspace_id, datasource_config_id, project_id, dataset_id, table_id, \
         table_metadata, column_descriptions, created_at, updated_at, \
         structure_refreshed_at, descriptions_refreshed_at, is_archived, last_verified \
         FROM datasource_table_cache \
         WHERE datasource_config_id = $1 AND is_archived = {bf} AND {ilike} \
         ORDER BY table_id \
         LIMIT 50"
    );
    let mut cached_tables: Vec<kyomi_core::models::table_cache::DatasourceTableCache> = match ac.db() {
        kyomi_core::db::DbPool::Postgres(pg) =>
            sqlx::query_as::<_, kyomi_core::models::table_cache::DatasourceTableCache>(&sql)
                .bind(&datasource.id)
                .bind(&search_pattern)
                .fetch_all(pg)
                .await
                .into_sfn()?,
        kyomi_core::db::DbPool::Sqlite(sq) =>
            sqlx::query_as::<_, kyomi_core::models::table_cache::DatasourceTableCache>(&sql)
                .bind(&datasource.id)
                .bind(&search_pattern)
                .fetch_all(sq)
                .await
                .into_sfn()?,
    };

    // BigQuery public datasets: include matching tables if enabled.
    if datasource.datasource_type == kyomi_core::DatasourceType::Bigquery {
        let include_public = datasource
            .connection_config
            .get("include_public_datasets")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);

        if include_public {
            let public_sql = format!(
                "SELECT id, workspace_id, datasource_config_id, project_id, dataset_id, table_id, \
                 table_metadata, column_descriptions, created_at, updated_at, \
                 structure_refreshed_at, descriptions_refreshed_at, is_archived, last_verified \
                 FROM datasource_table_cache \
                 WHERE workspace_id = $1 AND is_archived = {bf} AND {ilike} \
                 ORDER BY table_id \
                 LIMIT 50"
            );
            let public_tables: Vec<kyomi_core::models::table_cache::DatasourceTableCache> = match ac.db() {
                kyomi_core::db::DbPool::Postgres(pg) =>
                    sqlx::query_as::<_, kyomi_core::models::table_cache::DatasourceTableCache>(&public_sql)
                        .bind(kyomi_auth::catalog::indexers::bigquery_public::PUBLIC_DATA_WORKSPACE_ID)
                        .bind(&search_pattern)
                        .fetch_all(pg)
                        .await
                        .into_sfn()?,
                kyomi_core::db::DbPool::Sqlite(sq) =>
                    sqlx::query_as::<_, kyomi_core::models::table_cache::DatasourceTableCache>(&public_sql)
                        .bind(kyomi_auth::catalog::indexers::bigquery_public::PUBLIC_DATA_WORKSPACE_ID)
                        .bind(&search_pattern)
                        .fetch_all(sq)
                        .await
                        .into_sfn()?,
            };
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

            let table_type_str = table
                .table_metadata
                .get("table_type")
                .and_then(|v| v.as_str())
                .unwrap_or("TABLE");
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
/// Only workspace admins can trigger refreshes. Delegates to the shared
/// catalog refresh service in [`super::catalog_refresh`] which handles all
/// datasource types including BigQuery REST API indexing.
#[server(prefix = "/leptos-api")]
pub async fn refresh_catalog(
    datasource_slug: String,
) -> Result<(), ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;

    // Check admin permission.
    let is_admin = ac
        .auth
        .workspace
        .workspace_roles
        .contains(&kyomi_core::enums::WorkspaceRole::WorkspaceAdmin)
        || ac.auth.workspace.is_owner;

    if !is_admin {
        return Err(ServerFnError::new(
            "Only workspace admins can trigger catalog refresh",
        ));
    }

    let datasource = kyomi_auth::datasource_service::resolve_datasource(
        ac.db(),
        &datasource_slug,
        &ac.ws_id,
        false,
    )
    .await
    .into_sfn()?;

    // (The old "sample datasources cannot be refreshed manually" gate
    // was valid when samples lived in a shared sentinel workspace. Samples
    // are now indexed into the user's workspace by the generic per-workspace
    // indexer, so the normal refresh path works for them too.)

    let encryption_key = ac.encryption_key()?;

    let ds_type: kyomi_core::datasource_registry::DatasourceType =
        datasource.datasource_type.into();

    // Fetch and decrypt credentials in one service call.
    let (user_cred, credentials) =
        kyomi_auth::datasource_service::get_decrypted_user_credentials(
            ac.db(),
            &ac.auth.user_id,
            &datasource.id,
            &*encryption_key,
        )
        .await
        .into_sfn()?;

    // Refresh OAuth credentials if needed.
    let credentials = kyomi_datasource_server::ensure_valid_oauth_credentials(
        &credentials,
        &datasource.connection_config,
        &ds_type,
    )
    .await
    .into_sfn()?;

    // Persist refreshed token if it changed.
    if let Some(ref cred) = user_cred {
        let _ = kyomi_auth::datasource_service::save_user_credential(
            ac.db(),
            &*encryption_key,
            &ac.auth.user_id,
            &datasource.id,
            &cred.workspace_id,
            &credentials,
        )
        .await;
    }

    let user_context =
        super::datasources::build_user_context(&ac.ctx, &ac.auth).await?;

    let embedding = ac
        .ctx
        .embedding
        .wait_ready()
        .await
        .into_sfn()?;

    let params = super::catalog_refresh::CatalogRefreshParams {
        db: ac.db(),
        embedding,
        encryption_key: &encryption_key,
        datasource,
        workspace_id: &ac.ws_id,
        user_id: &ac.auth.user_id,
        force: false,
        connect_registry: ac.ctx.connect_registry.as_ref(),
        user_context,
        credentials,
    };

    let result = super::catalog_refresh::execute_catalog_refresh(params)
        .await
        .map_err(|e: kyomi_core::Error| ServerFnError::new(e.to_string()))?;

    if result.status == "error" {
        Err(ServerFnError::new(result.message))
    } else {
        Ok(())
    }
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
    .into_sfn()?;

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
                    .into_sfn()?
            }
            kyomi_core::db::DbPool::Sqlite(sq) => {
                sqlx::query_as::<_, kyomi_core::models::table_cache::DatasourceTableCache>(&sql)
                    .bind(&datasource.id)
                    .bind(dataset_id)
                    .bind(table_name)
                    .fetch_optional(sq)
                    .await
                    .into_sfn()?
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
                    .into_sfn()?
            }
            kyomi_core::db::DbPool::Sqlite(sq) => {
                sqlx::query_as::<_, kyomi_core::models::table_cache::DatasourceTableCache>(&sql)
                    .bind(&datasource.id)
                    .bind(project_id)
                    .bind(dataset_id)
                    .bind(table_name)
                    .fetch_optional(sq)
                    .await
                    .into_sfn()?
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
#[server(prefix = "/leptos-api")]
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
/// Matches the logic in `apps/server/src/routes/chart_generate.rs`
/// `generate_with_rules()`.
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
// Task 6.1: WebSocket connection info (user_id + workspace_id for WS URL)
// ---------------------------------------------------------------------------

/// Connection info needed to build the WebSocket URL on the client side.
///
/// The WebSocket connects to `/ws/{workspace_id}_{user_id}?token=...`
/// and needs both IDs.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WsConnectionInfo {
    pub user_id: String,
    pub workspace_id: String,
}

/// Return the current user's ID and workspace ID for WebSocket connection.
///
/// Called once when the SQL Editor page mounts to set up the streaming handler.
#[server(prefix = "/leptos-api")]
pub async fn get_ws_connection_info() -> Result<WsConnectionInfo, ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;

    Ok(WsConnectionInfo {
        user_id: ac.auth.user_id.clone(),
        workspace_id: ac.ws_id.clone(),
    })
}
