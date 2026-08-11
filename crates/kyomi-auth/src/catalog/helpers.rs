// SPDX-License-Identifier: AGPL-3.0-or-later

//! Catalog shared helpers — caching, archiving, status updates.
//!
//! These are the `kyomi_datasource`-independent helpers used by both SQL-based
//! indexers and special indexers (sample data, user dataset, BigQuery public).
//!
//! The `SQLCatalogIndexer` trait and `index_catalog_sql()` template method
//! live in `traits.rs` and depend on `kyomi_datasource` (compiled from `kyomi-api`).

use chrono::Utc;
use kyomi_core::{DbPool, Result};
use kyomi_embed::EmbeddingService;
use pgvector::Vector;
use serde_json::Value;
use tracing::{debug, info, warn};

use super::search_entries::{compute_schema_signature, create_search_entries};
use super::types::ColumnEntry;
use crate::embedding_persistence::{
    delete_embeddings_for_table, store_search_embeddings, SearchEntryInsert,
};

// ─── IndexerContext ────────────────────────────────────────────────────────────

/// Shared context passed to all indexing operations.
///
/// Contains the workspace/datasource identifiers, connection configuration,
/// and encryption key needed by shared helper functions.
#[derive(Clone)]
pub struct IndexerContext {
    /// Workspace ID this indexing run belongs to.
    pub workspace_id: String,
    /// Datasource config ID being indexed.
    pub datasource_config_id: String,
    /// Connection configuration from the datasource config row.
    pub connection_config: Value,
    /// Encryption key for decrypting stored credentials.
    pub encryption_key: std::sync::Arc<[u8; 32]>,
}

// ─── Shared helpers ────────────────────────────────────────────────────────────

/// Look up the workspace owner's email address.
///
/// Used by catalog indexing paths that need a "default user" to resolve stored
/// credentials against. The workspace owner is the creator/admin who is most
/// likely to have valid datasource credentials stored.
///
/// Returns `None` if the workspace doesn't exist or the owner can't be resolved.
pub async fn get_workspace_owner_email(db: &DbPool, workspace_id: &str) -> Option<String> {
    #[derive(sqlx::FromRow)]
    struct EmailRow {
        email: String,
    }

    let row = kyomi_core::db_fetch_optional!(
        db,
        EmailRow,
        "SELECT u.email \
         FROM workspaces w \
         JOIN users u ON u.user_id = w.owner_user_id \
         WHERE w.workspace_id = $1",
        workspace_id
    )
    .ok()
    .flatten();

    row.map(|r| r.email)
}

/// Check if a datasource can be refreshed now (respects rate limit).
///
/// Returns `true` if the datasource has never been refreshed, or if more
/// than `hours_threshold` hours have passed since the last refresh.
pub async fn can_refresh_now(
    db: &DbPool,
    datasource_config_id: &str,
    hours_threshold: i64,
) -> bool {
    #[derive(sqlx::FromRow)]
    struct RefreshRow {
        last_catalog_refresh: Option<chrono::DateTime<Utc>>,
    }

    let row = kyomi_core::db_fetch_optional!(
        db,
        RefreshRow,
        "SELECT last_catalog_refresh FROM datasource_configs WHERE id = $1",
        datasource_config_id
    );

    let Ok(Some(row)) = row else {
        return true; // datasource not found or error → allow refresh
    };

    match row.last_catalog_refresh {
        None => true, // never refreshed
        Some(ts) => {
            let elapsed = Utc::now() - ts;
            elapsed.num_hours() >= hours_threshold
        }
    }
}

/// Archive tables that were not seen during the current refresh cycle.
///
/// Marks tables as `is_archived = true` in the cache. Returns the full_name
/// strings of archived tables (format: `project_id.dataset_id.table_id`) so
/// callers can forward them to graph cleanup.
pub async fn archive_missing_tables(
    db: &DbPool,
    workspace_id: &str,
    datasource_config_id: &str,
    seen_table_ids: &std::collections::HashSet<String>,
) -> Result<Vec<String>> {
    // Callers are responsible for only calling this when discovery succeeded.
    // An empty seen_table_ids with a successful discovery means the datasource
    // genuinely has no tables — archiving everything is correct in that case.

    #[derive(sqlx::FromRow)]
    struct CacheRow {
        id: i32,
        project_id: String,
        dataset_id: String,
        table_id: String,
    }

    // Fetch all non-archived tables for this datasource
    let rows = kyomi_core::db_fetch_all!(
        db,
        CacheRow,
        r#"
        SELECT id, project_id, dataset_id, table_id
        FROM datasource_table_cache
        WHERE workspace_id = $1
          AND datasource_config_id = $2
          AND is_archived = false
        "#,
        workspace_id,
        datasource_config_id
    )
    .map_err(|e| kyomi_core::Error::Internal(format!("failed to fetch cached tables: {e}")))?;

    let is_pg = db.is_postgres();
    let now_expr = kyomi_core::sql_compat::now(is_pg);

    let mut archived_names = Vec::new();
    for row in &rows {
        let full_id = kyomi_core::build_full_table_name(&row.project_id, &row.dataset_id, &row.table_id);

        if !seen_table_ids.contains(&full_id) {
            let sql = format!(
                "UPDATE datasource_table_cache SET is_archived = true, updated_at = {now_expr} WHERE id = $1"
            );
            kyomi_core::db_execute!(db, &sql, row.id)
                .map_err(|e| {
                    kyomi_core::Error::Internal(format!("failed to archive table {full_id}: {e}"))
                })?;
            archived_names.push(full_id);
        }
    }

    if !archived_names.is_empty() {
        info!(
            workspace_id,
            datasource_config_id,
            archived_count = archived_names.len(),
            "archived tables no longer present in datasource"
        );
    }

    Ok(archived_names)
}

