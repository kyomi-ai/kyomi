// SPDX-License-Identifier: AGPL-3.0-or-later

//! Catalog indexer traits and shared helpers.
//!
//! Defines:
//! - [`CatalogIndexer`] — async trait for all catalog indexers
//! - [`SQLCatalogIndexer`] — trait extension for SQL-based indexers
//! - [`IndexerContext`] — shared state for indexing operations
//! - Shared helper functions (caching, archiving, status updates)
//! - [`index_catalog_sql`] — template method for SQL-based catalog indexing
//!
//! Moved from `kyomi-auth/src/catalog/traits.rs` to break the cyclic dependency
//! (`kyomi-auth` → `kyomi-datasource` → `kyomi-auth`).

use async_trait::async_trait;
use chrono::Utc;
use kyomi_core::{DbPool, Result};
use kyomi_datasource_server::DatasourceProvider;
use kyomi_embed::EmbeddingService;
use serde_json::Value;
use tracing::{info, warn};

use kyomi_auth::catalog::helpers::{
    archive_missing_tables, cache_table, update_datasource_last_refresh, update_workspace_status,
    CacheTableParams, IndexerContext,
};
use kyomi_auth::catalog::types::{CatalogIndexResult, ColumnEntry, TableEntry};

// ─── CatalogIndexer trait ──────────────────────────────────────────────────────

/// Core catalog indexer trait.
///
/// All catalog indexers (SQL-based and BigQuery REST) implement this trait.
/// It provides the main entry point for indexing a datasource's catalog.
#[async_trait]
pub trait CatalogIndexer: Send + Sync {
    /// Index the catalog for this datasource.
    ///
    /// This is the primary entry point. Implementations should:
    /// 1. Resolve credentials
    /// 2. Connect to the datasource
    /// 3. Discover and cache tables with embeddings
    /// 4. Archive missing tables
    /// 5. Update workspace status
    async fn index_catalog(
        &self,
        ctx: &IndexerContext,
        db: &DbPool,
        embedding: &EmbeddingService,
        user_email: Option<&str>,
        credentials: Option<&Value>,
        max_tables_per_dataset: Option<usize>,
    ) -> CatalogIndexResult;

    /// Build the fully-qualified table identifier for this datasource type.
    ///
    /// Default: `"dataset_id.table_name"`.
    /// Override for datasources with different naming (e.g., BigQuery: `"project.dataset.table"`).
    fn build_full_table_id(&self, dataset_id: &str, table_name: &str) -> String {
        format!("{dataset_id}.{table_name}")
    }
}

// ─── SQLCatalogIndexer trait ───────────────────────────────────────────────────

/// SQL-specific catalog indexer trait.
///
/// For datasources that use SQL for catalog discovery (PostgreSQL, MySQL,
/// ClickHouse, Snowflake, Databricks, Redshift, SQL Server, Synapse).
///
/// Implementors provide 5 methods; the shared [`index_catalog_sql`] function
/// implements the complete indexing flow (template method pattern).
#[async_trait]
pub trait SQLCatalogIndexer: Send + Sync {
    /// Container label for this datasource type (e.g., "schema", "database", "catalog").
    fn container_label(&self) -> &str;

    /// Connection config key for catalog containers (e.g., "catalog_schemas", "catalog_databases").
    fn container_config_key(&self) -> &str;

    /// Create a provider instance with the given credentials.
    async fn create_provider(
        &self,
        connection_config: &Value,
        credentials: &Value,
    ) -> Result<Box<dyn DatasourceProvider>>;

    /// Discover all available containers, excluding system ones.
    ///
    /// Implementations should filter out provider-specific system containers
    /// (e.g., `information_schema`, `pg_catalog`, `system`).
    async fn discover_all_containers(
        &self,
        provider: &dyn DatasourceProvider,
    ) -> Result<Vec<String>>;

    /// Get all tables in a container.
    ///
    /// Returns table entries with name and type. Should escape special characters
    /// in `container_name` for use in SQL.
    async fn get_tables_in_container(
        &self,
        provider: &dyn DatasourceProvider,
        container_name: &str,
        max_tables: Option<usize>,
    ) -> Result<Vec<TableEntry>>;

