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

use super::bigquery_rest::{extract_column_entry, extract_table_id, MISSING_LIST_KEY_HINT};
use crate::catalog::helpers::{
    cache_table, fold_table_outcomes, resolve_final_status, DatasetOutcome, IndexerContext,
    TableOutcome,
};
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

            match index_public_dataset_tables(IndexPublicDatasetParams {
                client: &client,
                db,
                embedding,
                ctx: &ctx,
                access_token,
                project_id: PUBLIC_PROJECT_ID,
                dataset_id,
                max_tables: Some(max_tables),
            })
            .await
            {
                Ok(dataset_outcome) => {
                    tables_indexed += dataset_outcome.tables_indexed;
                    // `seen_table_ids` is ignored here — this indexer has no
                    // archiving machinery (see the status-persistence note
                    // below), so the only field of `DatasetOutcome` this
                    // caller needs is `table_errors`.
                    errors.extend(dataset_outcome.table_errors);
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

        // No status is persisted anywhere for this indexer (KYO-365). Unlike
        // `UserDatasetIndexer::index_workspace_catalog`, which calls
        // `update_datasource_status` against a real `datasource_configs`
        // row, this run's `IndexerContext` uses the sentinel ids
        // `PUBLIC_DATASOURCE_CONFIG_ID` / `PUBLIC_DATA_WORKSPACE_ID` — no
        // `datasource_configs` row and no `workspaces` row exist for those
        // ids (verified against every migration and write site), so an
        // `update_datasource_status` call here would match zero rows and
        // silently no-op. That would be worse than not calling it: it would
        // look like status is handled when it isn't. The only place this
        // run's outcome is visible to a caller is the returned
        // `CatalogIndexResult` — see `resolve_public_index_result`.
        resolve_public_index_result(tables_indexed, errors)
            .with_times(&start_time.to_rfc3339(), &end_time.to_rfc3339())
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

/// Resolve the terminal [`CatalogIndexResult`] for a public-dataset indexing
/// run from its two summary values — the total tables indexed and the
/// collected per-dataset/per-table errors — mirroring `resolve_run_outcome`
/// in `catalog::indexers::user_dataset`.
///
/// This indexer has no archiving and persists no status anywhere (see the
/// comment at the `resolve_public_index_result` call site in
/// `index_public_datasets`), so unlike `resolve_run_outcome` there is no
/// "should we archive?" question and no status to write to
/// `datasource_configs` — the only decision left is what
/// `CatalogIndexResult` to hand back, via the shared `resolve_final_status`
/// (KYO-126/KYO-264/KYO-324/KYO-364).
///
/// - `"failed"` (every table that was listed ended up unusable, or a
///   dataset-level error occurred with nothing usable produced) ⇒
///   `CatalogIndexResult::error(&reason)`, with `result.errors` then
///   overwritten to the FULL `errors` list — `error()` on its own only
///   stores a single-element `vec![message]`, which would silently drop
///   every error but the first.
/// - `"idle"` (anything indexed, or a genuinely empty run with no errors at
///   all) ⇒ `CatalogIndexResult::completed(tables_indexed, 0)`, with
///   `result.errors` set when the run had partial failures alongside its
///   successes.
///
/// Pure and I/O-free (mirrors `fold_table_outcomes` in `catalog::helpers`)
/// so this exact decision can be exercised directly by a unit test; see
/// `all_tables_listed_all_schemas_denied_resolves_to_failed` et al.
fn resolve_public_index_result(tables_indexed: usize, errors: Vec<String>) -> CatalogIndexResult {
    let nothing_usable = tables_indexed == 0;
    let (status, failure_reason) = resolve_final_status(nothing_usable, &errors);

    if status == "failed" {
        let reason = failure_reason.expect("a \"failed\" status always carries a reason");
        let mut result = CatalogIndexResult::error(&reason);
        result.errors = Some(errors);
        return result;
    }

    let mut result = CatalogIndexResult::completed(tables_indexed, 0);
    if !errors.is_empty() {
        result.errors = Some(errors);
    }
    result
}

/// Parameters for [`index_public_dataset_tables`].
struct IndexPublicDatasetParams<'a> {
    client: &'a reqwest::Client,
    db: &'a DbPool,
    embedding: &'a EmbeddingService,
    ctx: &'a IndexerContext,
    access_token: &'a str,
    project_id: &'a str,
    dataset_id: &'a str,
    max_tables: Option<usize>,
}

/// Index all tables in a single public BigQuery dataset.
///
/// Returns a [`DatasetOutcome`] carrying the indexed-table count and any
/// bounded per-table failures — schema-fetch denials
/// (`TableOutcome::SchemaUnreadable`) and `cache_table` write failures
/// (`TableOutcome::WriteFailed`) alike, folded by the shared
/// `fold_table_outcomes` (KYO-365, sharing the machinery
/// `catalog::indexers::user_dataset` gained in KYO-324/KYO-364). The `Err`
/// return here is reserved for the table *listing* call itself failing; a
/// per-table failure never short-circuits the loop — it is captured as a
/// `TableOutcome` and folded into the `Ok` result instead, exactly like
/// `user_dataset::index_dataset_tables`. This indexer has no archiving, so
/// `seen_table_ids` on the returned `DatasetOutcome` is folded but ignored
/// by the caller.
async fn index_public_dataset_tables(
    params: IndexPublicDatasetParams<'_>,
) -> Result<DatasetOutcome> {
    let IndexPublicDatasetParams {
        client,
        db,
        embedding,
        ctx,
        access_token,
        project_id,
        dataset_id,
        max_tables,
    } = params;
    // List tables in the dataset. Paginates via `nextPageToken` (KYO-619)
    // and errs, rather than silently returning zero, when a page's `tables`
    // key is absent — see `bigquery_rest::parse_list_field`.
    let url = format!(
        "https://bigquery.googleapis.com/bigquery/v2/projects/{project_id}/datasets/{dataset_id}/tables"
    );
    let request_label = format!("BigQuery tables list for {project_id}.{dataset_id}");

    let mut table_ids: Vec<String> = super::bigquery_rest::paginate(
        "tables",
        extract_table_id,
        Some(MISSING_LIST_KEY_HINT),
        |page_token| {
            super::bigquery_rest::fetch_bigquery_list_page(
                client,
                access_token,
                &url,
                page_token,
                &request_label,
            )
        },
    )
    .await?;

    // Apply limit
    if let Some(max) = max_tables {
        table_ids.truncate(max);
    }

    let mut outcomes = Vec::with_capacity(table_ids.len());

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
                outcomes.push((full_table_id, TableOutcome::SchemaUnreadable(format!("{e}"))));
                continue;
            }
        };

        let outcome = match cache_table(crate::catalog::helpers::CacheTableParams {
            db,
            embedding,
            ctx,
            project_id,
            dataset_id,
            table_name: table_id,
            table_type: "TABLE",
            columns: &columns,
            full_table_id: &full_table_id,
        })
        .await
        {
            Ok(()) => TableOutcome::Indexed,
            Err(e) => {
                warn!(
                    table = full_table_id,
                    error = %e,
                    "failed to cache public table"
                );
                TableOutcome::WriteFailed(format!("{e}"))
            }
        };
        outcomes.push((full_table_id, outcome));
    }

    let dataset_label = format!("{project_id}.{dataset_id}");
    Ok(fold_table_outcomes(&dataset_label, outcomes))
}

