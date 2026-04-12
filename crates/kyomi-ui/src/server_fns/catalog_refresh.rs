// SPDX-License-Identifier: AGPL-3.0-or-later

//! Shared catalog refresh service — core orchestration logic used by both the
//! REST API handler (`apps/server/src/routes/catalog.rs`) and the Leptos server
//! function (`server_fns/sql_editor.rs::refresh_catalog`).
//!
//! This module owns the "build provider → discover containers → index tables →
//! archive stale entries → populate embeddings" pipeline for all datasource
//! types, including BigQuery's REST-API-based indexing path.

use std::collections::HashSet;
use std::sync::Arc;

use serde_json::Value;

use kyomi_auth::catalog::helpers::{
    archive_missing_tables, cache_table, update_datasource_last_refresh, update_workspace_status,
    CacheTableParams, IndexerContext,
};
use kyomi_auth::catalog::sql_helpers::{get_columns_sql, get_tables_in_container_sql};
use kyomi_auth::catalog::types::ColumnEntry;
use kyomi_core::datasource_registry::{self, DatasourceType};
use kyomi_core::models::datasource::DatasourceConfig;
use kyomi_core::DbPool;
use kyomi_datasource_server::ConnectRegistry;

// ---------------------------------------------------------------------------
// Public result type
// ---------------------------------------------------------------------------

/// Outcome of a catalog refresh operation.
#[derive(Debug, Clone)]
pub struct CatalogRefreshResult {
    pub status: String,
    pub message: String,
    pub datasource_id: String,
}

// ---------------------------------------------------------------------------
// Parameters
// ---------------------------------------------------------------------------

/// Everything the catalog refresh pipeline needs, gathered by the caller.
///
/// Both the REST handler and the Leptos server function construct this from
/// their respective state types (`AppState` / `ServerContext`).
pub struct CatalogRefreshParams<'a> {
    pub db: &'a DbPool,
    pub embedding: &'a kyomi_embed::EmbeddingService,
    pub encryption_key: &'a Arc<[u8; 32]>,
    pub datasource: DatasourceConfig,
    pub workspace_id: &'a str,
    pub user_id: &'a str,
    pub force: bool,
    /// Required for Connect-type datasources. `None` is fine when the
    /// datasource uses a direct connection.
    pub connect_registry: Option<&'a ConnectRegistry>,
    /// User context for OAuth-based providers (BigQuery, etc.).
    pub user_context: Option<kyomi_datasource_server::UserContext>,
    /// Pre-resolved, decrypted credentials for the user+datasource pair.
    /// Caller is responsible for decryption and OAuth refresh before calling.
    pub credentials: Value,
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

/// Execute the full catalog refresh pipeline for any datasource type.
///
/// This is the single source of truth for catalog refresh orchestration.
/// Callers (REST handler, Leptos server fn) are responsible for:
/// - Permission checks (admin-only)
/// - Sample datasource rejection
/// - Resolving credentials (decrypt + OAuth refresh)
/// - Building `UserContext`
pub async fn execute_catalog_refresh(
    params: CatalogRefreshParams<'_>,
) -> Result<CatalogRefreshResult, kyomi_core::Error> {
    let datasource = &params.datasource;

    tracing::info!(
        datasource_id = %datasource.id,
        datasource_type = %datasource.datasource_type,
        user_id = %params.user_id,
        "Starting catalog refresh"
    );

    // Rate limit check (unless force=true). Manual refresh uses 0 threshold
    // so any non-running datasource is eligible.
    if !params.force {
        let can_refresh =
            kyomi_auth::catalog::helpers::can_refresh_now(params.db, &datasource.id, 0).await;

        if !can_refresh {
            return Ok(CatalogRefreshResult {
                status: "already_running".into(),
                message: "Catalog indexing is already in progress for this datasource".into(),
                datasource_id: datasource.id.clone(),
            });
        }
    }

    // Dispatch based on connection type.
    if datasource.connection_type == "connect" {
        refresh_connect(params).await
    } else {
        let ds_type: DatasourceType = datasource.datasource_type.into();
        if ds_type == DatasourceType::BigQuery {
            refresh_bigquery_rest(params).await
        } else {
            refresh_sql_based(params, ds_type).await
        }
    }
}

