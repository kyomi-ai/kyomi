// SPDX-License-Identifier: AGPL-3.0-or-later

//! Catalog indexing service -- routes indexing requests to the correct provider indexer.
//!
//! Mirrors Python's `CatalogIndexingService` from `datasources/catalog_service.py`.
//!
//! The service loads datasource configuration from the database, determines the
//! correct indexer implementation, and dispatches the indexing call. Provider-
//! specific indexer implementations are registered in [`get_indexer`].

use kyomi_core::datasource_registry::DatasourceType;
use kyomi_core::DbPool;
use kyomi_datasource_server::ConnectRegistry;
use kyomi_embed::EmbeddingService;
use serde_json::Value;
use std::str::FromStr;
use std::sync::Arc;
use tracing::{info, warn};

/// Row type for loading a datasource config.
#[derive(sqlx::FromRow)]
struct DsConfigRow {
    datasource_type: String,
    connection_config: Value,
    connection_type: String,
    name: String,
}

/// Row type for resolving a single datasource config ID.
#[derive(sqlx::FromRow)]
struct DsConfigIdRow {
    id: String,
}

use super::indexers;
use super::traits::CatalogIndexer;
use kyomi_auth::catalog::helpers::{can_refresh_now, IndexerContext};
use kyomi_auth::catalog::types::CatalogIndexResult;

// ─── Indexer factory ───────────────────────────────────────────────────────────

/// Create the appropriate catalog indexer for a datasource type.
///
/// Returns `Some` with a concrete indexer for supported datasource types.
pub fn get_indexer(ds_type: &DatasourceType) -> Option<Box<dyn CatalogIndexer>> {
    match ds_type {
        DatasourceType::Postgres => Some(Box::new(indexers::PostgresIndexer)),
        DatasourceType::MySQL => Some(Box::new(indexers::MySqlIndexer)),
        DatasourceType::ClickHouse => Some(Box::new(indexers::ClickHouseIndexer)),
        DatasourceType::Snowflake => Some(Box::new(indexers::SnowflakeIndexer)),
        DatasourceType::Databricks => Some(Box::new(indexers::DatabricksIndexer)),
        DatasourceType::Redshift => Some(Box::new(indexers::RedshiftIndexer)),
        DatasourceType::SqlServer => Some(Box::new(indexers::SqlServerIndexer)),
        DatasourceType::Synapse => Some(Box::new(indexers::SynapseIndexer)),
        DatasourceType::BigQuery => Some(Box::new(indexers::BigQueryIndexer)),
        DatasourceType::FlareDb => Some(Box::new(indexers::FlareDbIndexer)),
    }
}

// ─── CatalogIndexingService ────────────────────────────────────────────────────

/// Arguments for [`CatalogIndexingService::index_datasource`].
///
/// Packaged into a struct to keep the call signature under clippy's
/// `too_many_arguments` threshold while keeping every field explicit at
/// the call site.
pub struct IndexDatasourceParams<'a> {
    pub db: &'a DbPool,
    pub encryption_key: std::sync::Arc<[u8; 32]>,
    pub embedding: &'a EmbeddingService,
    pub workspace_id: &'a str,
    pub datasource_config_id: &'a str,
    pub user_email: Option<&'a str>,
    pub credentials: Option<&'a Value>,
    pub max_tables_per_dataset: Option<usize>,
    /// When `true`, bypass the concurrent-run guard (the check that skips
    /// if an indexing run started within the last hour). Set by the
    /// manual refresh path so an explicit user click always proceeds even
    /// if a background run is in flight. Scheduler and post-create spawn
    /// leave this `false` so they defer to an in-flight run.
    pub force: bool,
    /// Connect registry for datasources that tunnel through a Connect binary.
    /// `None` is fine for callers that don't have a Connect registry (e.g.
    /// the scheduler) — Connect datasources will be skipped with an error.
    pub connect_registry: Option<&'a ConnectRegistry>,
}

/// How long a just-started indexing run blocks subsequent runs before
/// the stamp ages out. Chosen to cover all realistic indexing durations
/// plus slack — a ClickHouse sample schema indexes in 1s, a 50-table
/// Postgres in ~60s, so 60 minutes is 60× headroom. Longer than any
/// single run should take, shorter than the 24h `can_refresh_now` rate
/// limit so the two gates stay orthogonal. Panicked runs self-heal
/// after the stamp ages out.
const CONCURRENT_RUN_GUARD_MINUTES: i64 = 60;

