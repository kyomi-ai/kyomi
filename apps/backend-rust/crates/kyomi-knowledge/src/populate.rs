// SPDX-License-Identifier: AGPL-3.0-or-later

//! Embedding population pipeline -- generate and store embeddings.
//!
//! Replaces the FalkorDB population pipeline from `kyomi-graph::population`.
//! Writes embeddings to the database:
//!
//! - `datasource_table_cache.{name_embedding, desc_embedding}` for tables
//! - `column_embeddings.{name_embedding, desc_embedding}` for columns
//! - `agent_learnings.embedding` for learnings
//!
//! Postgres: binds `pgvector::Vector` for native vector operations.
//! SQLite: binds `Vec<u8>` BLOB (little-endian f32 bytes).

use kyomi_core::db::DbPool;
use kyomi_core::embedding_compat::embedding_to_bytes;
use kyomi_embed::EmbeddingService;
use serde_json::Value as JsonValue;

// ---------------------------------------------------------------------------
// Table embeddings
// ---------------------------------------------------------------------------

#[derive(sqlx::FromRow)]
struct TableRow {
    id: i32,
    project_id: String,
    dataset_id: String,
    table_id: String,
    table_metadata: Option<String>,
}

/// Populate table name/description embeddings for all non-archived tables
/// in a datasource. Returns the number of tables processed.
pub async fn populate_table_embeddings(
    db: &DbPool,
    embed: &EmbeddingService,
    workspace_id: &str,
    datasource_config_id: &str,
) -> anyhow::Result<usize> {
    let is_pg = db.is_postgres();
    let bool_false = kyomi_core::sql_compat::bool_false(is_pg);
    let sql = format!(
        "SELECT id, project_id, dataset_id, table_id, \
         CAST(table_metadata AS TEXT) as table_metadata \
         FROM datasource_table_cache \
         WHERE workspace_id = $1 AND datasource_config_id = $2 AND is_archived = {bool_false} \
           AND name_embedding IS NULL"
    );
    let rows = kyomi_core::db_fetch_all!(
        db,
        TableRow,
        &sql,
        &workspace_id,
        &datasource_config_id
    )?;

    if rows.is_empty() {
        return Ok(0);
    }

    // Batch-embed all table full names
    let full_names: Vec<String> = rows
        .iter()
        .map(|r| kyomi_core::build_full_table_name(&r.project_id, &r.dataset_id, &r.table_id))
        .collect();
    let name_refs: Vec<&str> = full_names.iter().map(|s| s.as_str()).collect();
    let name_embeddings = embed.embed_passages(&name_refs)?;

    // Batch-embed descriptions for tables that have them
    let metadata_values: Vec<Option<JsonValue>> = rows
        .iter()
        .map(|r| r.table_metadata.as_deref().and_then(|s| serde_json::from_str(s).ok()))
        .collect();
    let desc_texts: Vec<Option<&str>> = metadata_values
        .iter()
        .map(|m| extract_description_from_value(m.as_ref()))
        .collect();
    let non_empty_descs: Vec<&str> = desc_texts.iter().filter_map(|d| *d).collect();
    let desc_embeddings_batch = if non_empty_descs.is_empty() {
        vec![]
    } else {
        embed.embed_passages(&non_empty_descs)?
    };
    let mut desc_embed_iter = desc_embeddings_batch.into_iter();

    for (i, row) in rows.iter().enumerate() {
        let desc_emb = if desc_texts[i].is_some() {
            Some(desc_embed_iter
                .next()
                .ok_or_else(|| anyhow::anyhow!("missing description embedding"))?)
        } else {
            None
        };

        match db {
            DbPool::Postgres(pg) => {
                // Bind pgvector::Vector directly from f32 slices -- no byte round-trip.
                let name_vec = pgvector::Vector::from(name_embeddings[i].clone());
                let desc_vec = desc_emb.map(|e| pgvector::Vector::from(e));
                sqlx::query(
                    "UPDATE datasource_table_cache \
                     SET name_embedding = $1, desc_embedding = $2 \
                     WHERE id = $3",
                )
                .bind(&name_vec)
                .bind(&desc_vec)
                .bind(row.id)
                .execute(pg)
                .await?;
            }
            DbPool::Sqlite(sq) => {
                let name_bytes = embedding_to_bytes(&name_embeddings[i]);
                let desc_bytes = desc_emb.map(|e| embedding_to_bytes(&e));
                sqlx::query(
                    "UPDATE datasource_table_cache \
                     SET name_embedding = $1, desc_embedding = $2 \
                     WHERE id = $3",
                )
                .bind(&name_bytes)
                .bind(&desc_bytes)
                .bind(row.id)
                .execute(sq)
                .await?;
            }
        }
    }

    debug_assert!(
        desc_embed_iter.next().is_none(),
        "BUG: description embedding batch size mismatch"
    );

    let count = rows.len();
    tracing::info!(count, datasource_config_id, "Table embeddings populated");
    Ok(count)
}