// ---------------------------------------------------------------------------
// Connect datasource refresh
// ---------------------------------------------------------------------------

async fn refresh_connect(
    params: CatalogRefreshParams<'_>,
) -> Result<CatalogRefreshResult, kyomi_core::Error> {
    let datasource = &params.datasource;
    let registry = params.connect_registry.ok_or_else(|| {
        kyomi_core::Error::BadRequest("Connect registry not available".into())
    })?;

    let provider = kyomi_datasource_server::ConnectProvider::with_timeout(
        registry.clone(),
        datasource.id.clone(),
        std::time::Duration::from_secs(120),
    );

    // Test connection first.
    use kyomi_datasource_server::provider::DatasourceProvider as _;
    if let Err(e) = provider.test_connection().await {
        tracing::warn!(
            datasource_id = %datasource.id,
            error = %e,
            "Connection test failed during Connect catalog refresh"
        );
        return Ok(CatalogRefreshResult {
            status: "error".into(),
            message: "Connection test failed — is the Connect binary running?".into(),
            datasource_id: datasource.id.clone(),
        });
    }

    // Update workspace status to running.
    let _ = update_workspace_status(
        params.db,
        params.workspace_id,
        &datasource.id,
        "running",
        None,
    )
    .await;

    // Send discover_catalog command.
    let catalog_result = match provider.discover_catalog().await {
        Ok(cr) => cr,
        Err(e) => {
            tracing::warn!(
                datasource_id = %datasource.id,
                error = %e,
                "discover_catalog command failed"
            );
            let _ = update_workspace_status(
                params.db,
                params.workspace_id,
                &datasource.id,
                "failed",
                None,
            )
            .await;
            return Ok(CatalogRefreshResult {
                status: "error".into(),
                message: format!("Catalog discovery failed: {e}"),
                datasource_id: datasource.id.clone(),
            });
        }
    };

    // Process CatalogResult into cache_table calls.
    let mut tables_indexed = 0usize;
    let mut seen_table_ids = HashSet::new();

    let ctx = IndexerContext {
        workspace_id: params.workspace_id.to_string(),
        datasource_config_id: datasource.id.clone(),
        connection_config: datasource.connection_config.clone(),
        encryption_key: Arc::clone(params.encryption_key),
    };

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
            let archive_id =
                kyomi_core::build_full_table_name(project_id, dataset_id, table_name);
            seen_table_ids.insert(archive_id);

            let cached = cache_table(CacheTableParams {
                db: params.db,
                embedding: params.embedding,
                ctx: &ctx,
                project_id,
                dataset_id,
                table_name,
                table_type,
                columns: &columns,
                full_table_id: &full_table_id,
            })
            .await;

            if cached {
                tables_indexed += 1;
            }
        }
    }

    finalize_refresh(params.db, params.workspace_id, &datasource.id, &seen_table_ids, tables_indexed, &[], params.embedding).await
}

// ---------------------------------------------------------------------------
// BigQuery REST API refresh (datasets.list / tables.list / tables.get)
// ---------------------------------------------------------------------------