/// Fetch column schema for a public BigQuery table.
///
/// `tables.get` returns a single resource, not a paginated list, so this
/// makes exactly one request. It still routes through
/// `bigquery_rest::parse_list_field` for `schema.fields` so an absent
/// `fields` key errs instead of silently reporting zero columns (KYO-619).
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

    let schema = body.get("schema").ok_or_else(|| {
        kyomi_core::Error::Internal(
            "BigQuery table response missing expected \"schema\" field".to_string(),
        )
    })?;

    super::bigquery_rest::parse_list_field(schema, "fields", extract_column_entry, None)
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

    // ── resolve_public_index_result (KYO-365) ────────────────────────────
    //
    // These exercise the same composition `index_public_datasets` calls —
    // `fold_table_outcomes` (shared with `user_dataset.rs`, KYO-324/KYO-364)
    // feeding `resolve_public_index_result` (which wraps the shared
    // `resolve_final_status`, KYO-126/KYO-264) — rather than re-deriving the
    // idle/failed decision inline. A test that re-derives the production
    // decision instead of calling it would keep passing even if
    // `index_public_datasets`'s call site regressed back to an unconditional
    // `completed(..)`, which is exactly the bug this ticket exists to fix.

    fn schema_denied(table: &str) -> TableOutcome {
        TableOutcome::SchemaUnreadable(format!("HTTP 403: permission denied reading {table}"))
    }

    fn write_failed(table: &str) -> TableOutcome {
        TableOutcome::WriteFailed(format!("failed to insert cache entry for {table}: db closed"))
    }

    /// AC1 (headline): every table listed, every schema fetch denied ⇒ a
    /// failure whose reason names the real underlying error — NOT
    /// `completed(0, 0)`. This is the exact KYO-365 scenario: a public
    /// dataset the token can list but not read (`bigquery.tables.list`
    /// granted, `bigquery.tables.get` denied on every table).
    #[test]
    fn all_tables_listed_all_schemas_denied_resolves_to_failed() {
        let table_ids = ["bigquery-public-data.chicago_taxi_trips.t1", "bigquery-public-data.chicago_taxi_trips.t2"];
        let outcomes: Vec<(String, TableOutcome)> = table_ids
            .iter()
            .map(|t| (t.to_string(), schema_denied(t)))
            .collect();
        let dataset_outcome = fold_table_outcomes("bigquery-public-data.chicago_taxi_trips", outcomes);

        let result = resolve_public_index_result(
            dataset_outcome.tables_indexed,
            dataset_outcome.table_errors,
        );

        assert_eq!(
            result.status, "error",
            "a run where every listed table's schema fetch was denied must not report completed(0, 0)"
        );
        assert_eq!(result.tables_indexed, 0);
        let errors = result.errors.expect("error status must carry errors");
        assert!(
            errors[0].contains("bigquery-public-data.chicago_taxi_trips.t1"),
            "reason must name the real underlying error, got: {}",
            errors[0]
        );
        assert!(
            errors[0].contains("permission denied"),
            "reason must name the real underlying error, got: {}",
            errors[0]
        );
    }

    /// Criterion 4: a blanket `cache_table` write failure (schema reads all
    /// succeeded, every write failed) gets the same failure treatment as a
    /// blanket schema denial — not a silent `completed(0, 0)`.
    #[test]
    fn all_tables_listed_all_writes_failed_resolves_to_failed() {
        let table_ids = ["bigquery-public-data.samples.t1", "bigquery-public-data.samples.t2"];
        let outcomes: Vec<(String, TableOutcome)> = table_ids
            .iter()
            .map(|t| (t.to_string(), write_failed(t)))
            .collect();
        let dataset_outcome = fold_table_outcomes("bigquery-public-data.samples", outcomes);

        let result = resolve_public_index_result(
            dataset_outcome.tables_indexed,
            dataset_outcome.table_errors,
        );

        assert_eq!(result.status, "error");
        assert_eq!(result.tables_indexed, 0);
        let errors = result.errors.expect("error status must carry errors");
        assert!(
            errors[0].contains("bigquery-public-data.samples.t1"),
            "reason must name the failing table, got: {}",
            errors[0]
        );
        assert!(
            errors[0].contains("failed to insert cache entry"),
            "reason must contain the real underlying cache_table error text, got: {}",
            errors[0]
        );
    }

    /// AC2: some tables indexed, some schema fetches failed ⇒ still success
    /// (`"completed"`), with the failures surfaced via
    /// `CatalogIndexResult::errors` rather than silently dropped.
    #[test]
    fn partial_success_still_completes_with_errors_surfaced() {
        let outcomes = vec![
            ("bigquery-public-data.github_repos.t1".to_string(), TableOutcome::Indexed),
            (
                "bigquery-public-data.github_repos.t2".to_string(),
                schema_denied("bigquery-public-data.github_repos.t2"),
            ),
        ];
        let dataset_outcome = fold_table_outcomes("bigquery-public-data.github_repos", outcomes);

        let result = resolve_public_index_result(
            dataset_outcome.tables_indexed,
            dataset_outcome.table_errors,
        );

        assert_eq!(result.status, "completed");
        assert_eq!(result.tables_indexed, 1);
        let errors = result.errors.expect("partial failures must still be surfaced");
        assert_eq!(errors.len(), 1);
        assert!(errors[0].starts_with("bigquery-public-data.github_repos.t2: "));
    }

    /// AC3 (regression guard on `resolve_final_status`'s idle branch): a
    /// genuinely empty run — no tables, no errors — must still be
    /// `"completed"`, not a failure. A dataset that is accessible and
    /// genuinely has zero tables is not an error.
    #[test]
    fn genuinely_empty_run_still_completes() {
        let dataset_outcome = fold_table_outcomes("bigquery-public-data.empty_dataset", Vec::new());

        let result = resolve_public_index_result(
            dataset_outcome.tables_indexed,
            dataset_outcome.table_errors,
        );

        assert_eq!(result.status, "completed");
        assert_eq!(result.tables_indexed, 0);
        assert_eq!(result.errors, None);
    }
}