/// Update a datasource's catalog refresh status.
///
/// Sets the `catalog_refresh_status` VARCHAR column on `datasource_configs`,
/// scoped to `datasource_config_id`.
///
/// The column is VARCHAR(50) with values: 'idle', 'running', 'failed'.
/// Progress details are stored in `catalog_refresh_progress` (json column).
///
/// Filters on `workspace_id` in addition to `datasource_config_id` — that is
/// a tenant-isolation boundary (a caller must not be able to update another
/// workspace's datasource by id alone) and must not be dropped just because
/// `datasource_config_id` is already globally unique.
///
/// `error` is a human-readable failure reason, set whenever `status` is
/// `"failed"` and a specific cause is known (KYO-126). It is written as a
/// top-level `"error"` key in the stored envelope — a sibling of
/// `"progress"`, not nested inside it — because `get_catalog_refresh_status`
/// (`kyomi-ui/src/server_fns/sql_editor.rs`) and the settings page's refresh
/// poller already read `envelope.get("error")` directly on the whole
/// `catalog_refresh_progress` column value. Before this parameter existed,
/// every caller passed `None` here implicitly (there was no such field), so
/// that lookup always missed and the poller fell back to a generic message
/// even when a concrete failure reason was available.
///
/// `warnings` (KYO-327) is the run's collected per-container/per-table
/// discovery errors, written as a top-level `"warnings"` array — always
/// present, even when empty, so readers never need null-handling. Unlike
/// `error`, which only applies to a hard `"failed"` outcome, `warnings` is
/// meaningful on `"idle"` too: `resolve_final_status` folds a partial run
/// (some tables found, some containers denied) down to `"idle"` and
/// discards the individual error strings in that decision — this is the one
/// place they survive to be shown to the user. Callers doing an
/// intermediate/progress write (status `"running"`, or an early exit that
/// isn't the run's resolved final status) pass `&[]`; only the three
/// terminal call sites that already call `resolve_final_status`
/// (`kyomi_agent::catalog::traits::index_catalog_sql`,
/// `kyomi_agent::catalog::indexers::connect::process_discovered_catalog`,
/// `UserDatasetIndexer::index_workspace_catalog`) pass the run's real error
/// slice, on both the `"idle"` and `"failed"` outcomes it can resolve to.
///
/// KYO-267: this was previously `update_workspace_status`, writing to
/// `workspaces.catalog_refresh_status`/`catalog_refresh_progress` — columns
/// shared by every datasource in the workspace. Two different datasources
/// refreshing concurrently (`index_started_within` below keys off
/// `datasource_config_id`, not `workspace_id`, so this was always possible)
/// meant one datasource's successful `"idle"` write could silently
/// overwrite another's `"failed"` + reason, with no history of the failure
/// ever having happened. Moving both columns onto `datasource_configs`
/// removes the shared-state entirely — each datasource now owns its own
/// status/reason pair.
pub async fn update_datasource_status(
    db: &DbPool,
    workspace_id: &str,
    datasource_config_id: &str,
    status: &str,
    progress: Option<Value>,
    error: Option<&str>,
    warnings: &[String],
) -> Result<()> {
    let progress_json =
        build_progress_envelope(datasource_config_id, progress.as_ref(), error, warnings);

    let is_pg = db.is_postgres();
    let json_cast = if is_pg { "::json" } else { "" };
    let sql = format!(
        "UPDATE datasource_configs SET catalog_refresh_status = $1, catalog_refresh_progress = $2{json_cast} WHERE id = $3 AND workspace_id = $4"
    );

    kyomi_core::db_execute!(db, &sql, status, progress_json, datasource_config_id, workspace_id)
        .map_err(|e| {
            kyomi_core::Error::Internal(format!("failed to update datasource status: {e}"))
        })?;

    Ok(())
}