async fn refresh_bigquery_rest(
    params: CatalogRefreshParams<'_>,
) -> Result<CatalogRefreshResult, kyomi_core::Error> {
    let datasource = &params.datasource;

    let catalog_projects: Vec<String> = datasource
        .connection_config
        .get("catalog_projects")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();

    if catalog_projects.is_empty() {
        let _ = update_workspace_status(
            params.db,
            params.workspace_id,
            &datasource.id,
            "idle",
            None,
        )
        .await;
        return Ok(CatalogRefreshResult {
            status: "completed".into(),
            message: "No projects configured for catalog indexing. Add projects in datasource settings.".into(),
            datasource_id: datasource.id.clone(),
        });
    }

    // Update workspace status to running.
    let _ = update_workspace_status(
        params.db,
        params.workspace_id,
        &datasource.id,
        "running",
        None,
    )
    .await;

    // Resolve access token using the datasource's configured auth mode.
    let user_context_ref = params.user_context.as_ref();
    let access_token = kyomi_datasource_server::providers::bigquery::resolve_access_token(
        &datasource.connection_config,
        &params.credentials,
        user_context_ref,
    )
    .await?;

    let http_client = kyomi_datasource_server::http_client()?;
    let mut tables_indexed = 0usize;
    let mut seen_table_ids = HashSet::new();
    let mut errors: Vec<String> = Vec::new();

    let ctx = IndexerContext {
        workspace_id: params.workspace_id.to_string(),
        datasource_config_id: datasource.id.clone(),
        connection_config: datasource.connection_config.clone(),
        encryption_key: Arc::clone(params.encryption_key),
    };

    use kyomi_auth::catalog::indexers::user_dataset::{
        get_bigquery_table_schema, list_bigquery_datasets, list_bigquery_tables,
    };

    for project in &catalog_projects {
        // List datasets via REST API (like Python's bq_client.list_datasets).
        let datasets = match list_bigquery_datasets(&http_client, &access_token, project).await {
            Ok(ds) => ds,
            Err(e) => {
                tracing::warn!(
                    project = %project,
                    "Failed to list datasets for BigQuery project: {e}"
                );
                errors.push(format!("{project}: failed to list datasets"));
                continue;
            }
        };

        for dataset_id in &datasets {
            // List tables via REST API (like Python's bq_client.list_tables).
            let tables = match list_bigquery_tables(
                &http_client,
                &access_token,
                project,
                dataset_id,
            )
            .await
            {
                Ok(t) => t,
                Err(e) => {
                    tracing::warn!(
                        project = %project,
                        dataset = %dataset_id,
                        "Failed to list tables: {e}"
                    );
                    errors.push(format!("{project}.{dataset_id}: failed to list tables"));
                    continue;
                }
            };

            for table_id in &tables {
                let full_table_id =
                    kyomi_core::build_full_table_name(project, dataset_id, table_id);
                seen_table_ids.insert(full_table_id.clone());

                // Get schema via REST API (like Python's bq_client.get_table).
                let columns = match get_bigquery_table_schema(
                    &http_client,
                    &access_token,
                    project,
                    dataset_id,
                    table_id,
                )
                .await
                {
                    Ok(c) => c,
                    Err(e) => {
                        tracing::warn!(
                            table = %full_table_id,
                            "Could not fetch schema, skipping: {e}"
                        );
                        continue;
                    }
                };

                let cached = cache_table(CacheTableParams {
                    db: params.db,
                    embedding: params.embedding,
                    ctx: &ctx,
                    project_id: project,
                    dataset_id,
                    table_name: table_id,
                    table_type: "TABLE",
                    columns: &columns,
                    full_table_id: &full_table_id,
                })
                .await;

                if cached {
                    tables_indexed += 1;
                }
            }
        }
    }

    finalize_refresh(
        params.db,
        params.workspace_id,
        &datasource.id,
        &seen_table_ids,
        tables_indexed,
        &errors,
        params.embedding,
    )
    .await
}

// ---------------------------------------------------------------------------
// SQL-based datasource refresh (Postgres, MySQL, ClickHouse, etc.)
// ---------------------------------------------------------------------------

