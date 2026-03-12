// SPDX-License-Identifier: AGPL-3.0-or-later

//! BigQuery Public Dataset Indexer.
//!
//! Indexes `bigquery-public-data` project into a shared sentinel workspace
//! (`public-data-workspace`). All workspaces that opt in to public datasets
//! share this one copy of the index.
//!
//! Python ref: `apps/backend-python/src/api/services/bigquery_public_indexer.py` (768 lines).
//!
//! Architecture:
//! - Uses BigQuery REST API (no user OAuth needed for public data metadata)
//! - Stores in sentinel workspace `public-data-workspace`
//! - Top 21 curated datasets by default, or all datasets in stress-test mode
//! - Daily/weekly refresh threshold

use chrono::{DateTime, TimeDelta, Utc};
use kyomi_core::{db_fetch_scalar, DbPool, Result};
use kyomi_embed::EmbeddingService;
use serde_json::Value;
use std::sync::Arc;
use tracing::{debug, info, warn};

use crate::catalog::helpers::{cache_table, IndexerContext};
use crate::catalog::types::{CatalogIndexResult, ColumnEntry};

/// Sentinel workspace ID for shared public BigQuery data.
pub const PUBLIC_DATA_WORKSPACE_ID: &str = "public-data-workspace";

/// Sentinel datasource config ID for public data.
const PUBLIC_DATASOURCE_CONFIG_ID: &str = "bigquery-public-indexer";

/// The BigQuery public data project ID.
const PUBLIC_PROJECT_ID: &str = "bigquery-public-data";

/// Top 21 most interesting public datasets to index by default.
const TOP_PUBLIC_DATASETS: &[&str] = &[
    "bitcoin_blockchain",
    "blockchain_analytics_ethereum_mainnet_us",
    "chicago_taxi_trips",
    "austin_bikeshare",
    "new_york_citibike",
    "america_health_rankings",
    "census_bureau_acs",
    "covid19_google_mobility",
    "austin_311",
    "austin_crime",
    "chicago_crime",
    "google_analytics_sample",
    "google_trends",
    "github_repos",
    "stackoverflow",
    "hacker_news",
    "noaa_gsod",
    "fda_food",
    "ml_datasets",
    "samples",
    "utility_us",
];

/// BigQuery Public Dataset Indexer service.
pub struct BigQueryPublicIndexer;

impl BigQueryPublicIndexer {
    /// Check if public dataset index needs refresh.
    pub async fn needs_refresh(db: &DbPool, hours_threshold: i64) -> bool {
        #[derive(sqlx::FromRow)]
        struct LastUpdate {
            last_update: Option<DateTime<Utc>>,
        }

        let row = kyomi_core::db_fetch_optional!(
            db,
            LastUpdate,
            "SELECT MAX(updated_at) as last_update FROM datasource_table_cache WHERE workspace_id = $1",
            PUBLIC_DATA_WORKSPACE_ID
        );

        let Ok(Some(row)) = row else {
            return true;
        };

        match row.last_update {
            None => true,
            Some(ts) => {
                let elapsed: TimeDelta = Utc::now() - ts;
                elapsed.num_hours() >= hours_threshold
            }
        }
    }