/// Build the JSON envelope stored in `datasource_configs.catalog_refresh_progress`.
///
/// Extracted from [`update_datasource_status`] so the shape — in particular,
/// `"error"` living as a top-level sibling of `"progress"` rather than
/// nested inside it — is directly testable. This is the exact shape
/// `get_catalog_refresh_status` and the settings page's refresh poller read
/// (KYO-126): both call `envelope.get("error")` on the whole column value.
///
/// The `"datasource_config_id"` key is redundant now that the envelope
/// lives on the datasource's own row (KYO-267) rather than a shared
/// workspace column — a reader no longer needs it to know which datasource
/// a status belongs to. It is kept purely as informational/debugging
/// context (costs nothing, and removing it risked breaking a reader that
/// wasn't checked), not because anything still depends on it for
/// attribution.
///
/// `"warnings"` (KYO-327) is always a JSON array, never null — a top-level
/// sibling of `"progress"`/`"error"`, same reasoning as `"error"` above.
/// Readers must never parse `"error"`'s collapsed `"<first> (+N more
/// errors)"` string for this; that string's wording is deliberately opaque
/// (see `CatalogIndexResult::errors`'s doc comment) and exists only for a
/// human reading the hard-failure reason, not for driving UI.
fn build_progress_envelope(
    datasource_config_id: &str,
    progress: Option<&Value>,
    error: Option<&str>,
    warnings: &[String],
) -> Value {
    serde_json::json!({
        "datasource_config_id": datasource_config_id,
        "updated_at": Utc::now().to_rfc3339(),
        "progress": progress,
        "error": error,
        "warnings": warnings,
    })
}

/// Determine the final `datasource_configs.catalog_refresh_status` value and
/// (when applicable) a failure reason, from the outcome of a catalog
/// indexing run.
///
/// Shared by both the SQL-based indexing template method
/// (`kyomi_agent::catalog::traits::index_catalog_sql`) and the BigQuery
/// user-dataset (REST) indexer (`kyomi_auth::catalog::indexers::user_dataset`)
/// — both fold their own container/dataset-shaped discovery loop down to the
/// same two inputs before calling this.
///
/// KYO-126: before this function existed, `index_catalog_sql` unconditionally
/// wrote `"idle"` at the end of every run — including one where every
/// container's discovery query failed (e.g. the role lacks permission to
/// read the catalog) and zero tables were found. That made a total discovery
/// failure indistinguishable from a healthy, empty datasource. KYO-264 found
/// and fixed the same bug on the BigQuery REST path (`UserDatasetIndexer`),
/// which had the identical unconditional-`"idle"` write plus a second layer
/// of the bug: per-dataset failures were only `warn!`-logged and dropped
/// before ever reaching an `errors` vec, so simply routing
/// `nothing_found`/`errors` through this function was not enough on its own
/// — the per-dataset errors first had to be propagated up to the caller (see
/// `fold_dataset_outcomes` in `catalog/indexers/user_dataset.rs`).
///
/// This function draws the line at whether any discovery error was observed:
/// - `nothing_found` + at least one error → the zero tables are *caused by*
///   a real failure: report `"failed"` with a reason built from the
///   collected errors.
/// - `nothing_found` with no errors → every container/dataset query
///   genuinely succeeded and simply returned no tables (or the user
///   configured zero containers) — this is not a failure and must keep
///   reporting `"idle"`, or a legitimately empty-but-accessible schema would
///   wrongly show as a broken datasource.
/// - not `nothing_found` → normal completion, `"idle"`, regardless of
///   whether some individual tables/containers/datasets errored along the
///   way (partial success is still success; those errors are already
///   surfaced via `CatalogIndexResult::errors`).
pub fn resolve_final_status(nothing_found: bool, errors: &[String]) -> (&'static str, Option<String>) {
    if !nothing_found || errors.is_empty() {
        return ("idle", None);
    }

    let reason = match errors.len() {
        1 => errors[0].clone(),
        n => format!("{} (+{} more error{})", errors[0], n - 1, if n == 2 { "" } else { "s" }),
    };
    ("failed", Some(reason))
}

/// Update the datasource's last_catalog_refresh timestamp.
pub async fn update_datasource_last_refresh(
    db: &DbPool,
    datasource_config_id: &str,
) -> Result<()> {
    let is_pg = db.is_postgres();
    let now_expr = kyomi_core::sql_compat::now(is_pg);
    let sql = format!(
        "UPDATE datasource_configs SET last_catalog_refresh = {now_expr} WHERE id = $1"
    );

    kyomi_core::db_execute!(db, &sql, datasource_config_id)
        .map_err(|e| {
            kyomi_core::Error::Internal(format!(
                "failed to update datasource last_catalog_refresh: {e}"
            ))
        })?;

    Ok(())
}