async fn refresh_sql_based(
    params: CatalogRefreshParams<'_>,
    ds_type: DatasourceType,
) -> Result<CatalogRefreshResult, kyomi_core::Error> {
    let datasource = &params.datasource;
    let user_context_ref = params.user_context.as_ref();

    // Create provider.
    let provider = match kyomi_datasource_server::create_provider(
        &ds_type,
        &datasource.connection_config,
        &params.credentials,
        user_context_ref,
    )
    .await
    {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(
                datasource_id = %datasource.id,
                error = %e,
                "Failed to create provider for catalog refresh"
            );
            return Ok(CatalogRefreshResult {
                status: "error".into(),
                message: "Failed to connect to datasource".into(),
                datasource_id: datasource.id.clone(),
            });
        }
    };

    // Test connection.
    if let Err(e) = provider.test_connection().await {
        tracing::warn!(
            datasource_id = %datasource.id,
            error = %e,
            "Connection test failed during catalog refresh"
        );
        provider.close().await;
        return Ok(CatalogRefreshResult {
            status: "error".into(),
            message: "Connection test failed — check datasource credentials and network access"
                .into(),
            datasource_id: datasource.id.clone(),
        });
    }

    // Update workspace status to running.
    let _ = update_workspace_status(
        params.db,
        params.workspace_id,
        &datasource.id,
        "running",
        None,
    )
    .await;

    // Resolve containers to index.
    //
    // Matches Python's BaseSQLCatalogIndexer._get_catalog_containers() logic:
    // 1. If connection_config has a configured container list — use those directly.
    // 2. If configured as empty [] — user explicitly chose nothing, index nothing.
    // 3. If not configured (key missing) — discover all via provider methods.
    let meta = datasource_registry::get_metadata(&ds_type);
    let container_key = meta.catalog_config_keys.first().copied().unwrap_or("");

    let configured = if !container_key.is_empty() {
        datasource.connection_config.get(container_key)
    } else {
        None
    };

    let containers: Vec<String> = if let Some(Value::Array(arr)) = configured {
        if arr.is_empty() {
            // Empty array — user explicitly chose none — index nothing.
            tracing::info!(
                container_key,
                datasource = %datasource.name,
                "Empty container config — nothing to index"
            );
            vec![]
        } else {
            // User configured specific containers — use those.
            let items: Vec<String> = arr
                .iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect();
            tracing::info!(
                container_key,
                count = items.len(),
                datasource = %datasource.name,
                "Using configured containers for catalog refresh"
            );
            items
        }
    } else {
        // Not configured — discover all.
        tracing::info!(
            container_key,
            datasource = %datasource.name,
            "No container config — discovering all"
        );
        let discovery = discover_primary(provider.as_ref(), &ds_type).await;
        if let Some(err) = discovery.error {
            tracing::warn!(
                datasource_id = %datasource.id,
                error = %err,
                "Failed to discover containers during catalog refresh"
            );
            provider.close().await;
            let _ = update_workspace_status(
                params.db,
                params.workspace_id,
                &datasource.id,
                "failed",
                None,
            )
            .await;
            return Ok(CatalogRefreshResult {
                status: "error".into(),
                message:
                    "Failed to discover catalog containers — check datasource connectivity".into(),
                datasource_id: datasource.id.clone(),
            });
        }
        discovery.items
    };

    // Index tables in each container.
    let mut tables_indexed = 0usize;
    let mut seen_table_ids = HashSet::new();
    let mut errors: Vec<String> = Vec::new();

    // Analytics datasources use _-prefixed hidden tables for transforms;
    // only the public views (without _) should be indexed.
    let is_analytics = datasource
        .connection_config
        .get("analytics_site_id")
        .and_then(|v| v.as_str())
        .is_some();

    tracing::info!(
        containers = ?containers,
        datasource = %datasource.name,
        "Starting catalog indexing for {} containers",
        containers.len()
    );

    let ds_type_str = ds_type.as_str();
    let has_schema_column = ds_type_str == "snowflake" || ds_type_str == "databricks";

    for container in &containers {
        let tables_sql = get_tables_in_container_sql(ds_type_str, container);
        let Some(sql) = tables_sql else {
            tracing::warn!(
                container,
                ds_type = ds_type_str,
                "No table listing SQL for datasource type — skipping container"
            );
            continue;
        };

        let table_rows = match provider.execute_query(&sql, None, None, false).await {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(
                    container,
                    error = %e,
                    "Failed to list tables in container"
                );
                errors.push(format!("Failed to list tables in '{container}': {e}"));
                continue;
            }
        };

        let rows = table_rows.rows.as_deref().unwrap_or(&[]);
        tracing::info!(
            container,
            tables_found = rows.len(),
            "Listed tables in container"
        );

        for table_row in rows {
            // Snowflake and Databricks return (TABLE_SCHEMA, TABLE_NAME, TABLE_TYPE)
            // because tables span multiple schemas within each database/catalog.
            // Other SQL datasources return just (TABLE_NAME).
            let (table_name, effective_container): (&str, String) = if has_schema_column {
                let schema_name = match table_row.first().and_then(|v| v.as_str()) {
                    Some(s) if !s.is_empty() => s,
                    _ => continue,
                };
                let tbl = match table_row.get(1).and_then(|v| v.as_str()) {
                    Some(t) if !t.is_empty() => t,
                    _ => continue,
                };
                // effective_container = "database.schema" or "catalog.schema"
                (tbl, format!("{container}.{schema_name}"))
            } else {
                let tbl = match table_row.first().and_then(|v| v.as_str()) {
                    Some(t) => t,
                    None => continue,
                };
                (tbl, container.clone())
            };

            // Skip hidden transform tables for analytics datasources.
            if is_analytics && table_name.starts_with('_') {
                continue;
            }

            // Get columns — pass effective_container so Snowflake/Databricks
            // can split "database.schema" for the INFORMATION_SCHEMA query.
            let columns_sql = get_columns_sql(ds_type_str, &effective_container, table_name);
            let columns = if let Some(sql) = columns_sql {
                match provider.execute_query(&sql, None, None, false).await {
                    Ok(result) => result
                        .rows
                        .as_deref()
                        .unwrap_or(&[])
                        .iter()
                        .map(|row| ColumnEntry {
                            name: row
                                .first()
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string(),
                            col_type: row.get(1).and_then(|v| v.as_str()).map(String::from),
                            native_type: row.get(1).and_then(|v| v.as_str()).map(String::from),
                            description: row.get(2).and_then(|v| v.as_str()).map(String::from),
                        })
                        .collect::<Vec<_>>(),
                    Err(_) => Vec::new(),
                }
            } else {
                Vec::new()
            };

            let project_id = "";
            let dataset_id = effective_container.as_str();
            let archive_id =
                kyomi_core::build_full_table_name(project_id, dataset_id, table_name);
            seen_table_ids.insert(archive_id);

            let ctx = IndexerContext {
                workspace_id: params.workspace_id.to_string(),
                datasource_config_id: datasource.id.clone(),
                connection_config: datasource.connection_config.clone(),
                encryption_key: Arc::clone(params.encryption_key),
            };

            let full_table_id = format!("{effective_container}.{table_name}");
            let cached = cache_table(CacheTableParams {
                db: params.db,
                embedding: params.embedding,
                ctx: &ctx,
                project_id,
                dataset_id,
                table_name,
                table_type: "TABLE",
                columns: &columns,
                full_table_id: &full_table_id,
            })
            .await;

            if cached {
                tables_indexed += 1;
            }
        }
    }

    provider.close().await;

    finalize_refresh(
        params.db,
        params.workspace_id,
        &datasource.id,
        &seen_table_ids,
        tables_indexed,
        &errors,
        params.embedding,
    )
    .await
}

