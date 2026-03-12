// SPDX-License-Identifier: AGPL-3.0-or-later

//! Learning reference materialization -- resolve and store entity references.
//!
//! Replaces FalkorDB edges (MENTIONS_TABLE, MENTIONS_COLUMN, MENTIONS_METRIC)
//! with rows in the `learning_references` table. Each row represents a
//! relationship between a learning and a table, column, or metric.

use crate::sql_references;
use kyomi_core::db::DbPool;
use serde_json::Value as JsonValue;
use std::collections::HashSet;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Single-learning materialization
// ---------------------------------------------------------------------------

#[derive(sqlx::FromRow)]
struct LearningRefRow {
    reference_queries: Option<String>,
    structured_metadata: Option<String>,
}

#[derive(sqlx::FromRow)]
struct CachedTable {
    project_id: String,
    dataset_id: String,
    table_id: String,
}

/// Materialize all entity references for a single learning.
///
/// Extracts table, column, and metric references from the learning's
/// `reference_queries` (via SQL parsing) and `structured_metadata` (via
/// explicit JSON fields), then resolves them against the workspace's
/// `datasource_table_cache` to produce canonical names.
///
/// Replaces existing references for this learning (DELETE + INSERT).
///
/// If `preloaded_table_names` is provided, uses those instead of querying
/// the database for workspace tables. This avoids N+1 queries when called
/// in a loop (e.g., from `backfill_all_references`).
pub async fn materialize_learning_references(
    db: &DbPool,
    learning_id: &str,
    workspace_id: &str,
    preloaded_table_names: Option<&[String]>,
) -> anyhow::Result<()> {
    let parsed_id = Uuid::parse_str(learning_id)?;

    let row = kyomi_core::db_fetch_optional!(
        db,
        LearningRefRow,
        "SELECT CAST(reference_queries AS TEXT) as reference_queries, \
         CAST(structured_metadata AS TEXT) as structured_metadata \
         FROM agent_learnings \
         WHERE learning_id = $1",
        &parsed_id
    )?;

    let row = match row {
        Some(r) => r,
        None => {
            tracing::warn!(learning_id, "Learning not found for reference materialization");
            return Ok(());
        }
    };

    // Parse JSON from text columns
    let ref_queries_json: Option<JsonValue> = row
        .reference_queries
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok());
    let structured_meta_json: Option<JsonValue> = row
        .structured_metadata
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok());

    // Collect all raw table names from SQL parsing and structured metadata
    let mut raw_table_names: HashSet<String> = HashSet::new();
    let mut column_refs: Vec<String> = Vec::new();
    let mut metric_refs: Vec<String> = Vec::new();

    // Parse reference_queries SQL to extract table references
    if let Some(ref ref_queries) = ref_queries_json {
        if let Some(queries) = ref_queries.as_array() {
            for query_val in queries {
                if let Some(sql) = query_val.as_str() {
                    let refs = sql_references::extract_references(sql);
                    for table_name in refs.tables {
                        raw_table_names.insert(table_name);
                    }
                }
            }
        }
    }

    // Extract from structured_metadata
    if let Some(ref meta) = structured_meta_json {
        if let Some(tables) = meta.get("related_tables").and_then(|v| v.as_array()) {
            for table_val in tables {
                if let Some(table_name) = table_val.as_str() {
                    raw_table_names.insert(table_name.to_string());
                }
            }
        }
        if let Some(columns) = meta.get("related_columns").and_then(|v| v.as_array()) {
            for col_val in columns {
                if let Some(col_ref) = col_val.as_str() {
                    column_refs.push(col_ref.to_string());
                }
            }
        }
        if let Some(metrics) = meta.get("related_metrics").and_then(|v| v.as_array()) {
            for metric_val in metrics {
                if let Some(metric_name) = metric_val.as_str() {
                    metric_refs.push(metric_name.to_string());
                }
            }
        }
    }

    // Use preloaded table names if provided, otherwise fetch from database
    let owned_names: Vec<String>;
    let known_full_names: &[String] = match preloaded_table_names {
        Some(names) => names,
        None => {
            let cached_tables = kyomi_core::db_fetch_all!(
                db,
                CachedTable,
                "SELECT project_id, dataset_id, table_id \
                 FROM datasource_table_cache \
                 WHERE workspace_id = $1 AND is_archived = false",
                &workspace_id
            )?;

            owned_names = cached_tables
                .iter()
                .map(|t| kyomi_core::build_full_table_name(&t.project_id, &t.dataset_id, &t.table_id))
                .collect();
            &owned_names
        }
    };

    // Resolve raw table names to canonical full names using fuzzy matching
    let mut resolved_refs: HashSet<(String, String)> = HashSet::new(); // (ref_type, ref_name)

    for raw_name in &raw_table_names {
        if let Some(full_name) = resolve_table_name(raw_name, known_full_names) {
            resolved_refs.insert(("table".to_string(), full_name));
        }
    }

    // Resolve column references (format: "table_name.column_name")
    for col_ref in &column_refs {
        if let Some(last_dot) = col_ref.rfind('.') {
            let table_part = &col_ref[..last_dot];
            let col_name = &col_ref[last_dot + 1..];
            if !col_name.is_empty() {
                if let Some(full_table) = resolve_table_name(table_part, known_full_names) {
                    // Format: "full_table_name#column_name"
                    resolved_refs
                        .insert(("column".to_string(), format!("{full_table}#{col_name}")));
                }
            }
        }
    }

    // Metric references are stored by name directly
    for metric_name in &metric_refs {
        resolved_refs.insert(("metric".to_string(), metric_name.clone()));
    }

    // Delete existing refs and insert new ones in a transaction
    // Transactions require match blocks since DbPool doesn't have begin()
    match db {
        DbPool::Postgres(pg) => {
            let mut tx = pg.begin().await?;

            sqlx::query("DELETE FROM learning_references WHERE learning_id = $1")
                .bind(parsed_id)
                .execute(&mut *tx)
                .await?;

            for (ref_type, ref_name) in &resolved_refs {
                sqlx::query(
                    "INSERT INTO learning_references (learning_id, workspace_id, ref_type, ref_name) \
                     VALUES ($1, $2, $3, $4) \
                     ON CONFLICT (learning_id, ref_type, ref_name) DO NOTHING",
                )
                .bind(parsed_id)
                .bind(workspace_id)
                .bind(ref_type)
                .bind(ref_name)
                .execute(&mut *tx)
                .await?;
            }

            tx.commit().await?;
        }
        DbPool::Sqlite(sq) => {
            let mut tx = sq.begin().await?;

            sqlx::query("DELETE FROM learning_references WHERE learning_id = $1")
                .bind(parsed_id.to_string())
                .execute(&mut *tx)
                .await?;

            for (ref_type, ref_name) in &resolved_refs {
                sqlx::query(
                    "INSERT INTO learning_references (learning_id, workspace_id, ref_type, ref_name) \
                     VALUES ($1, $2, $3, $4) \
                     ON CONFLICT (learning_id, ref_type, ref_name) DO NOTHING",
                )
                .bind(parsed_id.to_string())
                .bind(workspace_id)
                .bind(ref_type)
                .bind(ref_name)
                .execute(&mut *tx)
                .await?;
            }

            tx.commit().await?;
        }
    }

    tracing::debug!(
        learning_id,
        refs = resolved_refs.len(),
        "Learning references materialized"
    );

    Ok(())
}