// ---------------------------------------------------------------------------
// Column embeddings
// ---------------------------------------------------------------------------

/// Populate column embeddings for all columns in a datasource's tables.
/// Returns the number of columns processed.
pub async fn populate_column_embeddings(
    db: &DbPool,
    embed: &EmbeddingService,
    workspace_id: &str,
    datasource_config_id: &str,
) -> anyhow::Result<usize> {
    let is_pg = db.is_postgres();
    let bool_false = kyomi_core::sql_compat::bool_false(is_pg);
    let table_sql = format!(
        "SELECT id, project_id, dataset_id, table_id, \
         CAST(table_metadata AS TEXT) as table_metadata \
         FROM datasource_table_cache \
         WHERE workspace_id = $1 AND datasource_config_id = $2 AND is_archived = {bool_false}"
    );
    let table_rows = kyomi_core::db_fetch_all!(
        db,
        TableRow,
        &table_sql,
        &workspace_id,
        &datasource_config_id
    )?;

    struct ColumnInfo {
        table_cache_id: i32,
        table_full_name: String,
        name: String,
        data_type: String,
        description: Option<String>,
    }

    let mut all_columns: Vec<ColumnInfo> = Vec::new();
    for row in &table_rows {
        let full_name = kyomi_core::build_full_table_name(&row.project_id, &row.dataset_id, &row.table_id);
        let metadata: Option<JsonValue> = row.table_metadata.as_deref().and_then(|s| serde_json::from_str(s).ok());
        let metadata_ref = metadata.as_ref().cloned().unwrap_or(JsonValue::Null);
        let columns = extract_columns_from_metadata(&metadata_ref);
        for (col_name, col_type, col_desc) in columns {
            all_columns.push(ColumnInfo {
                table_cache_id: row.id,
                table_full_name: full_name.clone(),
                name: col_name,
                data_type: col_type,
                description: col_desc,
            });
        }
    }

    if all_columns.is_empty() {
        return Ok(0);
    }

    // Filter out columns that already have embeddings
    #[derive(sqlx::FromRow)]
    struct ExistingCol {
        table_cache_id: i32,
        column_name: String,
    }

    let existing: Vec<ExistingCol> = kyomi_core::db_fetch_all!(
        db,
        ExistingCol,
        "SELECT table_cache_id, column_name \
         FROM column_embeddings \
         WHERE workspace_id = $1 AND name_embedding IS NOT NULL",
        &workspace_id
    )?;

    let existing_set: std::collections::HashSet<(i32, &str)> = existing
        .iter()
        .map(|e| (e.table_cache_id, e.column_name.as_str()))
        .collect();

    all_columns.retain(|c| !existing_set.contains(&(c.table_cache_id, c.name.as_str())));

    if all_columns.is_empty() {
        return Ok(0);
    }

    // Batch-embed column names with enriched text: "table_full_name.column_name (data_type)"
    let name_texts: Vec<String> = all_columns
        .iter()
        .map(|c| format!("{}.{} ({})", c.table_full_name, c.name, c.data_type))
        .collect();
    let name_refs: Vec<&str> = name_texts.iter().map(|s| s.as_str()).collect();
    let name_embeddings = embed.embed_passages(&name_refs)?;

    // Batch-embed column descriptions where they exist
    let non_empty_descs: Vec<&str> = all_columns
        .iter()
        .filter_map(|c| c.description.as_deref())
        .collect();
    let desc_embeddings_batch = if non_empty_descs.is_empty() {
        vec![]
    } else {
        embed.embed_passages(&non_empty_descs)?
    };
    let mut desc_embed_iter = desc_embeddings_batch.into_iter();

    for (i, col) in all_columns.iter().enumerate() {
        let desc_emb = if col.description.is_some() {
            Some(desc_embed_iter
                .next()
                .ok_or_else(|| anyhow::anyhow!("missing column description embedding"))?)
        } else {
            None
        };

        match db {
            DbPool::Postgres(pg) => {
                // Bind pgvector::Vector directly from f32 slices -- no byte round-trip.
                let name_vec = pgvector::Vector::from(name_embeddings[i].clone());
                let desc_vec = desc_emb.map(|e| pgvector::Vector::from(e));
                sqlx::query(
                    "INSERT INTO column_embeddings \
                         (table_cache_id, workspace_id, column_name, data_type, description, \
                          name_embedding, desc_embedding) \
                     VALUES ($1, $2, $3, $4, $5, $6, $7) \
                     ON CONFLICT (table_cache_id, column_name) DO UPDATE SET \
                         data_type = EXCLUDED.data_type, \
                         description = EXCLUDED.description, \
                         name_embedding = EXCLUDED.name_embedding, \
                         desc_embedding = EXCLUDED.desc_embedding",
                )
                .bind(col.table_cache_id)
                .bind(workspace_id)
                .bind(&col.name)
                .bind(&col.data_type)
                .bind(col.description.as_deref())
                .bind(&name_vec)
                .bind(&desc_vec)
                .execute(pg)
                .await?;
            }
            DbPool::Sqlite(sq) => {
                let name_bytes = embedding_to_bytes(&name_embeddings[i]);
                let desc_bytes = desc_emb.map(|e| embedding_to_bytes(&e));
                sqlx::query(
                    "INSERT INTO column_embeddings \
                         (table_cache_id, workspace_id, column_name, data_type, description, \
                          name_embedding, desc_embedding) \
                     VALUES ($1, $2, $3, $4, $5, $6, $7) \
                     ON CONFLICT (table_cache_id, column_name) DO UPDATE SET \
                         data_type = EXCLUDED.data_type, \
                         description = EXCLUDED.description, \
                         name_embedding = EXCLUDED.name_embedding, \
                         desc_embedding = EXCLUDED.desc_embedding",
                )
                .bind(col.table_cache_id)
                .bind(workspace_id)
                .bind(&col.name)
                .bind(&col.data_type)
                .bind(col.description.as_deref())
                .bind(&name_bytes)
                .bind(&desc_bytes)
                .execute(sq)
                .await?;
            }
        }
    }

    debug_assert!(
        desc_embed_iter.next().is_none(),
        "BUG: column description embedding batch size mismatch"
    );

    let count = all_columns.len();
    tracing::info!(count, datasource_config_id, "Column embeddings populated");
    Ok(count)
}