    /// Index the top curated public datasets.
    ///
    /// Uses BigQuery REST API to discover tables and columns in each dataset.
    /// Does NOT require user authentication — public metadata is accessible
    /// with just an API key or even anonymous access for some operations.
    ///
    /// Note: In production, this requires a GCP API key or service account
    /// since even public metadata queries need authentication for rate limiting.
    /// The `access_token` parameter should be a service account token or API key.
    pub async fn index_public_datasets(
        db: &DbPool,
        embedding: &EmbeddingService,
        access_token: &str,
        max_tables_per_dataset: Option<usize>,
    ) -> CatalogIndexResult {
        let start_time = Utc::now();

        if access_token.is_empty() {
            return CatalogIndexResult::skipped(
                "No access token available for BigQuery public dataset indexing",
            )
            .with_times(&start_time.to_rfc3339(), &Utc::now().to_rfc3339());
        }

        let ctx = IndexerContext {
            workspace_id: PUBLIC_DATA_WORKSPACE_ID.to_string(),
            datasource_config_id: PUBLIC_DATASOURCE_CONFIG_ID.to_string(),
            connection_config: serde_json::json!({}),
            encryption_key: Arc::new([0u8; 32]),
        };

        let client = match crate::http_client() {
            Ok(c) => c,
            Err(e) => return CatalogIndexResult::error(&format!("Failed to build HTTP client: {e}")),
        };
        let max_tables = max_tables_per_dataset.unwrap_or(50);

        let mut tables_indexed = 0usize;
        let mut datasets_processed = 0usize;
        let mut errors = Vec::new();

        info!(
            datasets = TOP_PUBLIC_DATASETS.len(),
            max_tables_per_dataset = max_tables,
            "starting BigQuery public dataset indexing"
        );

        for (i, dataset_id) in TOP_PUBLIC_DATASETS.iter().enumerate() {
            debug!(
                dataset_id,
                index = i + 1,
                total = TOP_PUBLIC_DATASETS.len(),
                "indexing public dataset"
            );

            match index_public_dataset_tables(
                &client,
                db,
                embedding,
                &ctx,
                access_token,
                PUBLIC_PROJECT_ID,
                dataset_id,
                Some(max_tables),
            )
            .await
            {
                Ok(count) => {
                    tables_indexed += count;
                    datasets_processed += 1;
                }
                Err(e) => {
                    let msg = format!("Failed to index public dataset {dataset_id}: {e}");
                    warn!("{msg}");
                    errors.push(msg);
                }
            }
        }

        let end_time = Utc::now();
        let elapsed = (end_time - start_time).num_seconds();

        info!(
            datasets_processed,
            tables_indexed,
            errors = errors.len(),
            elapsed_secs = elapsed,
            "BigQuery public dataset indexing complete"
        );

        let mut result = CatalogIndexResult::completed(tables_indexed, 0)
            .with_times(&start_time.to_rfc3339(), &end_time.to_rfc3339());

        if !errors.is_empty() {
            result.errors = Some(errors);
        }

        result
    }

    /// Get cached public tables.
    pub async fn get_public_table_count(db: &DbPool) -> i64 {
        db_fetch_scalar!(
            db,
            i64,
            "SELECT COUNT(*) FROM datasource_table_cache WHERE workspace_id = $1",
            PUBLIC_DATA_WORKSPACE_ID
        )
        .unwrap_or(0)
    }
}

/// Index all tables in a single public BigQuery dataset.
///
/// Returns the number of tables indexed.
async fn index_public_dataset_tables(
    client: &reqwest::Client,
    db: &DbPool,
    embedding: &EmbeddingService,
    ctx: &IndexerContext,
    access_token: &str,
    project_id: &str,
    dataset_id: &str,
    max_tables: Option<usize>,
) -> Result<usize> {
    // List tables in the dataset
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
            "BigQuery tables list failed for {project_id}.{dataset_id} (HTTP {status}): {body}"
        )));
    }

    let body: Value = resp.json().await.map_err(|e| {
        kyomi_core::Error::Internal(format!("Failed to parse BigQuery tables response: {e}"))
    })?;

    let mut table_ids: Vec<String> = body
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

    // Apply limit
    if let Some(max) = max_tables {
        table_ids.truncate(max);
    }

    let mut tables_indexed = 0usize;

    for table_id in &table_ids {
        let full_table_id = format!("{project_id}.{dataset_id}.{table_id}");

        // Fetch table schema
        let columns = match get_public_table_schema(
            client,
            access_token,
            project_id,
            dataset_id,
            table_id,
        )
        .await
        {
            Ok(c) => c,
            Err(e) => {
                warn!(
                    table = full_table_id,
                    error = %e,
                    "could not fetch schema for public table, skipping"
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

/// Fetch column schema for a public BigQuery table.
async fn get_public_table_schema(
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
    use super::*;

    #[test]
    fn public_data_workspace_id_is_stable() {
        assert_eq!(PUBLIC_DATA_WORKSPACE_ID, "public-data-workspace");
    }

    #[test]
    fn top_public_datasets_count() {
        assert_eq!(TOP_PUBLIC_DATASETS.len(), 21);
    }

    #[test]
    fn top_public_datasets_are_unique() {
        let mut seen = std::collections::HashSet::new();
        for ds in TOP_PUBLIC_DATASETS {
            assert!(seen.insert(ds), "duplicate dataset: {ds}");
        }
    }

    #[test]
    fn public_project_id_is_correct() {
        assert_eq!(PUBLIC_PROJECT_ID, "bigquery-public-data");
    }
}