// ---------------------------------------------------------------------------
// Shared finalization (archive, timestamps, embeddings, result)
// ---------------------------------------------------------------------------

async fn finalize_refresh(
    db: &DbPool,
    workspace_id: &str,
    datasource_id: &str,
    seen_table_ids: &HashSet<String>,
    tables_indexed: usize,
    errors: &[String],
    embedding: &kyomi_embed::EmbeddingService,
) -> Result<CatalogRefreshResult, kyomi_core::Error> {
    // Archive missing tables.
    let archived_names = archive_missing_tables(db, workspace_id, datasource_id, seen_table_ids)
        .await
        .unwrap_or_default();
    let tables_archived = archived_names.len();

    // Update last refresh timestamp.
    let _ = update_datasource_last_refresh(db, datasource_id).await;

    // Update workspace status to idle.
    let _ = update_workspace_status(db, workspace_id, datasource_id, "idle", None).await;

    // Populate embeddings after successful indexing.
    if tables_indexed > 0 {
        populate_embeddings_after_indexing(db, embedding, workspace_id, datasource_id).await;
    }

    // Partial success (errors + some tables indexed) returns "completed" — this
    // aligns SQL-based and BigQuery paths to the same semantics. Only return
    // "error" when zero tables were indexed and errors occurred.
    if !errors.is_empty() && tables_indexed == 0 {
        Ok(CatalogRefreshResult {
            status: "error".into(),
            message: format!("Catalog refresh failed: {}", errors.join("; ")),
            datasource_id: datasource_id.to_string(),
        })
    } else {
        Ok(CatalogRefreshResult {
            status: "completed".into(),
            message: format!(
                "Catalog refreshed successfully. {} tables indexed, {} archived.",
                tables_indexed, tables_archived
            ),
            datasource_id: datasource_id.to_string(),
        })
    }
}