// ---------------------------------------------------------------------------
// Backfill
// ---------------------------------------------------------------------------

/// Backfill references for all active learnings in a workspace.
/// Returns the number of learnings processed.
///
/// Pre-fetches workspace tables once and passes them to each learning's
/// materialization to avoid N+1 queries.
pub async fn backfill_all_references(
    db: &DbPool,
    workspace_id: &str,
) -> anyhow::Result<usize> {
    #[derive(sqlx::FromRow)]
    struct LearningIdRow {
        learning_id: String,
    }

    let is_pg = db.is_postgres();
    let true_val = kyomi_core::sql_compat::bool_true(is_pg);
    let false_val = kyomi_core::sql_compat::bool_false(is_pg);
    let sql = format!(
        "SELECT CAST(learning_id AS TEXT) as learning_id FROM agent_learnings \
         WHERE workspace_id = $1 AND enabled = {true_val} AND is_superseded = {false_val}"
    );

    let learning_rows = kyomi_core::db_fetch_all!(
        db,
        LearningIdRow,
        &sql,
        &workspace_id
    )?;

    // Pre-fetch all workspace tables once to avoid N+1 queries
    let cached_tables = kyomi_core::db_fetch_all!(
        db,
        CachedTable,
        "SELECT project_id, dataset_id, table_id \
         FROM datasource_table_cache \
         WHERE workspace_id = $1 AND is_archived = false",
        &workspace_id
    )?;

    let known_full_names: Vec<String> = cached_tables
        .iter()
        .map(|t| kyomi_core::build_full_table_name(&t.project_id, &t.dataset_id, &t.table_id))
        .collect();

    for lr in &learning_rows {
        materialize_learning_references(db, &lr.learning_id, workspace_id, Some(&known_full_names)).await?;
    }

    tracing::info!(
        count = learning_rows.len(),
        workspace_id,
        "Backfilled learning references"
    );

    Ok(learning_rows.len())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Resolve a raw table name to a canonical full name from known tables.
///
/// Matching rules:
/// - Exact match: `raw_name == full_name`
/// - Suffix match: `full_name` ends with `.<raw_name>`
///
/// Returns the first match found.
fn resolve_table_name(raw_name: &str, known_full_names: &[String]) -> Option<String> {
    // Try exact match first
    if let Some(full) = known_full_names.iter().find(|f| f.as_str() == raw_name) {
        return Some(full.clone());
    }

    // Try suffix match (e.g., raw_name "orders" matches "public.orders")
    let suffix = format!(".{raw_name}");
    let matches: Vec<&String> = known_full_names
        .iter()
        .filter(|f| f.ends_with(&suffix))
        .collect();

    if matches.len() > 1 {
        tracing::warn!(
            raw_name,
            match_count = matches.len(),
            first_match = %matches[0],
            "Ambiguous suffix match: multiple tables match '{}', using first",
            raw_name,
        );
    }

    matches.into_iter().next().cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_table_name_exact() {
        let known = vec![
            "public.orders".to_string(),
            "public.customers".to_string(),
        ];
        assert_eq!(
            resolve_table_name("public.orders", &known),
            Some("public.orders".to_string())
        );
    }

    #[test]
    fn test_resolve_table_name_suffix() {
        let known = vec![
            "public.orders".to_string(),
            "myproject.billing.subscriptions".to_string(),
        ];
        assert_eq!(
            resolve_table_name("orders", &known),
            Some("public.orders".to_string())
        );
        assert_eq!(
            resolve_table_name("billing.subscriptions", &known),
            Some("myproject.billing.subscriptions".to_string())
        );
    }

    #[test]
    fn test_resolve_table_name_not_found() {
        let known = vec!["public.orders".to_string()];
        assert_eq!(resolve_table_name("missing_table", &known), None);
    }

    #[test]
    fn test_resolve_table_name_no_partial() {
        // "rders" should NOT match "public.orders" (suffix requires dot prefix)
        let known = vec!["public.orders".to_string()];
        assert_eq!(resolve_table_name("rders", &known), None);
    }
}