// ---------------------------------------------------------------------------
// Learning embeddings
// ---------------------------------------------------------------------------

#[derive(sqlx::FromRow)]
struct LearningInsightRow {
    insight: String,
}

/// Populate embedding for a single learning.
pub async fn populate_learning_embedding(
    db: &DbPool,
    embed: &EmbeddingService,
    learning_id: &str,
) -> anyhow::Result<()> {
    let row = kyomi_core::db_fetch_optional!(
        db,
        LearningInsightRow,
        "SELECT insight FROM agent_learnings WHERE learning_id = $1",
        learning_id
    )?;

    let row = match row {
        Some(r) => r,
        None => {
            tracing::warn!(learning_id, "Learning not found for embedding population");
            return Ok(());
        }
    };

    let embedding = embed.embed_passage(&row.insight)?;

    match db {
        DbPool::Postgres(pg) => {
            // Bind pgvector::Vector directly from f32 vec -- no byte round-trip.
            let vec = pgvector::Vector::from(embedding.clone());
            sqlx::query("UPDATE agent_learnings SET embedding = $1 WHERE learning_id = $2")
                .bind(&vec)
                .bind(learning_id)
                .execute(pg)
                .await?;
        }
        DbPool::Sqlite(sq) => {
            let emb_bytes = embedding_to_bytes(&embedding);
            sqlx::query("UPDATE agent_learnings SET embedding = $1 WHERE learning_id = $2")
                .bind(&emb_bytes)
                .bind(learning_id)
                .execute(sq)
                .await?;
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Full workspace population
// ---------------------------------------------------------------------------

/// Full population: all tables, columns, and learnings for a workspace.
pub async fn populate_workspace(
    db: &DbPool,
    embed: &EmbeddingService,
    workspace_id: &str,
) -> anyhow::Result<()> {
    // Get all distinct datasource_config_ids for the workspace
    #[derive(sqlx::FromRow)]
    struct DsId {
        datasource_config_id: String,
    }

    let is_pg = db.is_postgres();
    let bool_false = kyomi_core::sql_compat::bool_false(is_pg);
    let bool_true = kyomi_core::sql_compat::bool_true(is_pg);
    let ds_sql = format!(
        "SELECT DISTINCT datasource_config_id \
         FROM datasource_table_cache \
         WHERE workspace_id = $1 AND is_archived = {bool_false} \
           AND datasource_config_id IS NOT NULL"
    );
    let ds_rows = kyomi_core::db_fetch_all!(
        db,
        DsId,
        &ds_sql,
        &workspace_id
    )?;

    let mut total_tables = 0usize;
    let mut total_columns = 0usize;

    for ds_row in &ds_rows {
        total_tables += populate_table_embeddings(db, embed, workspace_id, &ds_row.datasource_config_id).await?;
        total_columns += populate_column_embeddings(db, embed, workspace_id, &ds_row.datasource_config_id).await?;
    }

    // Populate learning embeddings
    #[derive(sqlx::FromRow)]
    struct LearningIdRow {
        learning_id: String,
    }

    let learning_sql = format!(
        "SELECT CAST(learning_id AS TEXT) as learning_id FROM agent_learnings \
         WHERE workspace_id = $1 AND enabled = {bool_true} AND is_superseded = {bool_false} \
           AND embedding IS NULL"
    );
    let learning_rows = kyomi_core::db_fetch_all!(
        db,
        LearningIdRow,
        &learning_sql,
        &workspace_id
    )?;

    for lr in &learning_rows {
        populate_learning_embedding(db, embed, &lr.learning_id).await?;
    }

    tracing::info!(
        tables = total_tables,
        columns = total_columns,
        learnings = learning_rows.len(),
        workspace_id,
        "Workspace population complete"
    );

    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Extract description from table_metadata JSON value.
fn extract_description_from_value(metadata: Option<&JsonValue>) -> Option<&str> {
    metadata?
        .get("description")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
}

/// Extract (column_name, column_type, column_description) triples from table_metadata JSON.
pub fn extract_columns_from_metadata(metadata: &JsonValue) -> Vec<(String, String, Option<String>)> {
    let columns = match metadata.get("columns").and_then(|c| c.as_array()) {
        Some(cols) => cols,
        None => return vec![],
    };

    columns
        .iter()
        .filter_map(|col| {
            let name = col.get("name").and_then(|n| n.as_str())?;
            let col_type = col
                .get("type")
                .and_then(|t| t.as_str())
                .unwrap_or("unknown");
            let description = col
                .get("description")
                .and_then(|d| d.as_str())
                .filter(|d| !d.is_empty())
                .map(|d| d.to_string());
            Some((name.to_string(), col_type.to_string(), description))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_columns_from_metadata() {
        let metadata = serde_json::json!({
            "columns": [
                {"name": "id", "type": "INTEGER"},
                {"name": "email", "type": "VARCHAR", "description": "User email address"},
                {"name": "created_at", "type": "TIMESTAMP", "description": ""}
            ]
        });
        let cols = extract_columns_from_metadata(&metadata);
        assert_eq!(cols.len(), 3);
        assert_eq!(cols[0], ("id".to_string(), "INTEGER".to_string(), None));
        assert_eq!(
            cols[1],
            (
                "email".to_string(),
                "VARCHAR".to_string(),
                Some("User email address".to_string())
            )
        );
        // Empty description should be treated as None
        assert_eq!(
            cols[2],
            ("created_at".to_string(), "TIMESTAMP".to_string(), None)
        );
    }

    #[test]
    fn test_extract_columns_missing_columns_key() {
        let metadata = serde_json::json!({"description": "A table"});
        let cols = extract_columns_from_metadata(&metadata);
        assert!(cols.is_empty());
    }

    #[test]
    fn test_extract_columns_missing_name() {
        let metadata = serde_json::json!({
            "columns": [
                {"type": "INTEGER"},
                {"name": "valid", "type": "TEXT"}
            ]
        });
        let cols = extract_columns_from_metadata(&metadata);
        assert_eq!(cols.len(), 1);
        assert_eq!(cols[0].0, "valid");
    }

    #[test]
    fn test_extract_description_from_value() {
        let meta = serde_json::json!({"description": "A useful table"});
        assert_eq!(extract_description_from_value(Some(&meta)), Some("A useful table"));

        let meta_empty = serde_json::json!({"description": ""});
        assert_eq!(extract_description_from_value(Some(&meta_empty)), None);

        let meta_none = serde_json::json!({});
        assert_eq!(extract_description_from_value(Some(&meta_none)), None);

        assert_eq!(extract_description_from_value(None), None);
    }
}
