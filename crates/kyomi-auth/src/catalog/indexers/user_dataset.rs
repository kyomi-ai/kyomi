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

        let outcome = resolve_run_outcome(!seen_table_ids.is_empty(), tables_indexed, &errors);

        // Archive tables that no longer exist — see `resolve_run_outcome` for
        // why this is gated on "was anything listed" rather than "was
        // anything usable".
        let tables_archived = if outcome.archive {
            let archived_names = archive_missing_tables(
                db,
                workspace_id,
                datasource_config_id,
                &seen_table_ids,
            )
            .await
            .unwrap_or_default();
            archived_names.len()
        } else {
            warn!(
                workspace_id,
                datasource_config_id,
                any_project_succeeded,
                "No tables found — preserving existing catalog (archiving skipped)"
            );
            0
        };

        // Update last refresh timestamp
        let _ = update_datasource_last_refresh(db, datasource_config_id).await;

        let _ = update_datasource_status(
            db,
            workspace_id,
            datasource_config_id,
            outcome.status,
            None,
            outcome.failure_reason.as_deref(),
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

        // If nothing usable came out of this run, return an error result so
        // callers surface the failure rather than silently reporting zero
        // tables indexed — see `resolve_run_outcome` for what the message
        // does and doesn't claim.
        if let Some(message) = outcome.error_message {
            let mut result = CatalogIndexResult::error(message)
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

/// The end-of-run decisions `index_workspace_catalog` makes once every
/// project has been walked: whether to archive, what status to persist, and
/// what the returned `CatalogIndexResult` should say.
struct RunOutcome {
    /// Whether `archive_missing_tables` should run at all.
    archive: bool,
    /// Value written to `datasource_configs.catalog_refresh_status`.
    status: &'static str,
    /// Failure reason persisted alongside a `"failed"` status.
    failure_reason: Option<String>,
    /// `Some(msg)` when the run produced nothing usable and the caller must
    /// return `CatalogIndexResult::error(msg)` instead of `completed(..)`.
    error_message: Option<&'static str>,
}

/// Resolve the two questions `index_workspace_catalog` needs answered once
/// every project has been walked — "should we archive?" and "what status do
/// we persist, with what reason and caller-facing message?"
///
/// `seen_any_table` answers "did discovery (listing) see anything?" — it
/// must be derived from `seen_table_ids`, which, since KYO-324, is
/// populated with every table a dataset *listing* returned, including ones
/// whose schema fetch subsequently failed (see `fold_table_outcomes`).
/// `tables_indexed` / `errors` answer "did we get anything *usable*?" — a
/// table can be listed and still contribute nothing if its schema read was
/// denied. These are genuinely different questions, and conflating them
/// back into one predicate is the exact KYO-324 regression: a run where
/// every table listed fine but every `get_bigquery_table_schema` call was
/// denied (a real BigQuery IAM split — `bigquery.tables.list` granted,
/// `bigquery.tables.get` denied) used to report `catalog_refresh_status =
/// 'idle'` with 0 tables and no reason, because the single predicate this
/// replaces (`seen_table_ids.is_empty() && tables_indexed == 0`) is false
/// the moment anything was listed at all — a silent success.
///
/// - **Archiving** only ever needs the first question. If nothing was ever
///   listed (regardless of whether any project query succeeded), the
///   existing catalog must be preserved — a successful listing that
///   enumerates 0 tables is just as unsafe to archive on as a failed one.
///   A table whose schema fetch failed still counts as "listed", so a
///   blanket `bigquery.tables.get` denial does not evict tables that
///   demonstrably still exist (KYO-324).
/// - **Status** needs the second question. `resolve_final_status` (shared
///   with the SQL path, KYO-126) draws the idle/failed line using `errors`,
///   which include per-dataset failures via `fold_dataset_outcomes` AND
///   per-table schema-fetch failures via `fold_table_outcomes` (KYO-324).
/// - **The returned error message** must not claim more than what actually
///   happened: when nothing was ever listed, no archiving ran; when tables
///   WERE listed but none of them ended up indexed, archiving already ran and
///   preserved exactly those listed tables. Note the second message says
///   "indexed", not "read": a listed table also fails to be indexed when its
///   schema read succeeded but `cache_table` declined the write
///   (`TableOutcome::NotCached`), so naming the read specifically would
///   overclaim on that path.
///
/// Pure and I/O-free (mirrors `fold_dataset_outcomes` / `fold_table_outcomes`
/// above) so this exact decision — not a re-derivation of it — can be
/// exercised directly by a unit test; see
/// `all_tables_listed_all_schemas_denied_resolves_to_failed` et al.
fn resolve_run_outcome(
    seen_any_table: bool,
    tables_indexed: usize,
    errors: &[String],
) -> RunOutcome {
    let nothing_listed = !seen_any_table;
    let nothing_usable = tables_indexed == 0;

    let (status, failure_reason) = resolve_final_status(nothing_usable, errors);

    let message = if nothing_listed {
        "No tables discovered — existing catalog preserved"
    } else {
        "Tables were listed but none could be indexed — existing catalog preserved for the tables still present"
    };
    let error_message = nothing_usable.then_some(message);

    RunOutcome {
        archive: !nothing_listed,
        status,
        failure_reason,
        error_message,
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
/// count and a list of formatted per-dataset (and, since KYO-324, per-table)
/// error strings.
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
/// KYO-324: each `Ok` outcome now also carries `table_errors` — per-table
/// schema-fetch failures collected by `fold_table_outcomes` for datasets
/// whose table *listing* succeeded but whose table *schema reads* were
/// denied. Those are folded into the same `errors` output so a total
/// schema-fetch denial reaches `resolve_final_status` exactly like a total
/// dataset-listing denial does.
///
/// Deliberately free of I/O (`outcomes` are already-resolved `Result`s) so
/// this can be exercised directly by a unit test without an HTTP-mocking
/// dependency — none exists in this workspace, and this fold is the entire
/// piece of decision logic KYO-264/KYO-324 needs proven.
fn fold_dataset_outcomes(
    project_id: &str,
    outcomes: Vec<(String, Result<DatasetOutcome>)>,
) -> (usize, Vec<String>) {
    let mut tables_indexed = 0usize;
    let mut errors = Vec::new();

    for (dataset_id, outcome) in outcomes {
        match outcome {
            Ok(o) => {
                tables_indexed += o.tables_indexed;
                errors.extend(o.table_errors);
            }
            Err(e) => errors.push(format!("{project_id}.{dataset_id}: {e}")),
        }
    }

    (tables_indexed, errors)
}

/// Cap on how many per-table schema-fetch failures a single dataset
/// contributes to the run's `errors` (and the persisted failure reason)
/// before further failures are collapsed into one summary line.
///
/// Every real caller of `index_workspace_catalog` passes
/// `max_tables_per_dataset: None` (verified: `catalog_scheduler.rs:489`,
/// `catalog_scheduler.rs:638`, `indexing_service.rs:355/423`,
/// `sql_editor.rs:760` all pass `None`), so nothing upstream bounds how many
/// tables a single dataset can enumerate. Without this cap, a dataset with
/// thousands of tables under a blanket `bigquery.tables.get` denial would
/// grow both `CatalogIndexResult::errors` and the summarised persisted
/// failure reason to thousands of near-identical lines.
const MAX_TABLE_ERRORS_PER_DATASET: usize = 5;

/// What happened to one table inside `index_dataset_tables`.
enum TableOutcome {
    /// Schema read and `cache_table` wrote the catalog row.
    Indexed,
    /// Schema read, but `cache_table` declined to write the row.
    NotCached,
    /// The table was listed, but its schema could not be read.
    SchemaUnreadable(String),
}

/// The result of indexing every table listed in one BigQuery dataset.
struct DatasetOutcome {
    tables_indexed: usize,
    /// Fully-qualified ids of EVERY table the listing returned — readable or
    /// not. Archiving keys off this set, so a table whose schema fetch was
    /// denied must still appear here or the run would evict a table that
    /// demonstrably still exists (KYO-324).
    seen_table_ids: Vec<String>,
    /// Bounded, formatted schema-fetch failures.
    table_errors: Vec<String>,
}

/// Fold per-table indexing outcomes from one dataset into a
/// [`DatasetOutcome`].
///
/// Mirrors `fold_dataset_outcomes` (KYO-264) one level down: every outcome —
/// whether the schema read succeeded, was declined by `cache_table`, or
/// failed outright — contributes its `full_table_id` to `seen_table_ids`.
/// That is the archiving invariant this ticket (KYO-324) exists to protect:
/// a table whose schema fetch was denied was still *listed*, so it must not
/// be treated as gone. Only `SchemaUnreadable` contributes to
/// `table_errors`, capped at `MAX_TABLE_ERRORS_PER_DATASET` with a trailing
/// summary line for anything beyond the cap.
///
/// Deliberately free of I/O (`outcomes` are already-resolved `TableOutcome`s)
/// so this can be exercised directly by a unit test without an
/// HTTP-mocking dependency — none exists in this workspace. Matches the
/// style and doc-comment depth of `fold_dataset_outcomes` above, its
/// established precedent.
fn fold_table_outcomes(
    dataset_label: &str,
    outcomes: Vec<(String, TableOutcome)>,
) -> DatasetOutcome {
    let mut tables_indexed = 0usize;
    let mut seen_table_ids = Vec::with_capacity(outcomes.len());
    let mut table_errors = Vec::new();
    let mut unreadable_beyond_cap = 0usize;

    for (full_table_id, outcome) in outcomes {
        seen_table_ids.push(full_table_id.clone());

        match outcome {
            TableOutcome::Indexed => tables_indexed += 1,
            TableOutcome::NotCached => {}
            TableOutcome::SchemaUnreadable(msg) => {
                if table_errors.len() < MAX_TABLE_ERRORS_PER_DATASET {
                    table_errors.push(format!("{full_table_id}: {msg}"));
                } else {
                    unreadable_beyond_cap += 1;
                }
            }
        }
    }

    if unreadable_beyond_cap > 0 {
        table_errors.push(format!(
            "{dataset_label}: {unreadable_beyond_cap} further table schema failure{} not shown",
            if unreadable_beyond_cap == 1 { "" } else { "s" }
        ));
    }

    DatasetOutcome {
        tables_indexed,
        seen_table_ids,
        table_errors,
    }
}

/// Index all tables in a single BigQuery dataset.
///
/// Returns a [`DatasetOutcome`] carrying the indexed-table count, every
/// listed table id (readable or not — see `fold_table_outcomes`), and any
/// bounded per-table schema-fetch errors. The `Err` return here is reserved
/// for the table *listing* call itself failing; a per-table schema-fetch
/// failure is captured as `TableOutcome::SchemaUnreadable` and folded into
/// the `Ok` result instead (KYO-324), because the table was still
/// demonstrably listed and must still count toward `seen_table_ids`.
async fn index_dataset_tables(
    params: IndexDatasetParams<'_>,
) -> Result<DatasetOutcome> {
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

    let mut outcomes = Vec::with_capacity(tables.len());

    for table_id in &tables {
        let full_table_id = format!("{project_id}.{dataset_id}.{table_id}");

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
                    outcomes.push((full_table_id, TableOutcome::SchemaUnreadable(format!("{e}"))));
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

        let outcome = if cached {
            TableOutcome::Indexed
        } else {
            TableOutcome::NotCached
        };
        outcomes.push((full_table_id, outcome));
    }

    let dataset_label = format!("{project_id}.{dataset_id}");
    let dataset_outcome = fold_table_outcomes(&dataset_label, outcomes);
    seen_table_ids.extend(dataset_outcome.seen_table_ids.iter().cloned());

    Ok(dataset_outcome)
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

    // ── fold_dataset_outcomes (KYO-264, updated shape for KYO-324) ────────

    /// An `Ok` dataset outcome carrying only a table count, for tests that
    /// exercise `fold_dataset_outcomes`'s roll-up logic and don't care
    /// about individual table ids or per-table errors.
    fn dataset_ok(tables_indexed: usize) -> Result<DatasetOutcome> {
        Ok(DatasetOutcome {
            tables_indexed,
            seen_table_ids: Vec::new(),
            table_errors: Vec::new(),
        })
    }

    fn permission_denied(dataset_id: &str) -> Result<DatasetOutcome> {
        Err(kyomi_core::Error::Internal(format!(
            "BigQuery tables list failed (HTTP 403): permission denied on {dataset_id}"
        )))
    }

    #[test]
    fn all_datasets_ok_sums_counts_with_no_errors() {
        let outcomes = vec![
            ("ds_a".to_string(), dataset_ok(3)),
            ("ds_b".to_string(), dataset_ok(2)),
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
            ("ds_a".to_string(), dataset_ok(4)),
            ("ds_b".to_string(), permission_denied("ds_b")),
        ];
        let (tables_indexed, errors) = fold_dataset_outcomes("proj-1", outcomes);
        assert_eq!(tables_indexed, 4);
        assert_eq!(errors.len(), 1);
        assert!(errors[0].starts_with("proj-1.ds_b: "));
    }

    /// KYO-324: an `Ok` dataset outcome can itself carry `table_errors` when
    /// the dataset's table *listing* succeeded but individual schema reads
    /// were denied. `fold_dataset_outcomes` must surface those into its
    /// `errors` output exactly like a whole-dataset `Err` does, or a total
    /// schema-fetch denial across one dataset would go unnoticed at the
    /// project level.
    #[test]
    fn ok_dataset_with_table_errors_propagates_into_project_errors() {
        let outcomes = vec![(
            "ds_a".to_string(),
            Ok(DatasetOutcome {
                tables_indexed: 0,
                seen_table_ids: vec!["proj-1.ds_a.t1".to_string()],
                table_errors: vec!["proj-1.ds_a.t1: HTTP 403 permission denied".to_string()],
            }),
        )];
        let (tables_indexed, errors) = fold_dataset_outcomes("proj-1", outcomes);
        assert_eq!(tables_indexed, 0);
        assert_eq!(errors, vec!["proj-1.ds_a.t1: HTTP 403 permission denied"]);
    }

    // ── error propagation end-to-end into resolve_run_outcome (KYO-264) ──
    //
    // These tie `fold_dataset_outcomes`'s output to the same
    // `resolve_run_outcome` call `index_workspace_catalog` makes, proving
    // the fix actually changes the persisted status — not just that errors
    // exist in a vec somewhere. Both scenarios here never list a single
    // table (every dataset failed, or every dataset was empty), so
    // `seen_any_table` is `false`.

    #[test]
    fn errored_and_empty_resolves_to_failed_with_non_generic_reason() {
        let outcomes = vec![
            ("ds_a".to_string(), permission_denied("ds_a")),
            ("ds_b".to_string(), permission_denied("ds_b")),
        ];
        let (tables_indexed, errors) = fold_dataset_outcomes("proj-1", outcomes);

        let run_outcome = resolve_run_outcome(false, tables_indexed, &errors);
        assert_eq!(run_outcome.status, "failed");
        let reason = run_outcome
            .failure_reason
            .expect("failed status must carry a reason");
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
            ("ds_a".to_string(), dataset_ok(0)),
            ("ds_b".to_string(), dataset_ok(0)),
        ];
        let (tables_indexed, errors) = fold_dataset_outcomes("proj-1", outcomes);

        let run_outcome = resolve_run_outcome(false, tables_indexed, &errors);
        assert_eq!(
            run_outcome.status, "idle",
            "datasets that were listed fine and are genuinely empty must not report failed"
        );
        assert_eq!(run_outcome.failure_reason, None);
    }

    // ── fold_table_outcomes (KYO-324) ──────────────────────────────────
    //
    // `fold_table_outcomes` is the pure seam this ticket exists to add.
    // `index_dataset_tables` inserted every listed table id into
    // `seen_table_ids` BEFORE fetching its schema, so a schema-fetch denial
    // never wrongly archived the table (see `archive_missing_tables`) — but
    // the denial itself was dropped (`warn!` + `continue`), so a dataset
    // where every schema fetch was denied looked identical to a genuinely
    // empty dataset: `tables_indexed == 0`, `seen_table_ids` non-empty
    // (implying `nothing_found == false`), `errors` empty — which the old
    // single `nothing_found` predicate resolved to `"idle"`.

    fn schema_denied(table: &str) -> TableOutcome {
        TableOutcome::SchemaUnreadable(format!("HTTP 403: permission denied reading {table}"))
    }

    #[test]
    fn all_tables_indexed_counts_and_seen_ids_match() {
        let outcomes = vec![
            ("proj-1.ds_a.t1".to_string(), TableOutcome::Indexed),
            ("proj-1.ds_a.t2".to_string(), TableOutcome::Indexed),
        ];
        let result = fold_table_outcomes("proj-1.ds_a", outcomes);
        assert_eq!(result.tables_indexed, 2);
        assert_eq!(
            result.seen_table_ids,
            vec!["proj-1.ds_a.t1".to_string(), "proj-1.ds_a.t2".to_string()]
        );
        assert!(result.table_errors.is_empty());
    }

    /// AC3 / the trap this ticket calls out explicitly: every table's
    /// schema fetch is denied, but every one of them was still *listed*.
    /// `seen_table_ids` must contain ALL of them — that set is what
    /// `archive_missing_tables` uses to decide what still exists. If this
    /// regresses (a table dropped from `seen_table_ids` because its schema
    /// fetch failed), a total `bigquery.tables.get` denial would archive
    /// the entire existing catalog instead of just failing the run.
    #[test]
    fn all_schema_unreadable_still_populates_seen_table_ids_completely() {
        let table_ids = ["proj-1.ds_a.t1", "proj-1.ds_a.t2", "proj-1.ds_a.t3"];
        let outcomes: Vec<(String, TableOutcome)> = table_ids
            .iter()
            .map(|t| (t.to_string(), schema_denied(t)))
            .collect();

        let result = fold_table_outcomes("proj-1.ds_a", outcomes);

        assert_eq!(result.tables_indexed, 0);
        assert_eq!(
            result.seen_table_ids.len(),
            table_ids.len(),
            "every listed table must appear in seen_table_ids even when its schema fetch failed"
        );
        for t in &table_ids {
            assert!(
                result.seen_table_ids.contains(&t.to_string()),
                "{t} missing from seen_table_ids — archive_missing_tables would wrongly evict it"
            );
        }
        assert_eq!(result.table_errors.len(), table_ids.len());
        for (t, err) in table_ids.iter().zip(result.table_errors.iter()) {
            assert!(
                err.starts_with(&format!("{t}: ")),
                "expected table-id-prefixed error, got: {err}"
            );
        }
    }

    #[test]
    fn mixed_indexed_and_unreadable_counts_and_errors_correctly() {
        let outcomes = vec![
            ("proj-1.ds_a.t1".to_string(), TableOutcome::Indexed),
            ("proj-1.ds_a.t2".to_string(), schema_denied("proj-1.ds_a.t2")),
            ("proj-1.ds_a.t3".to_string(), TableOutcome::NotCached),
        ];
        let result = fold_table_outcomes("proj-1.ds_a", outcomes);

        assert_eq!(result.tables_indexed, 1);
        assert_eq!(result.seen_table_ids.len(), 3, "NotCached must still count as seen");
        assert_eq!(result.table_errors.len(), 1);
        assert!(result.table_errors[0].starts_with("proj-1.ds_a.t2: "));
    }

    /// More `SchemaUnreadable` outcomes than `MAX_TABLE_ERRORS_PER_DATASET`:
    /// the individual error list must be bounded, with one trailing summary
    /// line for the rest — but `seen_table_ids` must remain complete, since
    /// archiving must not be affected by the error cap.
    #[test]
    fn table_errors_are_capped_with_a_summary_line_but_seen_ids_stay_complete() {
        let total = MAX_TABLE_ERRORS_PER_DATASET + 3;
        let table_ids: Vec<String> = (0..total)
            .map(|i| format!("proj-1.ds_a.t{i}"))
            .collect();
        let outcomes: Vec<(String, TableOutcome)> = table_ids
            .iter()
            .map(|t| (t.clone(), schema_denied(t)))
            .collect();

        let result = fold_table_outcomes("proj-1.ds_a", outcomes);

        assert_eq!(result.tables_indexed, 0);
        assert_eq!(
            result.seen_table_ids.len(),
            total,
            "the error cap must not drop any table from seen_table_ids"
        );

        // MAX_TABLE_ERRORS_PER_DATASET individual errors, plus one summary line.
        assert_eq!(result.table_errors.len(), MAX_TABLE_ERRORS_PER_DATASET + 1);
        for i in 0..MAX_TABLE_ERRORS_PER_DATASET {
            assert!(
                result.table_errors[i].starts_with(&format!("proj-1.ds_a.t{i}: ")),
                "expected table {i}'s error to be individually listed, got: {}",
                result.table_errors[i]
            );
        }
        let summary = result.table_errors.last().expect("summary line present");
        assert!(
            summary.starts_with("proj-1.ds_a: "),
            "summary line must be labeled with the dataset, got: {summary}"
        );
        assert!(
            summary.contains(&(total - MAX_TABLE_ERRORS_PER_DATASET).to_string()),
            "summary line must name how many further failures were dropped, got: {summary}"
        );
    }

    // ── end-to-end through resolve_run_outcome (KYO-324 acceptance) ───────
    //
    // These run `fold_table_outcomes`'s output through `resolve_run_outcome`
    // — the exact function `index_workspace_catalog` calls — rather than
    // re-deriving `nothing_listed` / `nothing_usable` / the
    // `resolve_final_status` call inline. That is deliberate: a test that
    // re-derives the production decision instead of calling it still passes
    // if someone reverts `index_workspace_catalog`'s call site back to the
    // pre-fix predicate, which is exactly the KYO-324 regression. Routing
    // through `resolve_run_outcome` means a mutation to that function's
    // predicates is what fails these tests, not a mutation to a copy living
    // in the test body.
    //
    // Four cases, matching `resolve_run_outcome`'s full input matrix:
    //   seen_any_table | tables_indexed | errors     | archive | status | error_message
    //   false          | 0              | empty      | false   | idle   | Some("No tables discovered...")
    //   true            | 0              | non-empty | true    | failed | Some("Tables were listed but none could be indexed...")
    //   true            | >0             | non-empty | true    | idle   | None
    // (seen_any_table=false with non-empty errors is exercised indirectly —
    // it can only arise from a whole-project listing failure, not from
    // `fold_table_outcomes`, so it isn't representable via this fixture.)

    /// AC1: every table listed, every schema fetch denied ⇒ `"failed"` with
    /// a reason naming the underlying error (not the generic message), and
    /// archiving must still run.
    #[test]
    fn all_tables_listed_all_schemas_denied_resolves_to_failed() {
        let table_ids = ["proj-1.ds_a.t1", "proj-1.ds_a.t2"];
        let outcomes: Vec<(String, TableOutcome)> = table_ids
            .iter()
            .map(|t| (t.to_string(), schema_denied(t)))
            .collect();
        let dataset_outcome = fold_table_outcomes("proj-1.ds_a", outcomes);
        let seen_any_table = !dataset_outcome.seen_table_ids.is_empty();

        let run_outcome = resolve_run_outcome(
            seen_any_table,
            dataset_outcome.tables_indexed,
            &dataset_outcome.table_errors,
        );

        assert!(
            run_outcome.archive,
            "tables were listed — archiving must still run"
        );
        assert_eq!(run_outcome.status, "failed");
        assert_eq!(
            run_outcome.error_message,
            Some(
                "Tables were listed but none could be indexed — existing catalog preserved for the tables still present"
            )
        );
        let reason = run_outcome
            .failure_reason
            .expect("failed status must carry a reason");
        assert_ne!(
            reason, "No tables discovered — existing catalog preserved",
            "reason must name the real schema-fetch error, not the generic empty-discovery message"
        );
        assert!(reason.contains("proj-1.ds_a.t1"));
        assert!(reason.contains("permission denied"));
    }

    /// AC2: some tables indexed, some schema fetches failed ⇒ still
    /// `"idle"` (partial success), with no failure reason or caller-facing
    /// error message — the schema-fetch failure itself still lives in
    /// `dataset_outcome.table_errors`, which the real caller folds into
    /// `CatalogIndexResult::errors` separately from `RunOutcome`.
    #[test]
    fn partial_success_resolves_to_idle_with_errors_still_surfaced() {
        let outcomes = vec![
            ("proj-1.ds_a.t1".to_string(), TableOutcome::Indexed),
            ("proj-1.ds_a.t2".to_string(), schema_denied("proj-1.ds_a.t2")),
        ];
        let dataset_outcome = fold_table_outcomes("proj-1.ds_a", outcomes);
        let seen_any_table = !dataset_outcome.seen_table_ids.is_empty();

        let run_outcome = resolve_run_outcome(
            seen_any_table,
            dataset_outcome.tables_indexed,
            &dataset_outcome.table_errors,
        );

        assert!(
            run_outcome.archive,
            "one table indexed successfully — archiving must run"
        );
        assert_eq!(
            run_outcome.status, "idle",
            "partial success must not be reported as failed"
        );
        assert_eq!(run_outcome.failure_reason, None);
        assert_eq!(run_outcome.error_message, None);
        assert_eq!(
            dataset_outcome.table_errors.len(),
            1,
            "the schema-fetch failure must still be present for CatalogIndexResult::errors"
        );
        assert!(dataset_outcome.table_errors[0].starts_with("proj-1.ds_a.t2: "));
    }

    /// AC3 regression, tied directly to `resolve_run_outcome`'s `archive`
    /// field: when every table's schema fetch is denied, `seen_table_ids`
    /// (which `archive_missing_tables` is keyed on) must still contain
    /// every table that was listed, and `resolve_run_outcome` must still
    /// report `archive: true` — so the existing cached catalog is not
    /// archived.
    #[test]
    fn all_schema_denied_does_not_starve_archiving_of_seen_ids() {
        let table_ids = ["proj-1.ds_a.t1", "proj-1.ds_a.t2", "proj-1.ds_a.t3"];
        let outcomes: Vec<(String, TableOutcome)> = table_ids
            .iter()
            .map(|t| (t.to_string(), schema_denied(t)))
            .collect();
        let dataset_outcome = fold_table_outcomes("proj-1.ds_a", outcomes);
        let seen_any_table = !dataset_outcome.seen_table_ids.is_empty();

        let run_outcome = resolve_run_outcome(
            seen_any_table,
            dataset_outcome.tables_indexed,
            &dataset_outcome.table_errors,
        );

        assert!(
            run_outcome.archive,
            "tables were listed, so archiving must run and use the full seen_table_ids set"
        );
        let seen_table_ids: HashSet<String> = dataset_outcome.seen_table_ids.iter().cloned().collect();
        for t in &table_ids {
            assert!(
                seen_table_ids.contains(*t),
                "{t} must be present so archive_missing_tables does not evict it"
            );
        }
    }

    /// The fourth row of `resolve_run_outcome`'s matrix: a genuinely empty
    /// run — nothing was ever listed (e.g. a dataset with zero tables) and
    /// nothing failed outright. Archiving must be skipped (there is no
    /// positive evidence of tables to archive against) and the status must
    /// be `"idle"`, not `"failed"` — an empty-but-accessible dataset is not
    /// an error, even though the caller still gets an informational
    /// `error_message` back so `CatalogIndexResult` reflects that nothing
    /// was indexed this run.
    #[test]
    fn genuinely_empty_run_skips_archiving_and_resolves_to_idle() {
        let dataset_outcome = fold_table_outcomes("proj-1.ds_a", Vec::new());
        let seen_any_table = !dataset_outcome.seen_table_ids.is_empty();

        let run_outcome = resolve_run_outcome(
            seen_any_table,
            dataset_outcome.tables_indexed,
            &dataset_outcome.table_errors,
        );

        assert!(
            !run_outcome.archive,
            "nothing was ever listed — archiving must be skipped"
        );
        assert_eq!(run_outcome.status, "idle");
        assert_eq!(run_outcome.failure_reason, None);
        assert_eq!(
            run_outcome.error_message,
            Some("No tables discovered — existing catalog preserved")
        );
    }
}
