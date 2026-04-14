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
    // Note: if seen_table_ids is empty (no tables found during refresh),
    // ALL existing tables get archived. This matches Python behavior —
    // a datasource with no tables should archive everything.

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

/// Update the workspace's catalog refresh status.
///
/// Sets the `catalog_refresh_status` VARCHAR column on the workspace.
///
/// The column is VARCHAR(50) with values: 'idle', 'running', 'failed'.
/// Progress details are stored in `catalog_refresh_progress` (json column).
pub async fn update_workspace_status(
    db: &DbPool,
    workspace_id: &str,
    datasource_config_id: &str,
    status: &str,
    progress: Option<Value>,
) -> Result<()> {
    let progress_json = serde_json::json!({
        "datasource_config_id": datasource_config_id,
        "updated_at": Utc::now().to_rfc3339(),
        "progress": progress,
    });

    let is_pg = db.is_postgres();
    let json_cast = if is_pg { "::json" } else { "" };
    let sql = format!(
        "UPDATE workspaces SET catalog_refresh_status = $1, catalog_refresh_progress = $2{json_cast} WHERE workspace_id = $3"
    );

    kyomi_core::db_execute!(db, &sql, status, progress_json, workspace_id)
        .map_err(|e| {
            kyomi_core::Error::Internal(format!("failed to update workspace status: {e}"))
        })?;

    Ok(())
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
}
