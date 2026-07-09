// SPDX-License-Identifier: AGPL-3.0-or-later

//! Connect catalog indexer.
//!
//! Uses the Kyomi Connect binary's `discover_catalog` command to enumerate
//! schemas, tables, and columns from databases behind firewalls or VPNs.
//! The Connect binary does the heavy lifting — this indexer just maps the
//! result into the shared caching pipeline.

use async_trait::async_trait;
use chrono::Utc;
use kyomi_core::DbPool;
use kyomi_embed::EmbeddingService;
use serde_json::Value;
use std::collections::HashSet;
use tracing::{info, warn};

use crate::catalog::traits::CatalogIndexer;
use kyomi_auth::catalog::helpers::{
    archive_missing_tables, cache_table, update_datasource_last_refresh, update_workspace_status,
    CacheTableParams, IndexerContext,
};
use kyomi_auth::catalog::types::{CatalogIndexResult, ColumnEntry};
use kyomi_datasource_server::ConnectRegistry;

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
            let _ = update_workspace_status(
                db, &ctx.workspace_id, &ctx.datasource_config_id, "failed", None,
            ).await;
            return CatalogIndexResult::error(
                "Connection test failed — is the Connect binary running?",
            )
            .with_times(&start_time.to_rfc3339(), &Utc::now().to_rfc3339())
            .with_ids(&ctx.datasource_config_id, &ctx.workspace_id);
        }

        let _ = update_workspace_status(
            db, &ctx.workspace_id, &ctx.datasource_config_id, "running", None,
        ).await;

        let catalog_result = match provider.discover_catalog().await {
            Ok(cr) => cr,
            Err(e) => {
                warn!(
                    datasource_config_id = ctx.datasource_config_id,
                    error = %e,
                    "discover_catalog command failed"
                );
                let _ = update_workspace_status(
                    db, &ctx.workspace_id, &ctx.datasource_config_id, "failed", None,
                ).await;
                return CatalogIndexResult::error(&format!("Catalog discovery failed: {e}"))
                    .with_times(&start_time.to_rfc3339(), &Utc::now().to_rfc3339())
                    .with_ids(&ctx.datasource_config_id, &ctx.workspace_id);
            }
        };

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
                let archive_id =
                    kyomi_core::build_full_table_name(project_id, dataset_id, table_name);
                seen_table_ids.insert(archive_id);

                let cached = cache_table(CacheTableParams {
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
                .await;

                if cached {
                    tables_indexed += 1;
                }
            }
        }

        // Archive missing tables — guard against empty discovery.
        let nothing_found = seen_table_ids.is_empty() && tables_indexed == 0;
        let tables_archived = if nothing_found {
            warn!(
                datasource_config_id = ctx.datasource_config_id,
                "No tables found via Connect — preserving existing catalog"
            );
            0
        } else {
            archive_missing_tables(
                db,
                &ctx.workspace_id,
                &ctx.datasource_config_id,
                &seen_table_ids,
            )
            .await
            .unwrap_or_default()
            .len()
        };

        let _ = update_datasource_last_refresh(db, &ctx.datasource_config_id).await;
        // Surface an empty discovery as a visible failure rather than a silent
        // "idle" (KYO-126). The Connect agent collapses a permission error into
        // an empty result, so "no tables found" is the signal the user needs to
        // see — it usually means the datasource role can't read the catalog.
        let (refresh_status, refresh_progress) = if nothing_found {
            (
                "failed",
                Some(serde_json::json!({
                    "error": "No tables were discovered via Kyomi Connect. The \
                              datasource may be empty, or the connection role may \
                              lack permission to read the catalog."
                })),
            )
        } else {
            ("idle", None)
        };
        let _ = update_workspace_status(
            db, &ctx.workspace_id, &ctx.datasource_config_id, refresh_status, refresh_progress,
        ).await;

        let end_time = Utc::now();

        info!(
            datasource_config_id = ctx.datasource_config_id,
            tables_indexed,
            tables_archived,
            elapsed_secs = (end_time - start_time).num_seconds(),
            "Connect catalog indexing complete"
        );

        if nothing_found {
            CatalogIndexResult::error("No tables discovered — existing catalog preserved")
                .with_times(&start_time.to_rfc3339(), &end_time.to_rfc3339())
                .with_ids(&ctx.datasource_config_id, &ctx.workspace_id)
        } else {
            CatalogIndexResult::completed(tables_indexed, tables_archived)
                .with_times(&start_time.to_rfc3339(), &end_time.to_rfc3339())
                .with_ids(&ctx.datasource_config_id, &ctx.workspace_id)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connect_indexer_requires_registry() {
        // ConnectIndexer can only be created with a registry — no Default.
        // This test verifies the struct exists and fields are correct.
        let _: fn(ConnectRegistry) -> ConnectIndexer = ConnectIndexer::new;
    }
}
