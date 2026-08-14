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
    archive_missing_tables, cache_table, resolve_final_status, update_datasource_last_refresh,
    update_datasource_status, CacheTableParams, IndexerContext,
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
    /// 5. Update this datasource's catalog refresh status
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

    /// Like [`get_tables_in_container`], but also returns any partial-failure
    /// messages the indexer accumulated while producing the table list.
    ///
    /// Default implementation: delegate to `get_tables_in_container` and
    /// report zero partial failures. This is correct for every indexer whose
    /// container listing is a single query (Postgres, MySQL, ClickHouse,
    /// Snowflake, Redshift, SQL Server, Synapse, FlareDb) — a failure there
    /// naturally becomes the `Err` case that `index_catalog_sql`'s caller
    /// already handles (recorded in `errors`, container skipped).
    ///
    /// Override this when `get_tables_in_container`'s implementation
    /// internally loops over sub-containers (Databricks: catalog -> schemas
    /// -> tables) and must tolerate an individual sub-container failing
    /// without aborting the whole container. Swallowing such a failure with
    /// only a log and no return path makes a fully permission-denied
    /// container indistinguishable from a genuinely empty one to the
    /// `nothing_found` check below — the exact silent-success bug KYO-126
    /// exists to fix, reappearing one level down (KYO-126, second pass).
    /// Returning the messages here lets [`index_catalog_sql`] fold them into
    /// the same `errors` accumulator every other indexer's `Err` path
    /// already feeds, so [`resolve_final_status`] can see them.
    ///
    /// [`get_tables_in_container`]: SQLCatalogIndexer::get_tables_in_container
    async fn get_tables_in_container_with_partial_failures(
        &self,
        provider: &dyn DatasourceProvider,
        container_name: &str,
        max_tables: Option<usize>,
    ) -> Result<(Vec<TableEntry>, Vec<String>)> {
        let tables = self
            .get_tables_in_container(provider, container_name, max_tables)
            .await?;
        Ok((tables, Vec::new()))
    }

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