/// Provider-agnostic catalog indexing service.
///
/// Routes indexing requests to the correct provider-specific indexer based on
/// the datasource type. All datasource configuration is loaded from the database.
pub struct CatalogIndexingService;

impl CatalogIndexingService {
    /// Index a specific datasource by its config ID.
    ///
    /// 1. Loads datasource config from the database
    /// 2. Resolves the appropriate indexer for the datasource type
    /// 3. Dispatches the indexing call
    pub async fn index_datasource(params: IndexDatasourceParams<'_>) -> CatalogIndexResult {
        let IndexDatasourceParams {
            db,
            encryption_key,
            embedding,
            workspace_id,
            datasource_config_id,
            user_email,
            credentials,
            max_tables_per_dataset,
            force,
            connect_registry,
        } = params;
        // Load datasource config
        let ds_config = kyomi_core::db_fetch_optional!(
            db, DsConfigRow,
            "SELECT datasource_type, connection_config, \
                    COALESCE(connection_type, 'direct') AS connection_type, name \
             FROM datasource_configs \
             WHERE id = $1 AND workspace_id = $2",
            &datasource_config_id,
            &workspace_id
        );

        let ds_config = match ds_config {
            Ok(Some(row)) => row,
            Ok(None) => {
                return CatalogIndexResult::error(&format!(
                    "Datasource config {datasource_config_id} not found in workspace {workspace_id}"
                ));
            }
            Err(e) => {
                return CatalogIndexResult::error(&format!(
                    "Failed to load datasource config: {e}"
                ));
            }
        };

        let ds_type_str = ds_config.datasource_type;
        let connection_config = ds_config.connection_config;
        let connection_type = ds_config.connection_type;
        let ds_name = ds_config.name;

        // Connect datasources use their own indexer that talks through the
        // Connect binary's WebSocket tunnel. They don't go through the
        // DatasourceType / SQLCatalogIndexer dispatch.
        let is_connect = connection_type == "connect";

        // Parse datasource type (still needed for logging and non-connect dispatch)
        let ds_type = match DatasourceType::from_str(&ds_type_str) {
            Ok(t) => t,
            Err(_) => {
                return CatalogIndexResult::error(&format!(
                    "Unsupported datasource type: '{ds_type_str}'"
                ));
            }
        };

        // Build the indexer: Connect datasources get ConnectIndexer, others
        // get the standard per-type indexer.
        let indexer: Box<dyn CatalogIndexer> = if is_connect {
            match connect_registry {
                Some(registry) => {
                    Box::new(indexers::ConnectIndexer::new(registry.clone()))
                }
                None => {
                    return CatalogIndexResult::skipped(
                        "Connect datasource skipped — no Connect registry available \
                         (Connect binary may not be running)",
                    );
                }
            }
        } else {
            match get_indexer(&ds_type) {
                Some(i) => i,
                None => {
                    return CatalogIndexResult::skipped(&format!(
                        "No catalog indexer available for datasource type '{ds_type_str}'"
                    ));
                }
            }
        };

        // Concurrent-run guard: skip if an indexing run started within the
        // last hour. This is the serialization point that prevents the
        // scheduler and spawn_post_create from doubling up. `force: true`
        // (set by manual refresh) bypasses the guard so an explicit user
        // click always proceeds.
        //
        // The stamp is written below BEFORE dispatching to the indexer, so
        // the first caller wins and any later caller (scheduler tick firing
        // 1 second after a fresh datasource create) observes the recent
        // stamp and skips cleanly.
        //
        // Self-healing: if the winning run panics and never updates
        // `last_catalog_refresh`, the start stamp ages out after 60 minutes
        // and the next scheduler tick picks the datasource back up. No
        // "running" status to get stuck in.
        if !force
            && kyomi_auth::catalog::helpers::index_started_within(
                db,
                datasource_config_id,
                CONCURRENT_RUN_GUARD_MINUTES,
            )
            .await
        {
            info!(
                workspace_id,
                datasource_config_id,
                datasource_name = ds_name,
                "skipping catalog indexing — another run started recently"
            );
            return CatalogIndexResult::skipped(
                "another indexing run started within the concurrent-run guard window",
            );
        }

        // Stamp the start so any concurrent caller skips. Failure to stamp
        // is logged but doesn't abort — better to proceed with the index
        // than to fail the whole run because the guard column couldn't be
        // updated.
        if let Err(e) =
            kyomi_auth::catalog::helpers::stamp_last_index_started_at(db, datasource_config_id)
                .await
        {
            warn!(
                workspace_id,
                datasource_config_id,
                error = %e,
                "failed to stamp last_index_started_at — concurrent-run guard may not engage for the next caller"
            );
        }

        let ctx = IndexerContext {
            workspace_id: workspace_id.to_string(),
            datasource_config_id: datasource_config_id.to_string(),
            connection_config,
            encryption_key,
        };

        info!(
            workspace_id,
            datasource_config_id,
            datasource_name = ds_name,
            datasource_type = ds_type_str,
            force,
            "starting catalog indexing"
        );

        let result = indexer
            .index_catalog(&ctx, db, embedding, user_email, credentials, max_tables_per_dataset)
            .await;

        // Populate embeddings after successful indexing so the AI agent can
        // search catalog data. This runs for all callers (scheduler, manual
        // refresh, post-create) — a single code path.
        if result.status == "completed" && result.tables_indexed > 0 {
            populate_embeddings_after_indexing(db, embedding, workspace_id, datasource_config_id)
                .await;
        }

        result
    }

