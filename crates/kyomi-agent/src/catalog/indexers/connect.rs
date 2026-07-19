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
            CatalogResult { containers: Vec::new() }
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
                    let _ = update_workspace_status(
                        db, &ctx.workspace_id, &ctx.datasource_config_id, "failed", None,
                    ).await;
                    return CatalogIndexResult::error(&format!("Catalog discovery failed: {e}"))
                        .with_times(&start_time.to_rfc3339(), &Utc::now().to_rfc3339())
                        .with_ids(&ctx.datasource_config_id, &ctx.workspace_id);
                }
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

        // Archive missing tables — guard against empty discovery. As on the
        // direct path, an *explicit empty selection* is exempt from the guard:
        // the user intentionally cleared the scope, so stale tables should be
        // archived rather than preserved.
        let nothing_found =
            seen_table_ids.is_empty() && tables_indexed == 0 && !explicit_empty;
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
        let _ = update_workspace_status(
            db, &ctx.workspace_id, &ctx.datasource_config_id, "idle", None,
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
}