// `can_refresh_now`, `archive_missing_tables`, `update_datasource_status`,
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
    .map_err(|e| {
        // `email` is intentionally omitted (PII) — this is an indexing path,
        // not an auth path. `datasource_config_id` is enough to correlate.
        warn!(
            datasource_config_id = ctx.datasource_config_id,
            error = %e,
            "failed to look up user for catalog indexing credential resolution"
        );
    })
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
    .map_err(|e| {
        warn!(
            user_id,
            datasource_config_id = ctx.datasource_config_id,
            error = %e,
            "failed to fetch stored credentials for catalog indexing"
        );
    })
    .ok()
    .flatten();

    let cred_record = cred?;

    // Decrypt the stored credentials
    match kyomi_auth::encryption::decrypt(&cred_record.credentials, encryption_key) {
        Ok(ref plaintext) => match serde_json::from_str::<Value>(plaintext) {
            Ok(value) => Some(value),
            Err(e) => {
                // Decryption succeeded (key is right, ciphertext intact) but the
                // plaintext isn't valid JSON — a corruption/schema-drift signal
                // that must not be confused with the decrypt-failure branch below.
                // NEVER log `plaintext` here — it is decrypted credential material.
                warn!(
                    user_id,
                    datasource_config_id = ctx.datasource_config_id,
                    error = %e,
                    "decrypted credentials failed to parse for catalog indexing"
                );
                None
            }
        },
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
    let _ = update_datasource_status(
        db,
        &ctx.workspace_id,
        &ctx.datasource_config_id,
        "running",
        None,
        None,
        &[],
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

        let _ = update_datasource_status(
            db,
            &ctx.workspace_id,
            &ctx.datasource_config_id,
            "idle",
            None,
            None,
            &[],
        )
        .await;

        return result;
    };

    // Create provider. `ctx.connection_config` came straight from the
    // database and may hold encrypted `COMMON_SENSITIVE` fields (e.g.
    // `ssh_private_key`) — every driver needs plaintext.
    let decrypted_config = match kyomi_auth::credential_service::decrypt_connection_config_secrets(
        &ctx.connection_config,
        &ctx.encryption_key,
    ) {
        Ok(config) => config,
        Err(e) => {
            let msg = format!("Failed to decrypt connection_config: {e}");
            let result = CatalogIndexResult::error(&msg)
                .with_times(&start_time.to_rfc3339(), &Utc::now().to_rfc3339())
                .with_ids(&ctx.datasource_config_id, &ctx.workspace_id);

            let _ = update_datasource_status(
                db,
                &ctx.workspace_id,
                &ctx.datasource_config_id,
                "failed",
                None,
                Some(&msg),
                &[],
            )
            .await;

            return result;
        }
    };
    let provider = match indexer
        .create_provider(&decrypted_config, &credentials)
        .await
    {
        Ok(p) => p,
        Err(e) => {
            let msg = format!("Failed to create provider: {e}");
            let result = CatalogIndexResult::error(&msg)
                .with_times(&start_time.to_rfc3339(), &Utc::now().to_rfc3339())
                .with_ids(&ctx.datasource_config_id, &ctx.workspace_id);

            let _ = update_datasource_status(
                db,
                &ctx.workspace_id,
                &ctx.datasource_config_id,
                "failed",
                None,
                Some(&msg),
                &[],
            )
            .await;

            return result;
        }
    };

    // Test connection
    if let Err(e) = provider.test_connection().await {
        provider.close().await;
        let msg = format!("Connection test failed: {e}");
        let result = CatalogIndexResult::error(&msg)
            .with_times(&start_time.to_rfc3339(), &Utc::now().to_rfc3339())
            .with_ids(&ctx.datasource_config_id, &ctx.workspace_id);

        let _ = update_datasource_status(
            db,
            &ctx.workspace_id,
            &ctx.datasource_config_id,
            "failed",
            None,
            Some(&msg),
            &[],
        )
        .await;

        return result;
    }

    // Discover containers
    let containers = match get_catalog_containers(ctx, indexer, provider.as_ref()).await {
        Ok(c) => c,
        Err(e) => {
            provider.close().await;
            let msg = format!("Failed to discover {}: {e}", indexer.container_label());
            let result = CatalogIndexResult::error(&msg)
                .with_times(&start_time.to_rfc3339(), &Utc::now().to_rfc3339())
                .with_ids(&ctx.datasource_config_id, &ctx.workspace_id);

            let _ = update_datasource_status(
                db,
                &ctx.workspace_id,
                &ctx.datasource_config_id,
                "failed",
                None,
                Some(&msg),
                &[],
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
        // Get tables in container. `_with_partial_failures` also surfaces
        // any sub-container errors the indexer tolerated internally (e.g.
        // Databricks: one permission-denied schema out of several in a
        // catalog) — see the trait method's doc comment. Every other
        // indexer's default implementation reports zero partial failures,
        // so this is a no-op for them.
        let (tables, partial_failures) = match indexer
            .get_tables_in_container_with_partial_failures(
                provider.as_ref(),
                container,
                max_tables_per_dataset,
            )
            .await
        {
            Ok((t, partial_failures)) => {
                any_container_succeeded = true;
                (t, partial_failures)
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

        // Already logged by the indexer at the point of failure (it has
        // richer context — e.g. which sub-schema) — fold straight into the
        // shared accumulator without a second log line.
        errors.extend(partial_failures);

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
            match cache_table(CacheTableParams {
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
            .await
            {
                Ok(()) => tables_indexed += 1,
                Err(e) => {
                    let msg = format!("Failed to cache table {full_table_id}: {e}");
                    warn!("{msg}");
                    errors.push(msg);
                }
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

    // Record this datasource's status. A container set that yields zero tables is
    // only a failure if at least one discovery error occurred along the
    // way (KYO-126) — a container that is accessible but genuinely empty
    // must still report `idle`. See `resolve_final_status`.
    let (final_status, failure_reason) = resolve_final_status(nothing_found, &errors);
    let _ = update_datasource_status(
        db,
        &ctx.workspace_id,
        &ctx.datasource_config_id,
        final_status,
        None,
        failure_reason.as_deref(),
        &errors,
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
    use std::sync::{Arc, Mutex};

    // ─── Log capture for KYO-217 credential-resolution warnings ────────────
    //
    // There is no `tracing-test` in this workspace and we were told not to add
    // a new dependency for this. `tracing-subscriber` is already a workspace
    // dependency (used by kyomi-core, apps/server, apps/desktop) — this wires
    // it in as a dev-dependency for kyomi-agent and uses its `fmt` layer with a
    // custom `MakeWriter` to capture formatted log lines into memory so tests
    // can assert on them directly, instead of only asserting on return values.

    #[derive(Clone, Default)]
    struct CapturingWriter(Arc<Mutex<Vec<u8>>>);

    impl CapturingWriter {
        fn contents(&self) -> String {
            String::from_utf8_lossy(&self.0.lock().expect("lock poisoned")).into_owned()
        }
    }

    impl std::io::Write for CapturingWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().expect("lock poisoned").extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for CapturingWriter {
        type Writer = CapturingWriter;

        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

    /// Installs a capturing `tracing` subscriber as the thread-local default
    /// for the duration of the returned guard. `#[tokio::test]` uses a
    /// current-thread runtime by default, so the thread-local dispatcher
    /// stays active across `.await` points in the test body.
    fn capture_logs() -> (CapturingWriter, tracing::subscriber::DefaultGuard) {
        let writer = CapturingWriter::default();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(writer.clone())
            .with_ansi(false)
            .without_time()
            .with_target(false)
            // Scope capture to WARN and above. `DbPool::connect` itself emits
            // an unrelated INFO "SQLite pool connected" line — without this
            // filter the "no warning at all" test below sees that line and
            // false-fails even though `resolve_indexing_credentials` logged
            // nothing.
            .with_max_level(tracing::Level::WARN)
            .finish();
        let guard = tracing::subscriber::set_default(subscriber);
        (writer, guard)
    }

    /// Seeds the FK chain `resolve_indexing_credentials` walks: a user, a
    /// workspace they own, and a datasource config to attach credentials to.
    async fn seed_credential_resolution_rows(sq: &sqlx::SqlitePool, user_id: &str, email: &str) {
        sqlx::query("INSERT INTO users (user_id, email) VALUES (?, ?)")
            .bind(user_id)
            .bind(email)
            .execute(sq)
            .await
            .expect("insert user");
        sqlx::query(
            "INSERT INTO workspaces (workspace_id, name, owner_user_id) \
             VALUES ('ws-cred', 'WS', ?)",
        )
        .bind(user_id)
        .execute(sq)
        .await
        .expect("insert workspace");
        sqlx::query(
            "INSERT INTO datasource_configs (id, workspace_id, name, datasource_type, slug) \
             VALUES ('ds-cred', 'ws-cred', 'DS', 'postgres', 'ds-cred')",
        )
        .execute(sq)
        .await
        .expect("insert datasource_config");
    }

    fn cred_resolution_ctx(key: [u8; 32]) -> IndexerContext {
        IndexerContext {
            workspace_id: "ws-cred".to_string(),
            datasource_config_id: "ds-cred".to_string(),
            connection_config: json!({}),
            encryption_key: Arc::new(key),
        }
    }

    /// KYO-217 regression: the `users` lookup query itself can fail (pool
    /// exhaustion, connection blip, transient Postgres error) — distinct from
    /// "email not found," which is a normal `Ok(None)`. Proven by dropping
    /// the `users` table so `SELECT user_id FROM users WHERE email = $1`
    /// itself errors, rather than merely returning zero rows: an empty
    /// result would silently take the same "no credentials" path and prove
    /// nothing about this failure mode.
    #[tokio::test]
    async fn users_lookup_query_failure_returns_none_and_warns_without_leaking_email() {
        let (writer, _guard) = capture_logs();

        let db = DbPool::connect("sqlite::memory:")
            .await
            .expect("connect in-memory sqlite");
        let DbPool::Sqlite(sq) = &db else {
            unreachable!("expected sqlite pool");
        };

        // No rows seeded — the `users` lookup is the very first DB step in
        // `resolve_indexing_credentials`, so nothing else needs to exist for
        // this failure mode. Dropping the table with no FK-dependent rows
        // present (unlike the seeded fixture, which would trip SQLite's
        // "FOREIGN KEY constraint failed" on a referenced parent table)
        // gives a clean, genuine query error.
        sqlx::query("DROP TABLE users")
            .execute(sq)
            .await
            .expect("drop users table");

        let ctx = cred_resolution_ctx([2u8; 32]);
        let result =
            resolve_indexing_credentials(&db, &ctx, Some("u-lookup-fail@test.local"), None).await;

        assert!(
            result.is_none(),
            "a failed user lookup must not resolve credentials"
        );

        let logs = writer.contents();
        assert!(
            logs.contains("failed to look up user for catalog indexing credential resolution"),
            "expected the users-lookup warning, got logs: {logs}"
        );
        assert!(
            !logs.contains("u-lookup-fail@test.local"),
            "the user's email is PII and must never appear in this indexing-path log, \
             got logs: {logs}"
        );
    }

    /// KYO-217 regression: `get_user_credential`'s query can fail
    /// independently of the (successful) users lookup that precedes it —
    /// same operational shape as the case above, one query later. Proven by
    /// dropping `user_datasource_credentials` so its SELECT itself errors.
    #[tokio::test]
    async fn get_user_credential_query_failure_returns_none_and_warns() {
        let (writer, _guard) = capture_logs();

        let db = DbPool::connect("sqlite::memory:")
            .await
            .expect("connect in-memory sqlite");
        let DbPool::Sqlite(sq) = &db else {
            unreachable!("expected sqlite pool");
        };
        seed_credential_resolution_rows(sq, "u-cred-fail", "u-cred-fail@test.local").await;

        // Force a genuine query error, not an empty result. The user row
        // resolved fine above; only the credential-fetch query fails.
        sqlx::query("DROP TABLE user_datasource_credentials")
            .execute(sq)
            .await
            .expect("drop user_datasource_credentials table");

        let ctx = cred_resolution_ctx([3u8; 32]);
        let result =
            resolve_indexing_credentials(&db, &ctx, Some("u-cred-fail@test.local"), None).await;

        assert!(
            result.is_none(),
            "a failed credential fetch must not resolve credentials"
        );

        let logs = writer.contents();
        assert!(
            logs.contains("failed to fetch stored credentials for catalog indexing"),
            "expected the credential-fetch warning, got logs: {logs}"
        );
    }

    /// KYO-217 case 1: decryption succeeds but the plaintext isn't valid JSON.
    ///
    /// This is the sharpest of the three failure modes — the key is right and
    /// the ciphertext is intact, so a silent `None` here hides schema drift or
    /// corruption. Must return `None` AND emit a warning distinguishable from
    /// the decrypt-failure warning below it.
    #[tokio::test]
    async fn parse_failure_returns_none_and_warns_distinctly_from_decrypt_failure() {
        let (writer, _guard) = capture_logs();

        let db = DbPool::connect("sqlite::memory:")
            .await
            .expect("connect in-memory sqlite");
        let DbPool::Sqlite(sq) = &db else {
            unreachable!("expected sqlite pool");
        };
        seed_credential_resolution_rows(sq, "u-parse", "u-parse@test.local").await;

        let key = [7u8; 32];
        let encrypted =
            kyomi_auth::encryption::encrypt("this is not json {{{", &key).expect("encrypt");
        sqlx::query(
            "INSERT INTO user_datasource_credentials \
             (user_id, datasource_config_id, workspace_id, credentials) \
             VALUES (?, 'ds-cred', 'ws-cred', ?)",
        )
        .bind("u-parse")
        .bind(&encrypted)
        .execute(sq)
        .await
        .expect("insert credential");

        let ctx = cred_resolution_ctx(key);
        let result =
            resolve_indexing_credentials(&db, &ctx, Some("u-parse@test.local"), None).await;

        assert!(
            result.is_none(),
            "decrypted-but-unparseable credentials must not resolve"
        );

        let logs = writer.contents();
        assert!(
            logs.contains("decrypted credentials failed to parse for catalog indexing"),
            "expected the parse-failure warning, got logs: {logs}"
        );
        assert!(
            !logs.contains("failed to decrypt stored credentials for catalog indexing"),
            "parse failure must not also emit the decrypt-failure message — \
             the two must stay distinguishable, got logs: {logs}"
        );
        assert!(
            logs.contains("ds-cred"),
            "warning must identify which datasource_config_id (and thus provider) \
             failed, got logs: {logs}"
        );
    }

    /// KYO-217 case 2 (regression guard): decryption itself fails. The
    /// existing decrypt-failure warning must still fire, and must stay
    /// distinct from the new parse-failure message added alongside it —
    /// this is what stops the two from merging into one indistinguishable
    /// warning in a future edit.
    #[tokio::test]
    async fn decrypt_failure_returns_none_and_keeps_existing_warning() {
        let (writer, _guard) = capture_logs();

        let db = DbPool::connect("sqlite::memory:")
            .await
            .expect("connect in-memory sqlite");
        let DbPool::Sqlite(sq) = &db else {
            unreachable!("expected sqlite pool");
        };
        seed_credential_resolution_rows(sq, "u-decrypt", "u-decrypt@test.local").await;

        // Too short to be a valid AES-256-GCM payload (version + nonce + tag) —
        // `kyomi_auth::encryption::decrypt` rejects it before ever touching JSON.
        sqlx::query(
            "INSERT INTO user_datasource_credentials \
             (user_id, datasource_config_id, workspace_id, credentials) \
             VALUES ('u-decrypt', 'ds-cred', 'ws-cred', 'short')",
        )
        .execute(sq)
        .await
        .expect("insert credential");

        let ctx = cred_resolution_ctx([9u8; 32]);
        let result =
            resolve_indexing_credentials(&db, &ctx, Some("u-decrypt@test.local"), None).await;

        assert!(result.is_none(), "undecryptable credentials must not resolve");

        let logs = writer.contents();
        assert!(
            logs.contains("failed to decrypt stored credentials for catalog indexing"),
            "expected the existing decrypt-failure warning, got logs: {logs}"
        );
        assert!(
            !logs.contains("decrypted credentials failed to parse for catalog indexing"),
            "decrypt failure must not also emit the parse-failure message, got logs: {logs}"
        );
    }

    /// KYO-217 case 3 (most important): a user with genuinely no stored
    /// credential row is a normal, expected condition — not a failure. It
    /// must return `None` with NO warning at all, or this fix turns routine
    /// "user hasn't connected this datasource" cases into permanent log noise.
    #[tokio::test]
    async fn no_credential_row_returns_none_without_any_warning() {
        let (writer, _guard) = capture_logs();

        let db = DbPool::connect("sqlite::memory:")
            .await
            .expect("connect in-memory sqlite");
        let DbPool::Sqlite(sq) = &db else {
            unreachable!("expected sqlite pool");
        };
        seed_credential_resolution_rows(sq, "u-none", "u-none@test.local").await;
        // Deliberately no INSERT into user_datasource_credentials.

        let ctx = cred_resolution_ctx([1u8; 32]);
        let result = resolve_indexing_credentials(&db, &ctx, Some("u-none@test.local"), None).await;

        assert!(result.is_none(), "no stored credentials should resolve to None");

        let logs = writer.contents();
        assert!(
            logs.trim().is_empty(),
            "no credential row is expected/routine and must not log anything, got logs: {logs}"
        );
    }

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

    // ── get_tables_in_container_with_partial_failures wiring (KYO-126, second pass) ──
    //
    // Databricks-shaped mock: overrides `get_tables_in_container_with_partial_failures`
    // directly (bypassing SQL entirely) to prove that whatever an indexer
    // returns through this channel actually reaches `errors` and, from there,
    // `resolve_final_status` — independent of Databricks' own SQL plumbing,
    // which is unit-tested separately in `indexers::databricks`.
    struct MockPartialFailureIndexer {
        tables: Vec<TableEntry>,
        partial_failures: Vec<String>,
    }

    #[async_trait]
    impl SQLCatalogIndexer for MockPartialFailureIndexer {
        fn container_label(&self) -> &str {
            "catalog"
        }

        fn container_config_key(&self) -> &str {
            "catalog_catalogs"
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
            Ok(vec!["main".to_string()])
        }

        async fn get_tables_in_container(
            &self,
            _provider: &dyn DatasourceProvider,
            _container_name: &str,
            _max_tables: Option<usize>,
        ) -> Result<Vec<TableEntry>> {
            Ok(self.tables.clone())
        }

        async fn get_tables_in_container_with_partial_failures(
            &self,
            _provider: &dyn DatasourceProvider,
            _container_name: &str,
            _max_tables: Option<usize>,
        ) -> Result<(Vec<TableEntry>, Vec<String>)> {
            Ok((self.tables.clone(), self.partial_failures.clone()))
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
    }

    /// Seeds the same FK chain as `redshift_refresh_does_not_archive_freshly_cached_tables`,
    /// parameterized so the two new tests below don't collide on row IDs.
    async fn seed_partial_failure_fixture(sq: &sqlx::SqlitePool, suffix: &str) -> IndexerContext {
        let user_id = format!("u-pf-{suffix}");
        let workspace_id = format!("ws-pf-{suffix}");
        let datasource_config_id = format!("ds-pf-{suffix}");

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
             VALUES (?, ?, 'DB', 'databricks', ?)",
        )
        .bind(&datasource_config_id)
        .bind(&workspace_id)
        .bind(format!("db-{suffix}"))
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

    /// KYO-126, second pass: a Databricks-shaped catalog where every schema
    /// is permission-denied must end up `"failed"` on the datasource's
    /// status column with a real reason, not `"idle"`. Before this fix, the
    /// per-schema failures never reached `errors` at all — the container
    /// call returned `Ok(vec![])`, `resolve_final_status(true, &[])` always
    /// resolves to `("idle", None)` regardless of how the emptiness arose.
    #[tokio::test]
    async fn databricks_shaped_all_schemas_failing_reports_failed_not_idle() {
        let db = DbPool::connect("sqlite::memory:")
            .await
            .expect("connect in-memory sqlite");
        let DbPool::Sqlite(sq) = &db else {
            unreachable!("expected sqlite pool");
        };
        let ctx = seed_partial_failure_fixture(sq, "allfail").await;
        let embedding = EmbeddingService::new().expect("load embedding model");
        let credentials = json!({ "token": "x" });

        let indexer = MockPartialFailureIndexer {
            tables: Vec::new(),
            partial_failures: vec![
                "Failed to list tables in schema 'main.sales': permission denied".to_string(),
                "Failed to list tables in schema 'main.marketing': permission denied".to_string(),
            ],
        };

        let result = index_catalog_sql(
            &indexer,
            &ctx,
            &db,
            &embedding,
            None,
            Some(&credentials),
            None,
        )
        .await;

        assert_eq!(
            result.errors.as_ref().map(Vec::len),
            Some(2),
            "both per-schema partial failures must reach the errors accumulator"
        );

        // The datasource-scoped status column is what the UI actually reads
        // (`CatalogIndexResult::status` is always "error" whenever nothing
        // is found, successful-empty or genuinely-failed alike — see
        // `resolve_final_status`'s doc comment for why that distinction
        // lives in the status column instead).
        let status: String = sqlx::query_scalar(
            "SELECT catalog_refresh_status FROM datasource_configs WHERE id = ?",
        )
        .bind(&ctx.datasource_config_id)
        .fetch_one(sq)
        .await
        .expect("read datasource status");
        assert_eq!(
            status, "failed",
            "a catalog where every schema is permission-denied must surface as failed"
        );
    }

    /// Companion regression guard: a Databricks-shaped catalog where only
    /// SOME schemas fail (others yield real tables) must still complete
    /// normally — one bad schema must not turn the whole catalog red. This
    /// is the "one bad apple" behavior the KYO-126 fix is required to
    /// preserve, not just the failure case above.
    #[tokio::test]
    async fn databricks_shaped_partial_schema_failure_still_completes() {
        let db = DbPool::connect("sqlite::memory:")
            .await
            .expect("connect in-memory sqlite");
        let DbPool::Sqlite(sq) = &db else {
            unreachable!("expected sqlite pool");
        };
        let ctx = seed_partial_failure_fixture(sq, "partial").await;
        let embedding = EmbeddingService::new().expect("load embedding model");
        let credentials = json!({ "token": "x" });

        let indexer = MockPartialFailureIndexer {
            tables: vec![TableEntry {
                name: "orders".to_string(),
                table_type: Some("TABLE".to_string()),
                dataset_override: Some("main.sales".to_string()),
            }],
            partial_failures: vec![
                "Failed to list tables in schema 'main.marketing': permission denied".to_string(),
            ],
        };

        let result = index_catalog_sql(
            &indexer,
            &ctx,
            &db,
            &embedding,
            None,
            Some(&credentials),
            None,
        )
        .await;

        // Two errors, not one: the schema-list partial failure PLUS a
        // `cache_table` write failure for `orders` (KYO-364) — pgvector
        // storage is unsupported on the in-memory SQLite pool this test runs
        // against (see the comment below), so `cache_table` always returns
        // `Err` here. Before KYO-364, `cache_table` returned a bare `false`
        // on any failure, and this loop's `if cached { tables_indexed += 1 }`
        // simply didn't increment — the failure never reached `errors` at
        // all. It now does.
        assert_eq!(
            result.errors.as_deref().map(<[String]>::len),
            Some(2),
            "the schema-list partial failure and the orders cache_table write failure must both surface"
        );
        let errors = result.errors.as_deref().expect("errors present");
        assert!(
            errors.iter().any(|e| e.contains("main.marketing")),
            "schema-list partial failure must be present, got: {errors:?}"
        );
        assert!(
            errors.iter().any(|e| e.contains("orders")),
            "cache_table write failure for orders must be present, got: {errors:?}"
        );

        // Not `result.tables_indexed` — `cache_table` only counts a table as
        // "indexed" if embedding storage also succeeds, and pgvector storage
        // is unsupported on the in-memory SQLite pool this test runs
        // against (see `redshift_refresh_does_not_archive_freshly_cached_tables`
        // above, which has the same property and asserts on the cache row
        // directly for the same reason). What actually decides
        // `nothing_found` — and therefore this test's point, that one bad
        // schema doesn't fail the whole catalog — is `seen_table_ids`, which
        // is populated as soon as a table is discovered, independent of
        // whether its embeddings could be stored.
        //
        // KYO-385: that last property is also a live bug, not just a
        // fixture quirk. Because `seen_table_ids` is populated before
        // `cache_table` runs, a run where every discovered table's write
        // fails still resolves to `"idle"` here — the `status` assertion at
        // the bottom of this test is asserting exactly that, on a fixture
        // where 0 of 1 tables were successfully cached. KYO-364 fixed this
        // on the BigQuery REST path (`resolve_run_outcome` in
        // `kyomi_auth::catalog::indexers::user_dataset`, which keys status
        // off `tables_indexed` and archiving off `seen_table_ids`
        // separately); porting that split here is KYO-385, which will
        // rename and re-assert this test.
        let cached_rows: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM datasource_table_cache WHERE datasource_config_id = ? AND is_archived = 0",
        )
        .bind(&ctx.datasource_config_id)
        .fetch_one(sq)
        .await
        .expect("count cached tables");
        assert_eq!(
            cached_rows, 1,
            "the accessible schema's table must still be cached despite the other schema's failure"
        );

        let status: String = sqlx::query_scalar(
            "SELECT catalog_refresh_status FROM datasource_configs WHERE id = ?",
        )
        .bind(&ctx.datasource_config_id)
        .fetch_one(sq)
        .await
        .expect("read datasource status");
        assert_eq!(
            status, "idle",
            "partial success (one bad schema, others fine) must not be reported as failed"
        );
    }
}
