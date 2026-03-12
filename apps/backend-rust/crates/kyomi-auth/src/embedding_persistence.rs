// SPDX-License-Identifier: AGPL-3.0-or-later

//! Embedding persistence — store, delete, and search pgvector embeddings.
//!
//! Provides helpers for:
//! - Storing search embeddings for datasource catalog entries
//! - Deleting embeddings when tables are re-indexed
//!
//! ## Migration Notice
//!
//! The `datasource_search_embeddings` table is being dropped (replaced by
//! pgvector embeddings on `datasource_table_cache` + `column_embeddings`
//! via `kyomi-knowledge`). The remaining functions here are used during
//! catalog indexing and will be migrated in a follow-up.

use kyomi_core::{DbPool, Result};
use pgvector::Vector;

// ─── Catalog search embedding persistence ─────────────────────────────────────

/// A single search entry to store as an embedding.
pub struct SearchEntryInsert {
    pub table_cache_id: i32,
    pub workspace_id: String,
    pub datasource_config_id: Option<String>,
    pub project_id: String,
    pub dataset_id: String,
    pub table_id: String,
    pub entry_type: String,
    pub text: String,
    pub weight: f64,
    pub column_name: Option<String>,
    pub embedding: Vector,
}

/// Batch-insert search embedding rows for a cached table.
///
/// Used during catalog indexing — each table generates multiple search entries
/// (table name, description, columns) with their pre-computed embeddings.
pub async fn store_search_embeddings(
    db: &DbPool,
    entries: &[SearchEntryInsert],
) -> Result<()> {
    if entries.is_empty() {
        return Ok(());
    }

    let sql = r#"
        INSERT INTO datasource_search_embeddings
            (table_cache_id, workspace_id, datasource_config_id, project_id,
             dataset_id, table_id, entry_type, text, weight, column_name, embedding)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
    "#;

    // Use a transaction for atomic batch insert
    match db {
        kyomi_core::db::DbPool::Postgres(pg) => {
            let mut tx = pg.begin().await.map_err(|e| {
                kyomi_core::Error::Internal(format!("failed to begin transaction: {e}"))
            })?;

            for entry in entries {
                sqlx::query(sql)
                    .bind(entry.table_cache_id)
                    .bind(&entry.workspace_id)
                    .bind(&entry.datasource_config_id)
                    .bind(&entry.project_id)
                    .bind(&entry.dataset_id)
                    .bind(&entry.table_id)
                    .bind(&entry.entry_type)
                    .bind(&entry.text)
                    .bind(entry.weight)
                    .bind(&entry.column_name)
                    .bind(&entry.embedding)
                    .execute(&mut *tx)
                    .await
                    .map_err(|e| {
                        kyomi_core::Error::Internal(format!("failed to insert search embedding: {e}"))
                    })?;
            }

            tx.commit().await.map_err(|e| {
                kyomi_core::Error::Internal(format!("failed to commit search embeddings: {e}"))
            })?;
        }
        kyomi_core::db::DbPool::Sqlite(_) => {
            return Err(kyomi_core::Error::Internal(
                "pgvector embeddings are not supported on SQLite".into(),
            ));
        }
    }

    Ok(())
}

/// Delete all search embeddings for a specific cached table.
///
/// Called before re-indexing a table to avoid stale embeddings.
pub async fn delete_embeddings_for_table(
    db: &DbPool,
    table_cache_id: i32,
) -> Result<u64> {
    let result = kyomi_core::db_execute!(
        db,
        "DELETE FROM datasource_search_embeddings WHERE table_cache_id = $1",
        table_cache_id
    )
    .map_err(|e| {
        kyomi_core::Error::Internal(format!("failed to delete embeddings: {e}"))
    })?;

    Ok(result.rows_affected())
}