// ---------------------------------------------------------------------------
// Embedding population (fire-and-forget)
// ---------------------------------------------------------------------------

/// Generate embeddings for freshly indexed catalog data.
///
/// Failures are logged as warnings — they never fail the indexing response.
async fn populate_embeddings_after_indexing(
    db: &DbPool,
    embedding: &kyomi_embed::EmbeddingService,
    workspace_id: &str,
    datasource_config_id: &str,
) {
    match kyomi_knowledge::populate::populate_table_embeddings(
        db,
        embedding,
        workspace_id,
        datasource_config_id,
    )
    .await
    {
        Ok(table_count) => {
            match kyomi_knowledge::populate::populate_column_embeddings(
                db,
                embedding,
                workspace_id,
                datasource_config_id,
            )
            .await
            {
                Ok(col_count) => {
                    tracing::info!(
                        workspace_id,
                        datasource_config_id,
                        tables = table_count,
                        columns = col_count,
                        "Embeddings populated after catalog refresh"
                    );
                }
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        "Column embedding population failed, continuing"
                    );
                }
            }
        }
        Err(e) => {
            tracing::warn!(
                error = %e,
                "Table embedding population failed, continuing"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// SQL helpers — discovery dispatch and catalog SQL generation
// ---------------------------------------------------------------------------

/// Call the primary discovery method for a datasource type.
///
/// Matches Python's `meta.discovery_method` registry field:
/// - BigQuery → `list_projects()` (handled separately via REST)
/// - PostgreSQL, Redshift, SQL Server, Synapse → `list_schemas()`
/// - MySQL, ClickHouse, Snowflake → `list_databases()`
/// - Databricks → `list_catalogs()`
async fn discover_primary(
    provider: &dyn kyomi_datasource_server::DatasourceProvider,
    ds_type: &DatasourceType,
) -> kyomi_datasource_server::DiscoveryResult {
    match ds_type.as_str() {
        "postgres" | "redshift" | "sqlserver" | "synapse" => provider.list_schemas().await,
        "mysql" | "clickhouse" | "snowflake" => provider.list_databases().await,
        "databricks" => provider.list_catalogs().await,
        _ => kyomi_datasource_server::DiscoveryResult {
            items: vec![],
            error: Some(format!(
                "Discovery not available for datasource type '{}'",
                ds_type.as_str()
            )),
        },
    }
}

// SQL helper functions (escape_sql_literal, escape_sql_identifier,
// get_tables_in_container_sql, get_columns_sql) are imported from
// kyomi_auth::catalog::sql_helpers at the top of this file.