    /// Get column metadata for a specific table.
    ///
    /// Returns column entries with name, type, native_type, and description.
    /// Should escape special characters in container/table names.
    async fn get_table_columns(
        &self,
        provider: &dyn DatasourceProvider,
        container_name: &str,
        table_name: &str,
    ) -> Result<Vec<ColumnEntry>>;

    /// Get the project_id for this datasource.
    ///
    /// Default: empty string. Override for datasources where project_id matters
    /// (e.g., PostgreSQL returns the database name).
    fn get_project_id(&self, _ctx: &IndexerContext) -> String {
        String::new()
    }

    /// Build the fully-qualified table identifier.
    ///
    /// Default: `"dataset_id.table_name"`. Override for non-standard naming.
    fn build_full_table_id(&self, dataset_id: &str, table_name: &str) -> String {
        format!("{dataset_id}.{table_name}")
    }
}

// ─── Shared helpers ────────────────────────────────────────────────────────────

// `can_refresh_now`, `archive_missing_tables`, `update_workspace_status`,
// `update_datasource_last_refresh`, `cache_table` all live in
// `kyomi_auth::catalog::helpers` and are imported above.

/// Resolve which containers to index using the template method pattern.
///
/// Logic:
/// 1. Check `connection_config[container_config_key]`
/// 2. If `None` or missing -> discover all containers (default for new datasources)
/// 3. If `[]` (empty list) -> return empty (user explicitly chose none)
/// 4. If `[...]` with items -> validate against available containers
pub async fn get_catalog_containers(
    ctx: &IndexerContext,
    indexer: &dyn SQLCatalogIndexer,
    provider: &dyn DatasourceProvider,
) -> Result<Vec<String>> {
    let config_key = indexer.container_config_key();
    let configured = ctx.connection_config.get(config_key);

    match configured {
        None => {
            // Not configured -> discover all available containers
            info!(
                workspace_id = ctx.workspace_id,
                config_key,
                "no catalog containers configured, discovering all"
            );
            indexer.discover_all_containers(provider).await
        }
        Some(Value::Null) => {
            // Explicitly null -> discover all
            indexer.discover_all_containers(provider).await
        }
        Some(Value::Array(arr)) if arr.is_empty() => {
            // Empty array -> user chose to index nothing
            info!(
                workspace_id = ctx.workspace_id,
                config_key,
                "catalog containers configured as empty, indexing nothing"
            );
            Ok(Vec::new())
        }
        Some(Value::Array(arr)) => {
            // Specific containers configured -> validate they exist
            let configured: Vec<String> = arr
                .iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect();

            let available = indexer.discover_all_containers(provider).await?;
            let valid: Vec<String> = configured
                .into_iter()
                .filter(|c| {
                    let exists = available.iter().any(|a| a.eq_ignore_ascii_case(c));
                    if !exists {
                        warn!(
                            workspace_id = ctx.workspace_id,
                            container = c,
                            label = indexer.container_label(),
                            "configured {} not found on server, skipping",
                            indexer.container_label()
                        );
                    }
                    exists
                })
                .collect();

            Ok(valid)
        }
        _ => {
            // Invalid type -> discover all as fallback
            warn!(
                workspace_id = ctx.workspace_id,
                config_key,
                "unexpected type for catalog config key, discovering all"
            );
            indexer.discover_all_containers(provider).await
        }
    }
}

// ─── Credential resolution ─────────────────────────────────────────────────────