    /// Check if a datasource can be refreshed (respects 24hr rate limit).
    pub async fn can_refresh_datasource(
        db: &DbPool,
        datasource_config_id: &str,
        hours_threshold: i64,
    ) -> bool {
        can_refresh_now(db, datasource_config_id, hours_threshold).await
    }

    /// Spawn background catalog indexing for a newly-created datasource.
    ///
    /// This is the provider-agnostic entry point called by the datasource
    /// creation route right after a new datasource is persisted. It:
    ///
    /// 1. Looks up the workspace owner's email so the credential resolver
    ///    can fall back to the owner's stored per-user credentials if neither
    ///    `indexing_credentials` nor shared credentials are configured.
    /// 2. Waits for the embedding model to finish loading.
    /// 3. Dispatches to [`index_datasource`], which routes through the
    ///    polymorphic [`CatalogIndexer`] trait — no per-provider branching.
    ///
    /// The rate limit check (`can_refresh_now`) is intentionally skipped:
    /// this is the *initial* index for a brand-new datasource, so there is
    /// nothing to rate-limit. Any failure (missing credentials, unreachable
    /// database, permission errors) is logged — the datasource row is left
    /// in place so the user can fix credentials and retry via the manual
    /// refresh button.
    ///
    /// Credential resolution order (handled by `resolve_indexing_credentials`
    /// in `catalog/traits.rs`):
    /// 1. `indexing_credentials` from connection_config (dedicated indexer creds)
    /// 2. Shared workspace credentials
    /// 3. Workspace owner's stored per-user credentials
    ///
    /// [`index_datasource`]: Self::index_datasource
    /// [`CatalogIndexer`]: crate::catalog::traits::CatalogIndexer
    pub fn spawn_post_create(
        db: kyomi_core::DbPool,
        encryption_key: Arc<[u8; 32]>,
        embedding: kyomi_embed::LazyEmbedding,
        workspace_id: String,
        datasource_id: String,
        connect_registry: Option<ConnectRegistry>,
    ) {
        tokio::spawn(async move {
            // Resolve workspace owner email so the third-tier credential
            // fallback (owner's stored creds) can run if shared/dedicated
            // creds aren't configured.
            let owner_email =
                kyomi_auth::catalog::helpers::get_workspace_owner_email(&db, &workspace_id).await;

            let embed = match embedding.wait_ready().await {
                Ok(e) => e,
                Err(e) => {
                    warn!(
                        datasource_id = %datasource_id,
                        workspace_id = %workspace_id,
                        error = %e,
                        "Embedding model not ready, skipping initial catalog indexing"
                    );
                    return;
                }
            };

            let result = Self::index_datasource(IndexDatasourceParams {
                db: &db,
                encryption_key,
                embedding: embed,
                workspace_id: &workspace_id,
                datasource_config_id: &datasource_id,
                user_email: owner_email.as_deref(),
                credentials: None,
                max_tables_per_dataset: None,
                force: false,
                connect_registry: connect_registry.as_ref(),
            })
            .await;

            info!(
                datasource_id = %datasource_id,
                workspace_id = %workspace_id,
                status = ?result.status,
                tables = result.tables_indexed,
                "Initial catalog indexing completed for new datasource"
            );
        });
    }