/// Stamp the datasource's `last_index_started_at` column with `now()`.
///
/// Called at the top of [`CatalogIndexingService::index_datasource`] so that
/// any concurrent caller (scheduler, post-create spawn, manual refresh)
/// can observe that an indexing run is in flight and skip.
///
/// [`CatalogIndexingService::index_datasource`]: (see crate `kyomi-agent`)
pub async fn stamp_last_index_started_at(
    db: &DbPool,
    datasource_config_id: &str,
) -> Result<()> {
    let is_pg = db.is_postgres();
    let now_expr = kyomi_core::sql_compat::now(is_pg);
    let sql = format!(
        "UPDATE datasource_configs SET last_index_started_at = {now_expr} WHERE id = $1"
    );

    kyomi_core::db_execute!(db, &sql, datasource_config_id).map_err(|e| {
        kyomi_core::Error::Internal(format!(
            "failed to stamp datasource last_index_started_at: {e}"
        ))
    })?;

    Ok(())
}

/// Returns `true` if an indexing run for this datasource started within
/// the last `minutes_threshold` minutes.
///
/// Reads `datasource_configs.last_index_started_at`. Returns `false` if:
/// - the column is NULL (never indexed), OR
/// - the stamp is older than the threshold (self-healing — a panicked run's
///   stamp ages out and the next attempt proceeds), OR
/// - the row can't be found or the query errors (fail open; the downstream
///   indexer will produce a clearer error if the datasource is genuinely
///   missing).
///
/// Use in combination with `can_refresh_now` (which guards against
/// "just finished, don't re-index" via `last_catalog_refresh`). This guard
/// protects against "just started, don't double up".
pub async fn index_started_within(
    db: &DbPool,
    datasource_config_id: &str,
    minutes_threshold: i64,
) -> bool {
    #[derive(sqlx::FromRow)]
    struct StartedRow {
        last_index_started_at: Option<chrono::DateTime<Utc>>,
    }

    let row = kyomi_core::db_fetch_optional!(
        db,
        StartedRow,
        "SELECT last_index_started_at FROM datasource_configs WHERE id = $1",
        datasource_config_id
    );

    let Ok(Some(row)) = row else {
        return false; // not found or error → allow caller to proceed
    };

    match row.last_index_started_at {
        None => false,
        Some(ts) => (Utc::now() - ts).num_minutes() < minutes_threshold,
    }
}

/// Cache a table and generate embeddings for its search entries.
///
/// This is the core caching + embedding function used by all indexers.
///
/// Flow:
/// 1. Build table_metadata JSON from columns
/// 2. Check if table exists in cache
/// 3. If exists AND schema unchanged AND embeddings exist → skip (update last_verified)
/// 4. Otherwise → upsert cache entry, delete old embeddings, generate new ones
///
/// Returns `true` if the table was cached/updated, `false` on error.
/// Parameters for [`cache_table`].
pub struct CacheTableParams<'a> {
    pub db: &'a DbPool,
    pub embedding: &'a EmbeddingService,
    pub ctx: &'a IndexerContext,
    pub project_id: &'a str,
    pub dataset_id: &'a str,
    pub table_name: &'a str,
    pub table_type: &'a str,
    pub columns: &'a [ColumnEntry],
    pub full_table_id: &'a str,
}

