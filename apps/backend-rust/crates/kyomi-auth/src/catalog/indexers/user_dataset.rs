// SPDX-License-Identifier: AGPL-3.0-or-later

//! User Dataset Indexer.
//!
//! Indexes user-owned BigQuery datasets for workspace-specific catalog search.
//! Uses the user's OAuth token to discover datasets they've created.
//!
//! Python ref: `apps/backend-python/src/api/services/user_dataset_indexer.py` (783 lines).
//!
//! Architecture:
//! - Workspace-scoped: each workspace gets its own cache
//! - Requires valid OAuth tokens (access + refresh)
//! - Silent failure on expired tokens (just log and skip)
//! - Incremental updates with soft deletes (is_archived flag)
//! - Uses shared `cache_table()` from traits.rs

use chrono::Utc;
use kyomi_core::{DbPool, Result};
use kyomi_embed::EmbeddingService;
use serde_json::Value;
use std::collections::HashSet;
use std::sync::Arc;
use tracing::{info, warn};

use crate::catalog::helpers::{
    archive_missing_tables, cache_table, update_datasource_last_refresh,
    update_workspace_status, IndexerContext,
};
use crate::catalog::types::{CatalogIndexResult, ColumnEntry};

/// User Dataset Indexer service.
///
/// Indexes BigQuery datasets accessible via user OAuth into workspace-specific cache.
pub struct UserDatasetIndexer;

impl UserDatasetIndexer {
    /// Index all BigQuery datasets for a workspace using OAuth credentials.
    ///
    /// Iterates over the given project IDs, lists datasets and tables in each,
    /// and caches them with embeddings.
    ///
    /// Returns silently with "skipped" status if OAuth tokens are expired.
    pub async fn index_workspace_catalog(
        db: &DbPool,
        embedding: &EmbeddingService,
        workspace_id: &str,
        datasource_config_id: &str,
        access_token: &str,
        project_ids: &[String],
        max_tables_per_dataset: Option<usize>,
    ) -> CatalogIndexResult {
        let start_time = Utc::now();

        if project_ids.is_empty() {
            return CatalogIndexResult::skipped("No BigQuery project IDs provided")
                .with_times(&start_time.to_rfc3339(), &Utc::now().to_rfc3339())
                .with_ids(datasource_config_id, workspace_id);
        }

        // Update workspace status to running
        let _ = update_workspace_status(
            db,
            workspace_id,
            datasource_config_id,
            "running",
            Some(serde_json::json!({
                "total_projects": project_ids.len(),
                "processed": 0,
            })),
        )
        .await;

        let ctx = IndexerContext {
            workspace_id: workspace_id.to_string(),
            datasource_config_id: datasource_config_id.to_string(),
            connection_config: serde_json::json!({}),
            encryption_key: Arc::new([0u8; 32]),
        };

        let client = match crate::http_client() {
            Ok(c) => c,
            Err(e) => return CatalogIndexResult::error(&format!("Failed to build HTTP client: {e}")),
        };
        let mut tables_indexed = 0usize;
        let mut errors = Vec::new();
        let mut seen_table_ids = HashSet::new();

        for (i, project_id) in project_ids.iter().enumerate() {
            info!(
                project_id,
                index = i + 1,
                total = project_ids.len(),
                "indexing BigQuery project"
            );

            match index_project_datasets(
                &client,
                db,
                embedding,
                &ctx,
                access_token,
                project_id,
                max_tables_per_dataset,
                &mut seen_table_ids,
            )
            .await
            {
                Ok(count) => {
                    tables_indexed += count;
                }
                Err(e) => {
                    // Check if this is an auth error (401) — skip silently
                    let err_str = format!("{e}");
                    if err_str.contains("401") || err_str.contains("Unauthorized") {
                        info!(
                            project_id,
                            "skipping project due to expired OAuth token"
                        );
                        let _ = update_workspace_status(
                            db,
                            workspace_id,
                            datasource_config_id,
                            "idle",
                            None,
                        )
                        .await;

                        return CatalogIndexResult::skipped("OAuth token expired")
                            .with_times(
                                &start_time.to_rfc3339(),
                                &Utc::now().to_rfc3339(),
                            )
                            .with_ids(datasource_config_id, workspace_id);
                    }

                    warn!(
                        project_id,
                        error = %e,
                        "failed to index project"
                    );
                    errors.push(format!("{project_id}: {e}"));
                }
            }

            // Update progress
            let _ = update_workspace_status(
                db,
                workspace_id,
                datasource_config_id,
                "running",
                Some(serde_json::json!({
                    "total_projects": project_ids.len(),
                    "processed": i + 1,
                    "tables_indexed": tables_indexed,
                })),
            )
            .await;
        }

        // Archive tables that no longer exist
        let archived_names = archive_missing_tables(
            db,
            workspace_id,
            datasource_config_id,
            &seen_table_ids,
        )
        .await
        .unwrap_or_default();
        let tables_archived = archived_names.len();

        // Update last refresh timestamp
        let _ = update_datasource_last_refresh(db, datasource_config_id).await;

        // Update workspace status to idle
        let _ = update_workspace_status(
            db,
            workspace_id,
            datasource_config_id,
            "idle",
            None,
        )
        .await;

        let end_time = Utc::now();
        let elapsed = (end_time - start_time).num_seconds();

        info!(
            workspace_id,
            tables_indexed,
            tables_archived,
            errors = errors.len(),
            elapsed_secs = elapsed,
            "user dataset indexing complete"
        );

        let mut result = CatalogIndexResult::completed(tables_indexed, tables_archived)
            .with_times(&start_time.to_rfc3339(), &end_time.to_rfc3339())
            .with_ids(datasource_config_id, workspace_id);

        if !errors.is_empty() {
            result.errors = Some(errors);
        }

        result
    }
}