/// Resolve credentials for catalog indexing.
///
/// Priority order:
/// 1. Provided credentials (if valid/non-empty)
/// 2. Shared credentials from connection_config
/// 3. User's stored credentials (from user_datasource_credentials)
///
/// Returns `None` if no credentials could be resolved.
pub async fn resolve_indexing_credentials(
    db: &DbPool,
    ctx: &IndexerContext,
    user_email: Option<&str>,
    provided_credentials: Option<&Value>,
) -> Option<Value> {
    let encryption_key = &*ctx.encryption_key;
    // 1. Use provided credentials if they have content
    if let Some(creds) = provided_credentials
        && creds.as_object().is_some_and(|o| !o.is_empty())
    {
        return Some(creds.clone());
    }

    // 2. Check shared credentials in connection_config
    let shared = kyomi_datasource_server::resolve_shared_credentials(
        &ctx.connection_config,
        &serde_json::json!({}),
    );
    if shared.is_object()
        && shared
            .as_object()
            .is_some_and(|obj| !obj.is_empty() && obj.values().any(|v| v.is_string()))
    {
        return Some(shared);
    }

    // 3. Look up user's stored credentials
    let email = user_email?;

    // Resolve user_id from email
    #[derive(sqlx::FromRow)]
    struct UserIdRow {
        user_id: String,
    }
    let user_id_row: Option<UserIdRow> = kyomi_core::db_fetch_optional!(
        db, UserIdRow,
        "SELECT user_id FROM users WHERE email = $1",
        &email
    )
    .ok()
    .flatten();

    let user_row = user_id_row?;

    let user_id = user_row.user_id;

    // Fetch the user's credential record
    let cred = kyomi_auth::datasource_service::get_user_credential(
        db,
        &user_id,
        &ctx.datasource_config_id,
    )
    .await
    .ok()
    .flatten();

    let cred_record = cred?;

    // Decrypt the stored credentials
    match kyomi_auth::encryption::decrypt(&cred_record.credentials, encryption_key) {
        Ok(ref plaintext) => serde_json::from_str(plaintext).ok(),
        Err(e) => {
            warn!(
                user_id,
                datasource_config_id = ctx.datasource_config_id,
                error = %e,
                "failed to decrypt stored credentials for catalog indexing"
            );
            None
        }
    }
}

// ─── SQL catalog indexing template method ───────────────────────────────────────