pub async fn cache_table(params: CacheTableParams<'_>) -> bool {
    let CacheTableParams {
        db,
        embedding,
        ctx,
        project_id,
        dataset_id,
        table_name,
        table_type,
        columns,
        full_table_id,
    } = params;
    // Build table_metadata JSON
    let columns_json: Vec<Value> = columns
        .iter()
        .map(|c| {
            serde_json::json!({
                "name": c.name,
                "type": c.col_type.as_deref().unwrap_or("unknown"),
                "native_type": c.native_type.as_deref().unwrap_or(""),
                "description": c.description.as_deref().unwrap_or(""),
            })
        })
        .collect();

    let table_metadata = serde_json::json!({
        "table_name": table_name,
        "dataset_id": dataset_id,
        "project_id": project_id,
        "table_type": table_type,
        "columns": columns_json,
    });

    // Check if table already exists in cache
    #[derive(sqlx::FromRow)]
    struct ExistingRow {
        id: i32,
        table_metadata: serde_json::Value,
    }

    let existing = kyomi_core::db_fetch_optional!(
        db,
        ExistingRow,
        r#"
        SELECT id, table_metadata
        FROM datasource_table_cache
        WHERE workspace_id = $1
          AND datasource_config_id = $2
          AND project_id = $3
          AND dataset_id = $4
          AND table_id = $5
        "#,
        &ctx.workspace_id,
        &ctx.datasource_config_id,
        project_id,
        dataset_id,
        table_name
    );

    let existing = match existing {
        Ok(row) => row,
        Err(e) => {
            warn!(
                table = full_table_id,
                error = %e,
                "failed to check existing cache entry"
            );
            return false;
        }
    };

    let is_pg = db.is_postgres();
    let now_expr = kyomi_core::sql_compat::now(is_pg);
    let false_val = kyomi_core::sql_compat::bool_false(is_pg);

    if let Some(ref row) = existing {
        let cache_id = row.id;
        let stored_metadata = &row.table_metadata;

        // Compare schema signatures
        let current_sig = compute_schema_signature(columns);
        let stored_sig = extract_schema_signature(stored_metadata);

        if current_sig == stored_sig {
            // Schema unchanged — check if embeddings exist
            let embedding_count: i64 = kyomi_core::db_fetch_scalar!(
                db,
                i64,
                "SELECT COUNT(*) FROM datasource_search_embeddings WHERE table_cache_id = $1",
                cache_id
            )
            .unwrap_or(0);

            if embedding_count > 0 {
                // Schema unchanged AND embeddings exist → just update last_verified
                let sql = format!(
                    "UPDATE datasource_table_cache SET last_verified = {now_expr}, is_archived = {false_val} WHERE id = $1"
                );
                let _ = kyomi_core::db_execute!(db, &sql, cache_id);

                debug!(table = full_table_id, "schema unchanged, skipping re-index");
                return true;
            }
        }

        // Schema changed OR no embeddings → update cache entry and re-embed
        let sql = format!(
            r#"
            UPDATE datasource_table_cache
            SET table_metadata = $1, structure_refreshed_at = {now_expr},
                updated_at = {now_expr}, last_verified = {now_expr}, is_archived = {false_val}
            WHERE id = $2
            "#
        );
        let update_result = kyomi_core::db_execute!(db, &sql, &table_metadata, cache_id);

        if let Err(e) = update_result {
            warn!(table = full_table_id, error = %e, "failed to update cache entry");
            return false;
        }

        // Delete old embeddings
        if let Err(e) = delete_embeddings_for_table(db, cache_id).await {
            warn!(table = full_table_id, error = %e, "failed to delete old embeddings");
        }

        // Generate and store new embeddings
        return generate_and_store_embeddings(GenerateEmbeddingsParams {
            db,
            embedding,
            workspace_id: &ctx.workspace_id,
            datasource_config_id: &ctx.datasource_config_id,
            project_id,
            dataset_id,
            table_name,
            columns,
            cache_id,
        })
        .await;
    }

    // Table doesn't exist in cache → insert
    let sql = format!(
        r#"
        INSERT INTO datasource_table_cache
            (workspace_id, datasource_config_id, project_id, dataset_id, table_id,
             table_metadata, is_archived, structure_refreshed_at, last_verified)
        VALUES ($1, $2, $3, $4, $5, $6, {false_val}, {now_expr}, {now_expr})
        RETURNING id
        "#
    );

    #[derive(sqlx::FromRow)]
    struct IdRow {
        id: i32,
    }

    let insert_result = kyomi_core::db_fetch_one!(
        db,
        IdRow,
        &sql,
        &ctx.workspace_id,
        &ctx.datasource_config_id,
        project_id,
        dataset_id,
        table_name,
        &table_metadata
    );

    let cache_id = match insert_result {
        Ok(row) => row.id,
        Err(e) => {
            warn!(table = full_table_id, error = %e, "failed to insert cache entry");
            return false;
        }
    };

    generate_and_store_embeddings(GenerateEmbeddingsParams {
        db,
        embedding,
        workspace_id: &ctx.workspace_id,
        datasource_config_id: &ctx.datasource_config_id,
        project_id,
        dataset_id,
        table_name,
        columns,
        cache_id,
    })
    .await
}

/// Extract a schema signature from stored `table_metadata` JSON.
///
/// Parses the `columns` array and produces a sorted signature matching
/// the format from [`compute_schema_signature`].
fn extract_schema_signature(table_metadata: &Value) -> Vec<(String, String, String)> {
    let Some(columns) = table_metadata.get("columns").and_then(|c| c.as_array()) else {
        return Vec::new();
    };

    let mut sig: Vec<(String, String, String)> = columns
        .iter()
        .map(|c| {
            (
                c.get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                c.get("type")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                c.get("description")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
            )
        })
        .collect();
    sig.sort();
    sig
}

struct GenerateEmbeddingsParams<'a> {
    db: &'a DbPool,
    embedding: &'a EmbeddingService,
    workspace_id: &'a str,
    datasource_config_id: &'a str,
    project_id: &'a str,
    dataset_id: &'a str,
    table_name: &'a str,
    columns: &'a [ColumnEntry],
    cache_id: i32,
}

