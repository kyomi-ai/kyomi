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
    archive_missing_tables, cache_table, resolve_final_status, update_datasource_last_refresh,
    update_datasource_status, IndexerContext,
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

        // Mark this datasource's catalog refresh as running
        let _ = update_datasource_status(
            db,
            workspace_id,
            datasource_config_id,
            "running",
            Some(serde_json::json!({
                "total_projects": project_ids.len(),
                "processed": 0,
            })),
            None,
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
        let mut any_project_succeeded = false;

        for (i, project_id) in project_ids.iter().enumerate() {
            info!(
                project_id,
                index = i + 1,
                total = project_ids.len(),
                "indexing BigQuery project"
            );

            match index_project_datasets(IndexProjectParams {
                client: &client,
                db,
                embedding,
                ctx: &ctx,
                access_token,
                project_id,
                max_tables_per_dataset,
                seen_table_ids: &mut seen_table_ids,
            })
            .await
            {
                Ok((count, dataset_errors)) => {
                    any_project_succeeded = true;
                    tables_indexed += count;
                    errors.extend(dataset_errors);
                }
                Err(e) => {
                    // Check if this is an auth error (401) — skip silently
                    let err_str = format!("{e}");
                    if err_str.contains("401") || err_str.contains("Unauthorized") {
                        info!(
                            project_id,
                            "skipping project due to expired OAuth token"
                        );
                        let _ = update_datasource_status(
                            db,
                            workspace_id,
                            datasource_config_id,
                            "idle",
                            None,
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
            let _ = update_datasource_status(
                db,
                workspace_id,
                datasource_config_id,
                "running",
                Some(serde_json::json!({
                    "total_projects": project_ids.len(),
                    "processed": i + 1,
                    "tables_indexed": tables_indexed,
                })),
                None,
            )
            .await;
        }

        // Archive tables that no longer exist — only when we have positive evidence
        // of tables. If no tables were found (regardless of whether any project query
        // succeeded), preserve the existing catalog. A successful query returning 0
        // rows is just as unsafe to archive on as a failed query.
        let nothing_found = seen_table_ids.is_empty() && tables_indexed == 0;

        let tables_archived = if nothing_found {
            warn!(
                workspace_id,
                datasource_config_id,
                any_project_succeeded,
                "No tables found — preserving existing catalog (archiving skipped)"
            );
            0
        } else {
            let archived_names = archive_missing_tables(
                db,
                workspace_id,
                datasource_config_id,
                &seen_table_ids,
            )
            .await
            .unwrap_or_default();
            archived_names.len()
        };

        // Update last refresh timestamp
        let _ = update_datasource_last_refresh(db, datasource_config_id).await;

        // Record this datasource's resolved final status (KYO-264).
        // `nothing_found` alone can't distinguish a genuinely empty-but-
        // accessible set of datasets from a total discovery failure —
        // `resolve_final_status` (shared with the SQL path, KYO-126) draws
        // that line using the `errors` collected above, which now include
        // per-dataset failures via `fold_dataset_outcomes`.
        let (final_status, failure_reason) = resolve_final_status(nothing_found, &errors);
        let _ = update_datasource_status(
            db,
            workspace_id,
            datasource_config_id,
            final_status,
            None,
            failure_reason.as_deref(),
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

        // If nothing was found, return an error result so callers surface
        // the failure rather than silently reporting zero tables indexed.
        if nothing_found {
            let mut result =
                CatalogIndexResult::error("No tables discovered — existing catalog preserved")
                    .with_times(&start_time.to_rfc3339(), &end_time.to_rfc3339())
                    .with_ids(datasource_config_id, workspace_id);
            if !errors.is_empty() {
                result.errors = Some(errors);
            }
            return result;
        }

        let mut result = CatalogIndexResult::completed(tables_indexed, tables_archived)
            .with_times(&start_time.to_rfc3339(), &end_time.to_rfc3339())
            .with_ids(datasource_config_id, workspace_id);

        if !errors.is_empty() {
            result.errors = Some(errors);
        }

        result
    }
}

/// Parameters for [`index_project_datasets`].
struct IndexProjectParams<'a> {
    client: &'a reqwest::Client,
    db: &'a DbPool,
    embedding: &'a EmbeddingService,
    ctx: &'a IndexerContext,
    access_token: &'a str,
    project_id: &'a str,
    max_tables_per_dataset: Option<usize>,
    seen_table_ids: &'a mut HashSet<String>,
}

/// Parameters for [`index_dataset_tables`].
struct IndexDatasetParams<'a> {
    client: &'a reqwest::Client,
    db: &'a DbPool,
    embedding: &'a EmbeddingService,
    ctx: &'a IndexerContext,
    access_token: &'a str,
    project_id: &'a str,
    dataset_id: &'a str,
    max_tables: Option<usize>,
    seen_table_ids: &'a mut HashSet<String>,
}

/// Index all datasets in a single BigQuery project.
///
/// Lists datasets via BigQuery REST API, then iterates tables in each.
/// Returns the total number of tables indexed across all datasets in the
/// project, plus any per-dataset errors collected along the way (KYO-264) —
/// e.g. IAM allowing `bigquery.datasets.list` but denying
/// `bigquery.tables.list` on every dataset. The `Err` return here is
/// reserved for a whole-project failure (listing the datasets themselves
/// failed, including an expired-OAuth 401 the caller checks for) — it is
/// distinct from per-dataset errors, which are always returned via the `Ok`
/// tuple's error list so the caller can fold them into its `errors`
/// accumulator regardless of whether other datasets in the project
/// succeeded.
async fn index_project_datasets(
    params: IndexProjectParams<'_>,
) -> Result<(usize, Vec<String>)> {
    let IndexProjectParams {
        client,
        db,
        embedding,
        ctx,
        access_token,
        project_id,
        max_tables_per_dataset,
        seen_table_ids,
    } = params;
    // List all datasets in the project
    let datasets = list_bigquery_datasets(client, access_token, project_id).await?;

    let mut outcomes = Vec::with_capacity(datasets.len());

    for dataset_id in &datasets {
        let outcome = index_dataset_tables(IndexDatasetParams {
            client,
            db,
            embedding,
            ctx,
            access_token,
            project_id,
            dataset_id,
            max_tables: max_tables_per_dataset,
            seen_table_ids,
        })
        .await;

        if let Err(ref e) = outcome {
            warn!(
                project_id,
                dataset_id,
                error = %e,
                "failed to index dataset"
            );
        }

        outcomes.push((dataset_id.clone(), outcome));
    }

    Ok(fold_dataset_outcomes(project_id, outcomes))
}

/// Fold per-dataset indexing outcomes into a project's total indexed-table
/// count and a list of formatted per-dataset error strings.
///
/// KYO-264: before this existed, `index_project_datasets` only logged
/// (`warn!`) each per-dataset `Err` and discarded it — so a project where
/// every dataset's table listing failed (e.g. IAM allows
/// `bigquery.datasets.list` but denies `bigquery.tables.list` on every
/// dataset) still returned `Ok(0)` with an empty error list. That made a
/// total discovery failure indistinguishable, to `resolve_final_status`,
/// from a project that genuinely has zero datasets — both look like
/// `nothing_found == true` with `errors.is_empty() == true`, which resolves
/// to `"idle"`.
///
/// Deliberately free of I/O (`outcomes` are already-resolved `Result`s) so
/// this can be exercised directly by a unit test without an HTTP-mocking
/// dependency — none exists in this workspace, and this fold is the entire
/// piece of decision logic KYO-264 needs proven.
fn fold_dataset_outcomes(
    project_id: &str,
    outcomes: Vec<(String, Result<usize>)>,
) -> (usize, Vec<String>) {
    let mut tables_indexed = 0usize;
    let mut errors = Vec::new();

    for (dataset_id, outcome) in outcomes {
        match outcome {
            Ok(count) => tables_indexed += count,
            Err(e) => errors.push(format!("{project_id}.{dataset_id}: {e}")),
        }
    }

    (tables_indexed, errors)
}

/// Index all tables in a single BigQuery dataset.
///
/// Returns the number of tables indexed.
async fn index_dataset_tables(
    params: IndexDatasetParams<'_>,
) -> Result<usize> {
    let IndexDatasetParams {
        client,
        db,
        embedding,
        ctx,
        access_token,
        project_id,
        dataset_id,
        max_tables,
        seen_table_ids,
    } = params;
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

        let cached = cache_table(crate::catalog::helpers::CacheTableParams {
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
    use super::*;

    #[test]
    fn full_table_id_format() {
        let full = format!("{}.{}.{}", "my-project", "my_dataset", "my_table");
        assert_eq!(full, "my-project.my_dataset.my_table");
    }

    // ── fold_dataset_outcomes (KYO-264) ──────────────────────────────────

    fn permission_denied(dataset_id: &str) -> Result<usize> {
        Err(kyomi_core::Error::Internal(format!(
            "BigQuery tables list failed (HTTP 403): permission denied on {dataset_id}"
        )))
    }

    #[test]
    fn all_datasets_ok_sums_counts_with_no_errors() {
        let outcomes = vec![
            ("ds_a".to_string(), Ok(3usize)),
            ("ds_b".to_string(), Ok(2usize)),
        ];
        let (tables_indexed, errors) = fold_dataset_outcomes("proj-1", outcomes);
        assert_eq!(tables_indexed, 5);
        assert!(errors.is_empty());
    }

    /// The KYO-264 headline scenario: IAM allows listing datasets but denies
    /// `bigquery.tables.list` on every one of them. Before the fix,
    /// `index_project_datasets` only `warn!`-logged each of these and
    /// returned `Ok(0)` with an empty error list — indistinguishable from a
    /// project with zero real datasets. This asserts the errors actually
    /// reach the fold's output, and that each is prefixed with enough
    /// context (project + dataset id) to be actionable.
    #[test]
    fn all_datasets_failing_produces_non_empty_errors_and_zero_count() {
        let outcomes = vec![
            ("ds_a".to_string(), permission_denied("ds_a")),
            ("ds_b".to_string(), permission_denied("ds_b")),
        ];
        let (tables_indexed, errors) = fold_dataset_outcomes("proj-1", outcomes);
        assert_eq!(tables_indexed, 0);
        assert_eq!(errors.len(), 2);
        assert!(
            errors[0].starts_with("proj-1.ds_a: "),
            "expected project+dataset prefix, got: {}",
            errors[0]
        );
        assert!(
            errors[1].starts_with("proj-1.ds_b: "),
            "expected project+dataset prefix, got: {}",
            errors[1]
        );
    }

    #[test]
    fn mixed_outcomes_sum_successes_and_collect_only_failures() {
        let outcomes = vec![
            ("ds_a".to_string(), Ok(4usize)),
            ("ds_b".to_string(), permission_denied("ds_b")),
        ];
        let (tables_indexed, errors) = fold_dataset_outcomes("proj-1", outcomes);
        assert_eq!(tables_indexed, 4);
        assert_eq!(errors.len(), 1);
        assert!(errors[0].starts_with("proj-1.ds_b: "));
    }

    // ── error propagation end-to-end into resolve_final_status (KYO-264) ──
    //
    // These tie `fold_dataset_outcomes`'s output directly to the same
    // `resolve_final_status` call `index_workspace_catalog` makes, proving
    // the fix actually changes the persisted status — not just that errors
    // exist in a vec somewhere.

    #[test]
    fn errored_and_empty_resolves_to_failed_with_non_generic_reason() {
        let outcomes = vec![
            ("ds_a".to_string(), permission_denied("ds_a")),
            ("ds_b".to_string(), permission_denied("ds_b")),
        ];
        let (tables_indexed, errors) = fold_dataset_outcomes("proj-1", outcomes);
        // Mirrors index_workspace_catalog's `nothing_found` computation when
        // no table was ever seen (seen_table_ids stays empty on the same
        // input that produces tables_indexed == 0 here).
        let nothing_found = tables_indexed == 0;

        let (status, reason) = resolve_final_status(nothing_found, &errors);
        assert_eq!(status, "failed");
        let reason = reason.expect("failed status must carry a reason");
        assert_ne!(
            reason, "No tables discovered — existing catalog preserved",
            "reason must be the real per-dataset error, not the generic result message"
        );
        assert!(reason.contains("proj-1.ds_a"));
        assert!(reason.contains("permission denied"));
    }

    #[test]
    fn accessible_but_genuinely_empty_resolves_to_idle() {
        let outcomes = vec![
            ("ds_a".to_string(), Ok(0usize)),
            ("ds_b".to_string(), Ok(0usize)),
        ];
        let (tables_indexed, errors) = fold_dataset_outcomes("proj-1", outcomes);
        let nothing_found = tables_indexed == 0;

        let (status, reason) = resolve_final_status(nothing_found, &errors);
        assert_eq!(
            status, "idle",
            "datasets that were listed fine and are genuinely empty must not report failed"
        );
        assert_eq!(reason, None);
    }
}