    /// Spawn background post-creation tasks for a new analytics site.
    ///
    /// Performs two operations in a background `tokio::spawn`:
    /// 1. Syncs analytics quota to Redis so the collector can enforce limits.
    /// 2. Runs catalog indexing so the AI agent can discover tables/columns.
    ///
    /// Used by both the REST handler and the MCP tool after creating an
    /// analytics site, ensuring consistent behavior regardless of entry point.
    pub fn spawn_analytics_post_create(
        db: kyomi_core::DbPool,
        redis: Option<kyomi_core::RedisPool>,
        encryption_key: Arc<[u8; 32]>,
        embedding: kyomi_embed::LazyEmbedding,
        workspace_id: String,
        datasource_id: String,
        subscription_tier: String,
    ) {
        tokio::spawn(async move {
            // 1. Sync analytics quota to Redis (requires raw Redis connection)
            if let Some(mut redis_conn) = redis {
                let configs = kyomi_auth::analytics_quota::default_tier_configs();
                if let Some(config) = configs.get(&subscription_tier)
                    && let Err(e) = kyomi_auth::analytics_quota::sync_quota_to_redis(
                        &mut redis_conn,
                        &workspace_id,
                        config,
                    )
                    .await
                {
                    warn!(error = %e, "Failed to sync analytics quota to Redis");
                }
            } else {
                tracing::debug!("Skipping analytics quota sync — Redis not available");
            }

            // 2. Catalog indexing
            let embed = match embedding.wait_ready().await {
                Ok(e) => e,
                Err(e) => {
                    warn!(error = %e, "Embedding model not ready, skipping catalog indexing");
                    return;
                }
            };

            let result = Self::index_datasource(IndexDatasourceParams {
                db: &db,
                encryption_key,
                embedding: embed,
                workspace_id: &workspace_id,
                datasource_config_id: &datasource_id,
                user_email: None,
                credentials: None,
                max_tables_per_dataset: None,
                force: false,
                connect_registry: None,
            })
            .await;

            info!(
                datasource_id = %datasource_id,
                workspace_id = %workspace_id,
                status = ?result.status,
                tables = result.tables_indexed,
                "Background catalog indexing completed for new analytics datasource"
            );
        });
    }

    /// Resolve a single datasource config ID for a workspace + type.
    ///
    /// Used when the caller specifies a datasource type instead of a config ID.
    /// Returns `None` if zero or multiple datasources of that type exist.
    pub async fn resolve_datasource(
        db: &DbPool,
        workspace_id: &str,
        datasource_type: &str,
    ) -> Option<String> {
        let rows = kyomi_core::db_fetch_all!(
            db, DsConfigIdRow,
            "SELECT id FROM datasource_configs \
             WHERE workspace_id = $1 AND datasource_type = $2 \
             LIMIT 2",
            &workspace_id,
            &datasource_type
        )
        .ok()?;

        if rows.len() == 1 {
            Some(rows[0].id.clone())
        } else {
            None // ambiguous or none found
        }
    }
}

/// Generate embeddings for freshly indexed catalog data.
///
/// Failures are logged as warnings — they never fail the indexing response.
async fn populate_embeddings_after_indexing(
    db: &DbPool,
    embedding: &EmbeddingService,
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
                    info!(
                        workspace_id,
                        datasource_config_id,
                        tables = table_count,
                        columns = col_count,
                        "Embeddings populated after catalog indexing"
                    );
                }
                Err(e) => {
                    warn!(error = %e, "Column embedding population failed, continuing");
                }
            }
        }
        Err(e) => {
            warn!(error = %e, "Table embedding population failed, continuing");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn get_indexer_returns_some_for_all_10_types() {
        assert!(get_indexer(&DatasourceType::Postgres).is_some());
        assert!(get_indexer(&DatasourceType::MySQL).is_some());
        assert!(get_indexer(&DatasourceType::ClickHouse).is_some());
        assert!(get_indexer(&DatasourceType::Snowflake).is_some());
        assert!(get_indexer(&DatasourceType::Databricks).is_some());
        assert!(get_indexer(&DatasourceType::Redshift).is_some());
        assert!(get_indexer(&DatasourceType::SqlServer).is_some());
        assert!(get_indexer(&DatasourceType::Synapse).is_some());
        assert!(get_indexer(&DatasourceType::BigQuery).is_some());
        assert!(get_indexer(&DatasourceType::FlareDb).is_some());
    }
}