/// Generate search entries, compute embeddings, and store them.
async fn generate_and_store_embeddings(params: GenerateEmbeddingsParams<'_>) -> bool {
    let GenerateEmbeddingsParams {
        db,
        embedding,
        workspace_id,
        datasource_config_id,
        project_id,
        dataset_id,
        table_name,
        columns,
        cache_id,
    } = params;
    let entries = create_search_entries(dataset_id, table_name, table_name, columns);

    if entries.is_empty() {
        return true;
    }

    // Collect texts for batch embedding
    let texts: Vec<&str> = entries.iter().map(|e| e.text.as_str()).collect();

    let vectors = match embedding.embed_passages(&texts) {
        Ok(vecs) => vecs,
        Err(e) => {
            warn!(
                table = %format!("{dataset_id}.{table_name}"),
                error = %e,
                "failed to compute embeddings"
            );
            return false;
        }
    };

    if vectors.len() != entries.len() {
        warn!(
            table = %format!("{dataset_id}.{table_name}"),
            expected = entries.len(),
            got = vectors.len(),
            "embedding count mismatch"
        );
        return false;
    }

    // Build insertion records
    let inserts: Vec<SearchEntryInsert> = entries
        .iter()
        .zip(vectors.iter())
        .map(|(entry, vec)| SearchEntryInsert {
            table_cache_id: cache_id,
            workspace_id: workspace_id.to_string(),
            datasource_config_id: Some(datasource_config_id.to_string()),
            project_id: project_id.to_string(),
            dataset_id: dataset_id.to_string(),
            table_id: table_name.to_string(),
            entry_type: entry.entry_type.clone(),
            text: entry.text.clone(),
            weight: entry.weight,
            column_name: entry.column_name.clone(),
            embedding: Vector::from(vec.clone()),
        })
        .collect();

    if let Err(e) = store_search_embeddings(db, &inserts).await {
        warn!(
            table = %format!("{dataset_id}.{table_name}"),
            error = %e,
            "failed to store embeddings"
        );
        return false;
    }

    true
}