/// Template method for SQL-based catalog indexing.
///
/// Orchestrates the complete indexing flow for SQL-based datasources:
/// 1. Resolve credentials
/// 2. Create provider and test connection
/// 3. Discover containers (schemas/databases)
/// 4. For each container: enumerate tables -> fetch columns -> cache with embeddings
/// 5. Archive missing tables
/// 6. Update workspace/datasource status
///
/// Individual SQL indexers implement [`SQLCatalogIndexer`] with provider-specific
/// details; this function handles all the shared logic.
pub async fn index_catalog_sql(
    indexer: &dyn SQLCatalogIndexer,
    ctx: &IndexerContext,
    db: &DbPool,
    embedding: &EmbeddingService,
    user_email: Option<&str>,
    provided_credentials: Option<&Value>,
    max_tables_per_dataset: Option<usize>,
) -> CatalogIndexResult {
    let start_time = Utc::now();

    // Update status to running
    let _ = update_workspace_status(
        db,
        &ctx.workspace_id,
        &ctx.datasource_config_id,
        "running",
        None,
    )
    .await;

    // Resolve credentials
    let credentials = resolve_indexing_credentials(
        db,
        ctx,
        user_email,
        provided_credentials,
    )
    .await;

    let Some(credentials) = credentials else {
        let result = CatalogIndexResult::skipped("No credentials available for catalog indexing")
            .with_times(
                &start_time.to_rfc3339(),
                &Utc::now().to_rfc3339(),
            )
            .with_ids(&ctx.datasource_config_id, &ctx.workspace_id);

        let _ = update_workspace_status(
            db,
            &ctx.workspace_id,
            &ctx.datasource_config_id,
            "idle",
            None,
        )
        .await;

        return result;
    };

    // Create provider. `ctx.connection_config` came straight from the
    // database and may hold encrypted `COMMON_SENSITIVE` fields (e.g.
    // `ssh_private_key`) — every driver needs plaintext.
    let decrypted_config = kyomi_auth::credential_service::decrypt_connection_config_secrets(
        &ctx.connection_config,
        &ctx.encryption_key,
    );
    let provider = match indexer
        .create_provider(&decrypted_config, &credentials)
        .await
    {
        Ok(p) => p,
        Err(e) => {
            let result = CatalogIndexResult::error(&format!(
                "Failed to create provider: {e}"
            ))
            .with_times(&start_time.to_rfc3339(), &Utc::now().to_rfc3339())
            .with_ids(&ctx.datasource_config_id, &ctx.workspace_id);

            let _ = update_workspace_status(
                db,
                &ctx.workspace_id,
                &ctx.datasource_config_id,
                "failed",
                None,
            )
            .await;

            return result;
        }
    };

    // Test connection
    if let Err(e) = provider.test_connection().await {
        provider.close().await;
        let result = CatalogIndexResult::error(&format!(
            "Connection test failed: {e}"
        ))
        .with_times(&start_time.to_rfc3339(), &Utc::now().to_rfc3339())
        .with_ids(&ctx.datasource_config_id, &ctx.workspace_id);

        let _ = update_workspace_status(
            db,
            &ctx.workspace_id,
            &ctx.datasource_config_id,
            "failed",
            None,
        )
        .await;

        return result;
    }

    // Discover containers
    let containers = match get_catalog_containers(ctx, indexer, provider.as_ref()).await {
        Ok(c) => c,
        Err(e) => {
            provider.close().await;
            let result = CatalogIndexResult::error(&format!(
                "Failed to discover {}: {e}",
                indexer.container_label()
            ))
            .with_times(&start_time.to_rfc3339(), &Utc::now().to_rfc3339())
            .with_ids(&ctx.datasource_config_id, &ctx.workspace_id);

            let _ = update_workspace_status(
                db,
                &ctx.workspace_id,
                &ctx.datasource_config_id,
                "failed",
                None,
            )
            .await;

            return result;
        }
    };

    info!(
        workspace_id = ctx.workspace_id,
        datasource_config_id = ctx.datasource_config_id,
        containers = containers.len(),
        label = indexer.container_label(),
        "discovered {} {}(s)",
        containers.len(),
        indexer.container_label()
    );

    // Index each container
    let mut tables_indexed = 0usize;
    let mut errors = Vec::new();
    let mut seen_table_ids = std::collections::HashSet::new();
    let mut any_container_succeeded = false;
    let project_id = indexer.get_project_id(ctx);

    // Analytics datasources use _-prefixed hidden tables for transforms;
    // only the public views (without _) should be indexed.
    let is_analytics = ctx
        .connection_config
        .get("analytics_site_id")
        .and_then(|v| v.as_str())
        .is_some();

    for container in &containers {
        // Get tables in container
        let tables = match indexer
            .get_tables_in_container(provider.as_ref(), container, max_tables_per_dataset)
            .await
        {
            Ok(t) => {
                any_container_succeeded = true;
                t
            }
            Err(e) => {
                let msg = format!(
                    "Failed to list tables in {} '{}': {e}",
                    indexer.container_label(),
                    container
                );
                warn!("{msg}");
                errors.push(msg);
                continue;
            }
        };

        for table in &tables {
            // Skip hidden transform tables (e.g. _sessions, _visitors) for analytics datasources
            if is_analytics && table.name.starts_with('_') {
                continue;
            }
            // Use dataset_override if the indexer provides one (e.g., Snowflake
            // returns "database.schema", Databricks returns "catalog.schema").
            let effective_dataset = table
                .dataset_override
                .as_deref()
                .unwrap_or(container);

            // For analytics datasources, store bare table names (no database prefix)
            // since the connection is already scoped to the per-site database.
            // The real database name is still used for column queries below.
            let (cache_dataset, full_table_id) = if is_analytics {
                (String::new(), table.name.clone())
            } else {
                (
                    effective_dataset.to_string(),
                    indexer.build_full_table_id(effective_dataset, &table.name),
                )
            };
            // `archive_missing_tables` reconstructs each cached table's identity
            // from its stored (project_id, dataset_id, table_id) columns via
            // `build_full_table_name`. The seen-set key MUST be built the same
            // way. Datasources whose `project_id` is non-empty (Postgres and
            // Redshift set it to the database name) produce a 3-part archival
            // key, so a 2-part `dataset.table` seen key never matches — every
            // table would be archived the instant it is cached ("0 tables
            // found / N archived"). Mirrors the Connect-path indexer.
            let archive_id = kyomi_core::build_full_table_name(
                &project_id,
                &cache_dataset,
                &table.name,
            );
            seen_table_ids.insert(archive_id);

            // Get columns — use effective_dataset (real database name) so the
            // indexer can query system.columns correctly.
            let columns = match indexer
                .get_table_columns(provider.as_ref(), effective_dataset, &table.name)
                .await
            {
                Ok(c) => c,
                Err(e) => {
                    let msg = format!(
                        "Failed to get columns for {}: {e}",
                        full_table_id
                    );
                    warn!("{msg}");
                    errors.push(msg);
                    continue;
                }
            };

            let table_type = table
                .table_type
                .as_deref()
                .unwrap_or("TABLE");

            // Cache table with embeddings
            let cached = cache_table(CacheTableParams {
                db,
                embedding,
                ctx,
                project_id: &project_id,
                dataset_id: &cache_dataset,
                table_name: &table.name,
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

    // Archive missing tables — only when we have positive evidence of tables.
    // Guard condition: if no tables were found (regardless of whether any container
    // query succeeded), preserve the existing catalog. A successful query returning
    // 0 rows is just as unsafe to archive on as a failed query.
    // Exception: containers.is_empty() means the user explicitly configured no
    // containers — archiving is correct there (removes stale tables from a datasource
    // the user intentionally emptied).
    let nothing_found =
        seen_table_ids.is_empty() && tables_indexed == 0 && !containers.is_empty();

    let tables_archived = if nothing_found {
        warn!(
            workspace_id = ctx.workspace_id,
            datasource_config_id = ctx.datasource_config_id,
            any_container_succeeded,
            "No tables found — preserving existing catalog (archiving skipped)"
        );
        0
    } else {
        let archived_names = archive_missing_tables(
            db,
            &ctx.workspace_id,
            &ctx.datasource_config_id,
            &seen_table_ids,
        )
        .await
        .unwrap_or_default();
        archived_names.len()
    };

    // Update datasource last refresh time
    let _ = update_datasource_last_refresh(db, &ctx.datasource_config_id).await;

    // Update workspace status to idle
    let _ = update_workspace_status(
        db,
        &ctx.workspace_id,
        &ctx.datasource_config_id,
        "idle",
        None,
    )
    .await;

    // Close provider
    provider.close().await;

    let end_time = Utc::now();

    info!(
        workspace_id = ctx.workspace_id,
        datasource_config_id = ctx.datasource_config_id,
        tables_indexed,
        tables_archived,
        errors = errors.len(),
        elapsed_secs = (end_time - start_time).num_seconds(),
        "catalog indexing complete"
    );

    // If nothing was found, return an error result so callers can surface
    // the failure rather than silently reporting zero tables indexed.
    if nothing_found {
        let mut result =
            CatalogIndexResult::error("No tables discovered — existing catalog preserved")
                .with_times(&start_time.to_rfc3339(), &end_time.to_rfc3339())
                .with_ids(&ctx.datasource_config_id, &ctx.workspace_id);
        if !errors.is_empty() {
            result.errors = Some(errors);
        }
        return result;
    }

    let mut result = CatalogIndexResult::completed(tables_indexed, tables_archived)
        .with_times(&start_time.to_rfc3339(), &end_time.to_rfc3339())
        .with_ids(&ctx.datasource_config_id, &ctx.workspace_id);

    if !errors.is_empty() {
        result.errors = Some(errors);
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use kyomi_core::DbPool;
    use kyomi_datasource_server::{DatasourceProvider, QueryResult};
    use kyomi_embed::EmbeddingService;
    use serde_json::json;

    /// Minimal provider stub. The mock indexer overrides all catalog SQL, so
    /// only `test_connection`/`close` are ever exercised.
    struct MockProvider;

    #[async_trait]
    impl DatasourceProvider for MockProvider {
        async fn test_connection(&self) -> kyomi_connect_protocol::Result<bool> {
            Ok(true)
        }

        async fn execute_query(
            &self,
            _sql: &str,
            _limit: Option<u32>,
            _offset: Option<u32>,
            _include_total: bool,
            _job_id: Option<&str>,
        ) -> kyomi_connect_protocol::Result<QueryResult> {
            Ok(QueryResult::success_empty())
        }

        async fn close(&self) {}
    }

    /// Mimics the Redshift/Postgres indexers: `get_project_id` returns the
    /// database name (non-empty), which is what triggers the 3-part archival
    /// key vs 2-part seen-key mismatch.
    struct MockRedshiftIndexer;

    #[async_trait]
    impl SQLCatalogIndexer for MockRedshiftIndexer {
        fn container_label(&self) -> &str {
            "schema"
        }

        fn container_config_key(&self) -> &str {
            "catalog_schemas"
        }

        async fn create_provider(
            &self,
            _connection_config: &Value,
            _credentials: &Value,
        ) -> Result<Box<dyn DatasourceProvider>> {
            Ok(Box::new(MockProvider))
        }

        async fn discover_all_containers(
            &self,
            _provider: &dyn DatasourceProvider,
        ) -> Result<Vec<String>> {
            Ok(vec!["public".to_string()])
        }

        async fn get_tables_in_container(
            &self,
            _provider: &dyn DatasourceProvider,
            _container_name: &str,
            _max_tables: Option<usize>,
        ) -> Result<Vec<TableEntry>> {
            Ok(vec![
                TableEntry {
                    name: "users".to_string(),
                    table_type: Some("BASE TABLE".to_string()),
                    dataset_override: None,
                },
                TableEntry {
                    name: "events".to_string(),
                    table_type: Some("BASE TABLE".to_string()),
                    dataset_override: None,
                },
            ])
        }

        async fn get_table_columns(
            &self,
            _provider: &dyn DatasourceProvider,
            _container_name: &str,
            _table_name: &str,
        ) -> Result<Vec<ColumnEntry>> {
            Ok(vec![ColumnEntry {
                name: "id".to_string(),
                col_type: Some("number".to_string()),
                native_type: Some("INTEGER".to_string()),
                description: None,
            }])
        }

        fn get_project_id(&self, ctx: &IndexerContext) -> String {
            ctx.connection_config
                .get("database")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string()
        }
    }

    /// Regression for the Redshift/Postgres "0 tables found / N archived" bug.
    ///
    /// When a SQL indexer reports a non-empty `project_id` (Redshift and
    /// Postgres return the database name), the refresh loop must record each
    /// cached table in `seen_table_ids` under the SAME 3-part identity that
    /// `archive_missing_tables` reconstructs from the stored
    /// `(project_id, dataset_id, table_id)` columns. Before the fix the seen-key
    /// was the 2-part `schema.table`, so every freshly-cached table was archived
    /// the instant it was cached — the catalog came back empty.
    #[tokio::test]
    async fn redshift_refresh_does_not_archive_freshly_cached_tables() {
        let db = DbPool::connect("sqlite::memory:")
            .await
            .expect("connect in-memory sqlite");
        let DbPool::Sqlite(sq) = &db else {
            unreachable!("expected sqlite pool");
        };

        // Seed FK parents: users -> workspaces -> datasource_configs.
        sqlx::query("INSERT INTO users (user_id, email) VALUES ('u1', 'u1@test.local')")
            .execute(sq)
            .await
            .expect("insert user");
        sqlx::query(
            "INSERT INTO workspaces (workspace_id, name, owner_user_id) VALUES ('ws1', 'WS', 'u1')",
        )
        .execute(sq)
        .await
        .expect("insert workspace");
        sqlx::query(
            "INSERT INTO datasource_configs (id, workspace_id, name, datasource_type, slug) \
             VALUES ('ds1', 'ws1', 'RS', 'redshift', 'rs')",
        )
        .execute(sq)
        .await
        .expect("insert datasource_config");

        let embedding = EmbeddingService::new().expect("load embedding model");
        let ctx = IndexerContext {
            workspace_id: "ws1".to_string(),
            datasource_config_id: "ds1".to_string(),
            connection_config: json!({ "database": "analytics" }),
            encryption_key: std::sync::Arc::new([0u8; 32]),
        };
        let credentials = json!({ "user": "x", "password": "y" });

        let result = index_catalog_sql(
            &MockRedshiftIndexer,
            &ctx,
            &db,
            &embedding,
            None,
            Some(&credentials),
            None,
        )
        .await;

        assert_eq!(
            result.tables_archived, 0,
            "freshly-cached tables must not be archived (bug archived all of them)"
        );

        // The user-observable symptom: rows must remain un-archived so they
        // show up in the catalog.
        let visible: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM datasource_table_cache WHERE is_archived = 0")
                .fetch_one(sq)
                .await
                .expect("count visible tables");
        assert_eq!(
            visible, 2,
            "both tables should remain visible in the catalog after refresh"
        );
    }
}
