// SPDX-License-Identifier: AGPL-3.0-or-later

//! Catalog indexing service -- routes indexing requests to the correct provider indexer.
//!
//! Mirrors Python's `CatalogIndexingService` from `datasources/catalog_service.py`.
//!
//! The service loads datasource configuration from the database, determines the
//! correct indexer implementation, and dispatches the indexing call. Provider-
//! specific indexer implementations are registered in [`get_indexer`].

use kyomi_core::datasource_registry::{self, DatasourceType};
use kyomi_core::DbPool;
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
    name: String,
}

/// Row type for listing datasource configs (id + type only).
#[derive(sqlx::FromRow)]
struct DsConfigIdTypeRow {
    id: String,
    datasource_type: String,
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
/// Returns `Some` with a concrete indexer for all 9 supported datasource types.
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
    }
}

// ─── CatalogIndexingService ────────────────────────────────────────────────────

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
    pub async fn index_datasource(
        db: &DbPool,
        encryption_key: std::sync::Arc<[u8; 32]>,
        embedding: &EmbeddingService,
        workspace_id: &str,
        datasource_config_id: &str,
        user_email: Option<&str>,
        credentials: Option<&Value>,
        max_tables_per_dataset: Option<usize>,
    ) -> CatalogIndexResult {
        // Load datasource config
        let ds_config = kyomi_core::db_fetch_optional!(
            db, DsConfigRow,
            "SELECT datasource_type, connection_config, name \
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
        let ds_name = ds_config.name;

        // Parse datasource type
        let ds_type = match DatasourceType::from_str(&ds_type_str) {
            Ok(t) => t,
            Err(_) => {
                return CatalogIndexResult::error(&format!(
                    "Unsupported datasource type: '{ds_type_str}'"
                ));
            }
        };

        // Get the indexer for this type
        let indexer = match get_indexer(&ds_type) {
            Some(i) => i,
            None => {
                return CatalogIndexResult::skipped(&format!(
                    "No catalog indexer available for datasource type '{ds_type_str}'"
                ));
            }
        };

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
            "starting catalog indexing"
        );

        indexer
            .index_catalog(&ctx, db, embedding, user_email, credentials, max_tables_per_dataset)
            .await
    }

    /// Index all datasources in a workspace.
    ///
    /// Iterates over all datasource configs for the workspace and indexes
    /// each one that has a registered indexer.
    pub async fn index_workspace(
        db: &DbPool,
        encryption_key: std::sync::Arc<[u8; 32]>,
        embedding: &EmbeddingService,
        workspace_id: &str,
        user_email: Option<&str>,
        credentials: Option<&Value>,
        max_tables_per_dataset: Option<usize>,
    ) -> Vec<CatalogIndexResult> {
        let ds_configs = kyomi_core::db_fetch_all!(
            db, DsConfigIdTypeRow,
            "SELECT id, datasource_type FROM datasource_configs WHERE workspace_id = $1",
            &workspace_id
        );

        let ds_configs = match ds_configs {
            Ok(rows) => rows,
            Err(e) => {
                return vec![CatalogIndexResult::error(&format!(
                    "Failed to list workspace datasources: {e}"
                ))];
            }
        };

        let mut results = Vec::new();

        for row in &ds_configs {
            let config_id = &row.id;
            let ds_type_str = &row.datasource_type;

            // Skip unsupported types
            if !datasource_registry::is_supported_type(ds_type_str) {
                warn!(
                    workspace_id,
                    datasource_config_id = %config_id,
                    datasource_type = %ds_type_str,
                    "skipping unsupported datasource type"
                );
                continue;
            }

            let result = Self::index_datasource(
                db,
                encryption_key.clone(),
                embedding,
                workspace_id,
                config_id,
                user_email,
                credentials,
                max_tables_per_dataset,
            )
            .await;

            results.push(result);
        }

        results
    }

    /// Check if a datasource can be refreshed (respects 24hr rate limit).
    pub async fn can_refresh_datasource(
        db: &DbPool,
        datasource_config_id: &str,
        hours_threshold: i64,
    ) -> bool {
        can_refresh_now(db, datasource_config_id, hours_threshold).await
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

            let result = Self::index_datasource(
                &db,
                encryption_key,
                embed,
                &workspace_id,
                &datasource_id,
                None, // no user email — shared credentials
                None, // no per-user credentials — uses connection_config
                None, // default max tables
            )
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn get_indexer_returns_some_for_all_9_types() {
        assert!(get_indexer(&DatasourceType::Postgres).is_some());
        assert!(get_indexer(&DatasourceType::MySQL).is_some());
        assert!(get_indexer(&DatasourceType::ClickHouse).is_some());
        assert!(get_indexer(&DatasourceType::Snowflake).is_some());
        assert!(get_indexer(&DatasourceType::Databricks).is_some());
        assert!(get_indexer(&DatasourceType::Redshift).is_some());
        assert!(get_indexer(&DatasourceType::SqlServer).is_some());
        assert!(get_indexer(&DatasourceType::Synapse).is_some());
        assert!(get_indexer(&DatasourceType::BigQuery).is_some());
    }
}