// ─── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── resolve_final_status (KYO-126) ───────────────────────────────────

    #[test]
    fn errored_and_empty_reports_failed_with_reason() {
        let errors = vec!["Failed to list tables in schema 'public': permission denied".to_string()];
        let (status, reason) = resolve_final_status(true, &errors);
        assert_eq!(status, "failed");
        assert_eq!(
            reason,
            Some("Failed to list tables in schema 'public': permission denied".to_string())
        );
    }

    #[test]
    fn errored_and_empty_with_multiple_errors_reports_count() {
        let errors = vec![
            "Failed to list tables in schema 'a': permission denied".to_string(),
            "Failed to list tables in schema 'b': permission denied".to_string(),
        ];
        let (status, reason) = resolve_final_status(true, &errors);
        assert_eq!(status, "failed");
        assert_eq!(
            reason,
            Some(
                "Failed to list tables in schema 'a': permission denied (+1 more error)"
                    .to_string()
            )
        );
    }

    #[test]
    fn empty_without_errors_reports_idle() {
        // Regression guard (KYO-126): an accessible datasource that
        // genuinely has zero tables (or where the user configured zero
        // containers) must not be reported as failed.
        let (status, reason) = resolve_final_status(true, &[]);
        assert_eq!(status, "idle");
        assert_eq!(reason, None);
    }

    #[test]
    fn not_nothing_found_reports_idle_even_with_partial_errors() {
        // A normal completion where some individual tables/containers
        // errored but at least one table was still indexed is a partial
        // success, not a failure — those errors are already surfaced via
        // `CatalogIndexResult::errors`.
        let errors = vec!["Failed to get columns for public.weird_table: timeout".to_string()];
        let (status, reason) = resolve_final_status(false, &errors);
        assert_eq!(status, "idle");
        assert_eq!(reason, None);
    }

    // ── build_progress_envelope (KYO-126) ────────────────────────────────

    #[test]
    fn envelope_puts_error_as_top_level_sibling_of_progress() {
        // This is the exact shape `get_catalog_refresh_status`
        // (kyomi-ui/src/server_fns/sql_editor.rs) and the settings page's
        // refresh poller read: `envelope.get("error")` directly on the
        // whole `catalog_refresh_progress` column value, not nested under
        // `envelope["progress"]["error"]`. Before this field existed, no
        // caller ever populated an "error" key at all, so that lookup
        // always missed regardless of what the caller passed as `progress`.
        let envelope = build_progress_envelope(
            "ds-1",
            Some(&serde_json::json!({"processed": 3})),
            Some("permission denied for schema analytics"),
            &[],
        );

        assert_eq!(
            envelope.get("error").and_then(|v| v.as_str()),
            Some("permission denied for schema analytics")
        );
        assert_eq!(
            envelope.get("progress"),
            Some(&serde_json::json!({"processed": 3}))
        );
        assert_eq!(
            envelope.get("datasource_config_id").and_then(|v| v.as_str()),
            Some("ds-1")
        );
    }

    #[test]
    fn envelope_error_is_null_when_none() {
        let envelope = build_progress_envelope("ds-1", None, None, &[]);
        assert!(envelope.get("error").is_some_and(|v| v.is_null()));
    }

    // ── build_progress_envelope warnings (KYO-327) ───────────────────────

    #[test]
    fn envelope_puts_warnings_as_top_level_sibling_of_progress_and_error() {
        // Same shape requirement as `envelope_puts_error_as_top_level_sibling_of_progress`
        // above, extended to the new `"warnings"` key: `get_catalog_stats`
        // (kyomi-ui/src/server_fns/datasources.rs) reads
        // `envelope.get("warnings")` directly on the whole
        // `catalog_refresh_progress` column value, not nested under
        // `envelope["progress"]["warnings"]`.
        let warnings =
            vec!["Failed to list tables in schema 'restricted': permission denied".to_string()];
        let envelope = build_progress_envelope(
            "ds-1",
            Some(&serde_json::json!({"processed": 3})),
            None,
            &warnings,
        );

        assert_eq!(
            envelope.get("warnings"),
            Some(&serde_json::json!([
                "Failed to list tables in schema 'restricted': permission denied"
            ]))
        );
        assert_eq!(
            envelope.get("progress"),
            Some(&serde_json::json!({"processed": 3}))
        );
    }

    #[test]
    fn envelope_warnings_is_empty_array_not_null_when_none() {
        // Unlike `"error"`, which is `null` when absent, `"warnings"` must
        // always be a present, empty JSON array — readers should never need
        // null-handling to distinguish "no warnings" from "field missing".
        let envelope = build_progress_envelope("ds-1", None, None, &[]);
        assert_eq!(envelope.get("warnings"), Some(&serde_json::json!([])));
        assert!(
            !envelope.get("warnings").is_some_and(|v| v.is_null()),
            "warnings must be an empty array, not null, when the run had no warnings"
        );
    }

    #[test]
    fn extract_schema_signature_from_metadata() {
        let metadata = serde_json::json!({
            "table_name": "users",
            "columns": [
                {"name": "id", "type": "number", "native_type": "INT", "description": ""},
                {"name": "name", "type": "string", "native_type": "VARCHAR", "description": "User name"},
            ]
        });

        let sig = extract_schema_signature(&metadata);
        assert_eq!(sig.len(), 2);
        // Should be sorted by name
        assert_eq!(sig[0].0, "id");
        assert_eq!(sig[1].0, "name");
        assert_eq!(sig[1].2, "User name");
    }

    #[test]
    fn extract_schema_signature_empty_columns() {
        let metadata = serde_json::json!({"table_name": "empty"});
        let sig = extract_schema_signature(&metadata);
        assert!(sig.is_empty());
    }

    #[test]
    fn extract_schema_signature_matches_compute() {
        let columns = vec![
            ColumnEntry {
                name: "a".into(),
                col_type: Some("string".into()),
                native_type: Some("VARCHAR".into()),
                description: Some("desc a".into()),
            },
            ColumnEntry {
                name: "b".into(),
                col_type: Some("number".into()),
                native_type: Some("INT".into()),
                description: None,
            },
        ];

        let computed = compute_schema_signature(&columns);

        // Build the equivalent metadata JSON
        let metadata = serde_json::json!({
            "columns": [
                {"name": "a", "type": "string", "native_type": "VARCHAR", "description": "desc a"},
                {"name": "b", "type": "number", "native_type": "INT", "description": ""},
            ]
        });
        let extracted = extract_schema_signature(&metadata);

        assert_eq!(computed, extracted);
    }

    // ── update_datasource_status concurrency (KYO-267) ───────────────────

    /// Seeds one workspace with two datasource rows, `datasource_config_id`s
    /// `"ds-A-{suffix}"` and `"ds-B-{suffix}"`. Parameterized by suffix so
    /// the two tests below don't collide on primary keys.
    async fn seed_two_datasource_fixture(sq: &sqlx::SqlitePool, suffix: &str) -> (String, String, String) {
        let user_id = format!("u-concurrency-{suffix}");
        let workspace_id = format!("ws-concurrency-{suffix}");
        let ds_a = format!("ds-A-{suffix}");
        let ds_b = format!("ds-B-{suffix}");

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
        for (id, slug) in [(&ds_a, "a"), (&ds_b, "b")] {
            sqlx::query(
                "INSERT INTO datasource_configs (id, workspace_id, name, datasource_type, slug) \
                 VALUES (?, ?, ?, 'postgres', ?)",
            )
            .bind(id)
            .bind(&workspace_id)
            .bind(format!("DS-{slug}-{suffix}"))
            .bind(format!("{slug}-{suffix}"))
            .execute(sq)
            .await
            .expect("insert datasource_config");
        }

        (workspace_id, ds_a, ds_b)
    }

    /// Reads back `(catalog_refresh_status, error reason)` for a single
    /// datasource, extracting the reason the same way
    /// `get_catalog_refresh_status`/`get_catalog_stats` do: the top-level
    /// `"error"` key of the stored `catalog_refresh_progress` envelope.
    async fn read_datasource_status(sq: &sqlx::SqlitePool, datasource_config_id: &str) -> (String, Option<String>) {
        #[derive(sqlx::FromRow)]
        struct Row {
            catalog_refresh_status: Option<String>,
            catalog_refresh_progress: Option<String>,
        }

        let row: Row = sqlx::query_as(
            "SELECT catalog_refresh_status, catalog_refresh_progress FROM datasource_configs WHERE id = ?",
        )
        .bind(datasource_config_id)
        .fetch_one(sq)
        .await
        .expect("read datasource status");

        let reason = row
            .catalog_refresh_progress
            .as_deref()
            .and_then(|p| serde_json::from_str::<Value>(p).ok())
            .and_then(|v| v.get("error").and_then(|e| e.as_str()).map(str::to_string));

        (row.catalog_refresh_status.unwrap_or_else(|| "idle".to_string()), reason)
    }

    /// This is the whole point of KYO-267. Before this fix,
    /// `catalog_refresh_status`/`catalog_refresh_progress` lived on
    /// `workspaces` and were shared by every datasource in it — datasource
    /// A failing, then datasource B finishing successfully, meant B's
    /// `"idle"` write silently clobbered A's `"failed"` + reason with no
    /// history of the failure ever having existed (the KYO-126 bug
    /// reintroduced by a different mechanism). Each datasource now owns its
    /// own status/reason pair, so A's failure must survive B's unrelated
    /// success regardless of write order.
    #[tokio::test]
    async fn concurrent_datasources_retain_independent_terminal_status() {
        let db = DbPool::connect("sqlite::memory:")
            .await
            .expect("connect in-memory sqlite");
        let DbPool::Sqlite(sq) = &db else {
            unreachable!("expected sqlite pool");
        };
        let (workspace_id, ds_a, ds_b) = seed_two_datasource_fixture(sq, "order1").await;

        // A fails first...
        update_datasource_status(&db, &workspace_id, &ds_a, "failed", None, Some("permission denied for schema analytics"), &[])
            .await
            .expect("write A failed");
        // ...then B finishes successfully.
        update_datasource_status(&db, &workspace_id, &ds_b, "idle", None, None, &[])
            .await
            .expect("write B idle");

        let (status_a, reason_a) = read_datasource_status(sq, &ds_a).await;
        let (status_b, reason_b) = read_datasource_status(sq, &ds_b).await;

        assert_eq!(status_a, "failed", "B's success must not clobber A's failure");
        assert_eq!(reason_a, Some("permission denied for schema analytics".to_string()));
        assert_eq!(status_b, "idle");
        assert_eq!(reason_b, None);
    }

    /// Companion to the test above with the write order reversed: B
    /// succeeds first, then A fails. A shared-column implementation would
    /// report whichever wrote last (A's `"failed"`) for *both* datasources
    /// here; per-datasource columns must still show each its own outcome
    /// regardless of ordering.
    #[tokio::test]
    async fn concurrent_datasources_retain_independent_terminal_status_reverse_order() {
        let db = DbPool::connect("sqlite::memory:")
            .await
            .expect("connect in-memory sqlite");
        let DbPool::Sqlite(sq) = &db else {
            unreachable!("expected sqlite pool");
        };
        let (workspace_id, ds_a, ds_b) = seed_two_datasource_fixture(sq, "order2").await;

        // B succeeds first...
        update_datasource_status(&db, &workspace_id, &ds_b, "idle", None, None, &[])
            .await
            .expect("write B idle");
        // ...then A fails.
        update_datasource_status(&db, &workspace_id, &ds_a, "failed", None, Some("connection timed out"), &[])
            .await
            .expect("write A failed");

        let (status_a, reason_a) = read_datasource_status(sq, &ds_a).await;
        let (status_b, reason_b) = read_datasource_status(sq, &ds_b).await;

        assert_eq!(status_a, "failed");
        assert_eq!(reason_a, Some("connection timed out".to_string()));
        assert_eq!(status_b, "idle", "A's later failure must not clobber B's earlier success");
        assert_eq!(reason_b, None);
    }
}