/// Index all datasets in a single BigQuery project.
///
/// Lists datasets via BigQuery REST API, then iterates tables in each.
/// Returns the total number of tables indexed.
async fn index_project_datasets(
    client: &reqwest::Client,
    db: &DbPool,
    embedding: &EmbeddingService,
    ctx: &IndexerContext,
    access_token: &str,
    project_id: &str,
    max_tables_per_dataset: Option<usize>,
    seen_table_ids: &mut HashSet<String>,
) -> Result<usize> {
    // List all datasets in the project
    let datasets = list_bigquery_datasets(client, access_token, project_id).await?;

    let mut tables_indexed = 0usize;

    for dataset_id in &datasets {
        match index_dataset_tables(
            client,
            db,
            embedding,
            ctx,
            access_token,
            project_id,
            dataset_id,
            max_tables_per_dataset,
            seen_table_ids,
        )
        .await
        {
            Ok(count) => {
                tables_indexed += count;
            }
            Err(e) => {
                warn!(
                    project_id,
                    dataset_id,
                    error = %e,
                    "failed to index dataset"
                );
            }
        }
    }

    Ok(tables_indexed)
}

/// Index all tables in a single BigQuery dataset.
///
/// Returns the number of tables indexed.
async fn index_dataset_tables(
    client: &reqwest::Client,
    db: &DbPool,
    embedding: &EmbeddingService,
    ctx: &IndexerContext,
    access_token: &str,
    project_id: &str,
    dataset_id: &str,
    max_tables: Option<usize>,
    seen_table_ids: &mut HashSet<String>,
) -> Result<usize> {
    let mut tables = list_bigquery_tables(client, access_token, project_id, dataset_id).await?;

    // Apply limit if specified
    if let Some(max) = max_tables {
        tables.truncate(max);
    }

    let mut tables_indexed = 0usize;

    for table_id in &tables {
        let full_table_id = format!("{project_id}.{dataset_id}.{table_id}");
        seen_table_ids.insert(full_table_id.clone());

        // Fetch table schema
        let columns =
            match get_bigquery_table_schema(client, access_token, project_id, dataset_id, table_id)
                .await
            {
                Ok(c) => c,
                Err(e) => {
                    warn!(
                        table = full_table_id,
                        error = %e,
                        "could not fetch schema, skipping"
                    );
                    continue;
                }
            };

        let cached = cache_table(
            db,
            embedding,
            ctx,
            project_id,
            dataset_id,
            table_id,
            "TABLE",
            &columns,
            &full_table_id,
        )
        .await;

        if cached {
            tables_indexed += 1;
        }
    }

    Ok(tables_indexed)
}

// ─── BigQuery REST API helpers ──────────────────────────────────────────────────

/// List all dataset IDs in a BigQuery project.
pub async fn list_bigquery_datasets(
    client: &reqwest::Client,
    access_token: &str,
    project_id: &str,
) -> Result<Vec<String>> {
    let url = format!(
        "https://bigquery.googleapis.com/bigquery/v2/projects/{project_id}/datasets"
    );

    let resp = client
        .get(&url)
        .query(&[("maxResults", super::BIGQUERY_API_MAX_RESULTS)])
        .header("Authorization", format!("Bearer {access_token}"))
        .timeout(std::time::Duration::from_secs(30))
        .send()
        .await
        .map_err(|e| {
            kyomi_core::Error::Internal(format!("BigQuery datasets list failed: {e}"))
        })?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(kyomi_core::Error::Internal(format!(
            "BigQuery datasets list failed (HTTP {status}): {body}"
        )));
    }

    let body: Value = resp.json().await.map_err(|e| {
        kyomi_core::Error::Internal(format!("Failed to parse BigQuery datasets response: {e}"))
    })?;

    let datasets = body
        .get("datasets")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|ds| {
                    ds.get("datasetReference")
                        .and_then(|r| r.get("datasetId"))
                        .and_then(|v| v.as_str())
                        .map(String::from)
                })
                .collect()
        })
        .unwrap_or_default();

    Ok(datasets)
}

/// List all table IDs in a BigQuery dataset.
pub async fn list_bigquery_tables(
    client: &reqwest::Client,
    access_token: &str,
    project_id: &str,
    dataset_id: &str,
) -> Result<Vec<String>> {
    let url = format!(
        "https://bigquery.googleapis.com/bigquery/v2/projects/{project_id}/datasets/{dataset_id}/tables"
    );

    let resp = client
        .get(&url)
        .query(&[("maxResults", super::BIGQUERY_API_MAX_RESULTS)])
        .header("Authorization", format!("Bearer {access_token}"))
        .timeout(std::time::Duration::from_secs(30))
        .send()
        .await
        .map_err(|e| {
            kyomi_core::Error::Internal(format!("BigQuery tables list failed: {e}"))
        })?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(kyomi_core::Error::Internal(format!(
            "BigQuery tables list failed (HTTP {status}): {body}"
        )));
    }

    let body: Value = resp.json().await.map_err(|e| {
        kyomi_core::Error::Internal(format!("Failed to parse BigQuery tables response: {e}"))
    })?;

    let tables = body
        .get("tables")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|t| {
                    t.get("tableReference")
                        .and_then(|r| r.get("tableId"))
                        .and_then(|v| v.as_str())
                        .map(String::from)
                })
                .collect()
        })
        .unwrap_or_default();

    Ok(tables)
}

/// Fetch column schema for a BigQuery table.
pub async fn get_bigquery_table_schema(
    client: &reqwest::Client,
    access_token: &str,
    project_id: &str,
    dataset_id: &str,
    table_id: &str,
) -> Result<Vec<ColumnEntry>> {
    let url = format!(
        "https://bigquery.googleapis.com/bigquery/v2/projects/{project_id}/datasets/{dataset_id}/tables/{table_id}"
    );

    let resp = client
        .get(&url)
        .header("Authorization", format!("Bearer {access_token}"))
        .timeout(std::time::Duration::from_secs(30))
        .send()
        .await
        .map_err(|e| {
            kyomi_core::Error::Internal(format!("BigQuery table get failed: {e}"))
        })?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(kyomi_core::Error::Internal(format!(
            "BigQuery table get failed (HTTP {status}): {body}"
        )));
    }

    let body: Value = resp.json().await.map_err(|e| {
        kyomi_core::Error::Internal(format!("Failed to parse BigQuery table response: {e}"))
    })?;

    let columns = body
        .get("schema")
        .and_then(|s| s.get("fields"))
        .and_then(|f| f.as_array())
        .map(|fields| {
            fields
                .iter()
                .filter_map(|field| {
                    let name = field.get("name")?.as_str()?.to_string();
                    let col_type = field.get("type").and_then(|v| v.as_str()).map(String::from);
                    let description = field
                        .get("description")
                        .and_then(|v| v.as_str())
                        .filter(|s| !s.is_empty())
                        .map(String::from);

                    Some(ColumnEntry {
                        name,
                        col_type: col_type.clone(),
                        native_type: col_type,
                        description,
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    Ok(columns)
}

#[cfg(test)]
mod tests {
    #[test]
    fn full_table_id_format() {
        let full = format!("{}.{}.{}", "my-project", "my_dataset", "my_table");
        assert_eq!(full, "my-project.my_dataset.my_table");
    }
}
