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
) -> kyomi_core::Result<usize> {
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
    let name_embeddings = embed.embed_passages_chunked(&name_refs).await?;

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
        embed.embed_passages_chunked(&non_empty_descs).await?
    };
    let mut desc_embed_iter = desc_embeddings_batch.into_iter();

    for (i, row) in rows.iter().enumerate() {
        let desc_emb = if desc_texts[i].is_some() {
            Some(desc_embed_iter
                .next()
                .ok_or_else(|| kyomi_core::Error::Internal("missing description embedding".into()))?)
        } else {
            None
        };

        match db {
            DbPool::Postgres(pg) => {
                // Bind pgvector::Vector directly from f32 slices -- no byte round-trip.
                let name_vec = pgvector::Vector::from(name_embeddings[i].clone());
                let desc_vec = desc_emb.map(pgvector::Vector::from);
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

/// Outcome of a [`populate_column_embeddings`] run.
///
/// Deliberately not a bare `usize`: `embedded` alone cannot distinguish "every
/// column got an embedding" from "some columns were silently dropped", which
/// is exactly the KYO-658 bug (a run that wrote fewer rows than it read was
/// reported as a plain success). A caller must look at `skipped` before
/// treating this as a clean run — see
/// `docs/standards/error-handling/empty-on-failure-must-not-look-like-a-real-result.md`.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ColumnEmbeddingOutcome {
    /// Columns whose embeddings were written.
    pub embedded: usize,
    /// Columns skipped because their `table_cache_id` no longer existed at
    /// write time (see [`write_column_embeddings_with_fk_guard`]).
    pub skipped: usize,
}

impl ColumnEmbeddingOutcome {
    /// Total columns this run attempted to write (embedded + skipped).
    pub fn attempted(&self) -> usize {
        self.embedded + self.skipped
    }

    /// `None` if every attempted column embedding was written -- a genuinely
    /// clean run. `Some(message)` otherwise, describing the shortfall in
    /// terms a log line or a `CatalogIndexResult.errors` entry can use
    /// verbatim.
    ///
    /// This is the single place that decides "was this run a clean
    /// success" -- callers must go through it rather than treating `Ok(_)`
    /// from [`populate_column_embeddings`] as proof nothing was lost (that
    /// was KYO-658: a run that silently wrote fewer rows than it read was
    /// reported as a plain success). Mirrors the pure
    /// `resolve_public_index_result` pattern in
    /// `kyomi_auth::catalog::indexers::bigquery_public`.
    pub fn failure_message(&self) -> Option<String> {
        if self.skipped == 0 {
            return None;
        }
        Some(format!(
            "{} of {} column embeddings were skipped -- their table_cache row no \
             longer existed at write time (the datasource or table was likely \
             deleted while embedding was in flight; see KYO-658)",
            self.skipped,
            self.attempted()
        ))
    }
}

/// One column ready to be written to `column_embeddings`, with its
/// embeddings already computed.
///
/// Split out from the read-then-embed steps of [`populate_column_embeddings`]
/// purely so a test can drive [`write_column_embeddings_with_fk_guard`]
/// directly with a deliberately stale `table_cache_id`, without needing to
/// force a real concurrent delete to land inside the (multi-second, per
/// KYO-644) embedding call above it. Same reasoning as the
/// `write_dashboard_summary_with_cas` split in `kyomi_agent::execution`
/// (KYO-539).
struct ColumnEmbeddingWrite {
    table_cache_id: i32,
    column_name: String,
    data_type: String,
    description: Option<String>,
    name_embedding: Vec<f32>,
    desc_embedding: Option<Vec<f32>>,
}

/// Write each column's embeddings, guarding against `table_cache_id` having
/// vanished between when it was read (before the slow embedding call in
/// [`populate_column_embeddings`]) and now.
///
/// In production this happens when a user deletes the datasource while
/// embedding is in flight: `DELETE FROM datasource_configs` cascades through
/// `fk_table_cache_datasource` to remove the `datasource_table_cache` row a
/// queued column embedding is about to reference (KYO-658). That is a
/// genuine, expected race — if the datasource is gone there is nothing to
/// attach a column embedding to, so skipping that column is correct.
///
/// What must NOT happen is letting the resulting foreign-key violation
/// propagate via `?`: that would abort the INSERT loop entirely, silently
/// dropping every column still queued after the vanished one — including
/// columns belonging to tables that were never touched. So each column's
/// insert is issued independently and only a genuine foreign-key violation
/// is caught and counted; every other database error still propagates as a
/// real failure. Do not "simplify" this back to a bare `?` — that is the
/// exact bug this function exists to close.
async fn write_column_embeddings_with_fk_guard(
    db: &DbPool,
    workspace_id: &str,
    columns: &[ColumnEmbeddingWrite],
) -> kyomi_core::Result<ColumnEmbeddingOutcome> {
    let mut outcome = ColumnEmbeddingOutcome::default();

    for col in columns {
        let insert_result: Result<(), sqlx::Error> = match db {
            DbPool::Postgres(pg) => {
                // Bind pgvector::Vector directly from f32 slices -- no byte round-trip.
                let name_vec = pgvector::Vector::from(col.name_embedding.clone());
                let desc_vec = col.desc_embedding.clone().map(pgvector::Vector::from);
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
                .bind(&col.column_name)
                .bind(&col.data_type)
                .bind(col.description.as_deref())
                .bind(&name_vec)
                .bind(&desc_vec)
                .execute(pg)
                .await
                .map(|_| ())
            }
            DbPool::Sqlite(sq) => {
                let name_bytes = embedding_to_bytes(&col.name_embedding);
                let desc_bytes = col.desc_embedding.as_ref().map(|e| embedding_to_bytes(e));
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
                .bind(&col.column_name)
                .bind(&col.data_type)
                .bind(col.description.as_deref())
                .bind(&name_bytes)
                .bind(&desc_bytes)
                .execute(sq)
                .await
                .map(|_| ())
            }
        };

        match insert_result {
            Ok(()) => outcome.embedded += 1,
            Err(sqlx::Error::Database(db_err)) if db_err.is_foreign_key_violation() => {
                outcome.skipped += 1;
                tracing::debug!(
                    workspace_id,
                    table_cache_id = col.table_cache_id,
                    column_name = %col.column_name,
                    "column_embeddings insert skipped -- table_cache_id no longer exists \
                     (datasource or table deleted while embedding was in flight, KYO-658)"
                );
            }
            Err(e) => return Err(e.into()),
        }
    }

    Ok(outcome)
}

/// Returns how many columns were embedded and how many were skipped because
/// their table vanished mid-run (see [`write_column_embeddings_with_fk_guard`]).
/// A non-zero `skipped` count means this run did NOT fully succeed even
/// though it returns `Ok` — callers must check it rather than treating `Ok`
/// alone as "everything was embedded".
pub async fn populate_column_embeddings(
    db: &DbPool,
    embed: &EmbeddingService,
    workspace_id: &str,
    datasource_config_id: &str,
) -> kyomi_core::Result<ColumnEmbeddingOutcome> {
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
        return Ok(ColumnEmbeddingOutcome::default());
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
        return Ok(ColumnEmbeddingOutcome::default());
    }

    // Batch-embed column names with enriched text: "table_full_name.column_name (data_type)"
    let name_texts: Vec<String> = all_columns
        .iter()
        .map(|c| format!("{}.{} ({})", c.table_full_name, c.name, c.data_type))
        .collect();
    let name_refs: Vec<&str> = name_texts.iter().map(|s| s.as_str()).collect();
    let name_embeddings = embed.embed_passages_chunked(&name_refs).await?;

    // Batch-embed column descriptions where they exist
    let non_empty_descs: Vec<&str> = all_columns
        .iter()
        .filter_map(|c| c.description.as_deref())
        .collect();
    let desc_embeddings_batch = if non_empty_descs.is_empty() {
        vec![]
    } else {
        embed.embed_passages_chunked(&non_empty_descs).await?
    };
    let mut desc_embed_iter = desc_embeddings_batch.into_iter();

    // This point is where KYO-658's race window opens: the slow embedding
    // calls above just ran (~34s for 538 columns, per KYO-644), and every
    // `table_cache_id` captured in `all_columns` was read *before* them. By
    // the time the writes below run, some of those rows may already be gone.
    let mut writes = Vec::with_capacity(all_columns.len());
    for (i, col) in all_columns.iter().enumerate() {
        let desc_emb = if col.description.is_some() {
            Some(desc_embed_iter
                .next()
                .ok_or_else(|| kyomi_core::Error::Internal("missing column description embedding".into()))?)
        } else {
            None
        };
        writes.push(ColumnEmbeddingWrite {
            table_cache_id: col.table_cache_id,
            column_name: col.name.clone(),
            data_type: col.data_type.clone(),
            description: col.description.clone(),
            name_embedding: name_embeddings[i].clone(),
            desc_embedding: desc_emb,
        });
    }

    debug_assert!(
        desc_embed_iter.next().is_none(),
        "BUG: column description embedding batch size mismatch"
    );

    let outcome = write_column_embeddings_with_fk_guard(db, workspace_id, &writes).await?;

    tracing::info!(
        embedded = outcome.embedded,
        skipped = outcome.skipped,
        datasource_config_id,
        "Column embeddings populated"
    );
    Ok(outcome)
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
) -> kyomi_core::Result<()> {
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
) -> kyomi_core::Result<()> {
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
    let mut total_columns_skipped = 0usize;

    for ds_row in &ds_rows {
        total_tables += populate_table_embeddings(db, embed, workspace_id, &ds_row.datasource_config_id).await?;
        let outcome =
            populate_column_embeddings(db, embed, workspace_id, &ds_row.datasource_config_id).await?;
        total_columns += outcome.embedded;
        total_columns_skipped += outcome.skipped;
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

    if total_columns_skipped > 0 {
        // See ColumnEmbeddingOutcome::failure_message -- this run did NOT
        // fully succeed even though every step above returned `Ok`, and
        // must not be reported at the same severity as a clean run.
        tracing::error!(
            workspace_id,
            columns_embedded = total_columns,
            columns_skipped = total_columns_skipped,
            "Workspace population did not fully succeed -- some column embeddings \
             were skipped because their table_cache row no longer existed (KYO-658)"
        );
    }

    tracing::info!(
        tables = total_tables,
        columns = total_columns,
        columns_skipped = total_columns_skipped,
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

    // -- ColumnEmbeddingOutcome::failure_message -- KYO-658 -------------------
    //
    // AC2: a run that fails to embed some or all columns must not be
    // reported as a clean success. `failure_message` is the single place
    // that decision gets made, so pin it directly with synthetic outcomes
    // rather than needing a full DB-backed run for every case -- mirrors
    // `resolve_public_index_result`'s tests in
    // `kyomi_auth::catalog::indexers::bigquery_public`.

    #[test]
    fn column_embedding_outcome_with_any_skips_is_not_a_clean_success() {
        let mostly_fine = ColumnEmbeddingOutcome { embedded: 537, skipped: 1 };
        let message = mostly_fine
            .failure_message()
            .expect("even a single skipped column must produce a failure message");
        assert!(message.contains('1'), "message must name the skip count: {message}");
        assert!(message.contains("538"), "message must name the total attempted: {message}");

        let total_loss = ColumnEmbeddingOutcome { embedded: 0, skipped: 12 };
        assert!(total_loss.failure_message().is_some());
    }

    #[test]
    fn column_embedding_outcome_with_zero_skips_is_a_clean_success() {
        let clean = ColumnEmbeddingOutcome { embedded: 538, skipped: 0 };
        assert_eq!(clean.failure_message(), None);

        let nothing_to_do = ColumnEmbeddingOutcome::default();
        assert_eq!(nothing_to_do.failure_message(), None);
    }

    // -- write_column_embeddings_with_fk_guard -- KYO-658 race fix ------------
    //
    // AC1: reproduces the reported FK violation. `datasource_table_cache`
    // and `column_embeddings` migrations declare a real
    // `ON DELETE CASCADE` foreign key (see
    // `apps/server/migrations-sqlite/00008_embedding_columns.sql` and the
    // Postgres equivalent), and this test pool runs with
    // `PRAGMA foreign_keys=ON` (`kyomi_core::db::DbPool::connect`), so the
    // violation below is the real constraint firing, not a simulation.
    //
    // A live concurrent-delete race is not forced here: per
    // `docs/standards/testing/nondeterministic-verdict-is-a-failing-test.md`,
    // interleaving a delete inside the embedding call would make the test's
    // outcome depend on scheduler timing. Instead this seeds a
    // `table_cache_id` that is already stale by the time the guarded insert
    // runs -- the same technique `write_dashboard_summary_with_cas`'s own
    // tests use for the equivalent seam in `kyomi_agent::execution`
    // (KYO-539), and one of the two reproduction strategies the ticket
    // sanctions. `write_column_embeddings_with_fk_guard` takes embeddings
    // that are already computed, so no embedding model needs to load for
    // this test -- the synthetic `Vec<f32>` below is plain data, not a
    // stubbed embedding *service*.
    async fn seed_workspace_and_table(
        db: &DbPool,
        workspace_id: &str,
        datasource_config_id: &str,
        table_id: &str,
    ) -> i32 {
        let sq = match db {
            DbPool::Sqlite(pool) => pool,
            DbPool::Postgres(_) => unreachable!("test pool is always sqlite"),
        };
        sqlx::query(
            "INSERT OR IGNORE INTO users (user_id, email) VALUES ('user-a', 'a@test.local')",
        )
        .execute(sq)
        .await
        .expect("seed user");
        sqlx::query(
            "INSERT OR IGNORE INTO workspaces (workspace_id, name, owner_user_id) \
             VALUES ($1, 'Workspace', 'user-a')",
        )
        .bind(workspace_id)
        .execute(sq)
        .await
        .expect("seed workspace");
        sqlx::query(
            "INSERT INTO datasource_configs (id, workspace_id, name, datasource_type, slug) \
             VALUES ($1, $2, $1, 'postgres', $1)",
        )
        .bind(datasource_config_id)
        .bind(workspace_id)
        .execute(sq)
        .await
        .expect("seed datasource_configs");
        sqlx::query(
            "INSERT INTO datasource_table_cache \
                 (workspace_id, datasource_config_id, project_id, dataset_id, table_id, table_metadata) \
             VALUES ($1, $2, 'proj', 'dataset', $3, '{}')",
        )
        .bind(workspace_id)
        .bind(datasource_config_id)
        .bind(table_id)
        .execute(sq)
        .await
        .expect("seed datasource_table_cache");

        sqlx::query_scalar::<_, i32>(
            "SELECT id FROM datasource_table_cache \
             WHERE datasource_config_id = $1 AND table_id = $2",
        )
        .bind(datasource_config_id)
        .bind(table_id)
        .fetch_one(sq)
        .await
        .expect("fetch seeded table_cache id")
    }

    fn fake_embedding() -> Vec<f32> {
        vec![0.1_f32; 384]
    }

    #[tokio::test]
    async fn write_column_embeddings_with_fk_guard_skips_a_vanished_table_and_counts_it() {
        let db = kyomi_core::db::DbPool::connect("sqlite::memory:")
            .await
            .expect("connect in-memory sqlite and run the full migration chain");

        let valid_id = seed_workspace_and_table(&db, "ws-1", "ds-valid", "table_valid").await;
        let doomed_id = seed_workspace_and_table(&db, "ws-1", "ds-doomed", "table_doomed").await;

        // Simulate KYO-658's race: the user deleted the doomed datasource
        // (the real production trigger -- `DELETE FROM datasource_configs`,
        // see `kyomi_auth::datasource_service::delete_datasource`) while
        // this column's embedding was still being computed. The cascade
        // has already removed `table_doomed`'s `datasource_table_cache`
        // row by the time the write below runs, exactly as it would in
        // production.
        let sq = match &db {
            DbPool::Sqlite(pool) => pool,
            DbPool::Postgres(_) => unreachable!("test pool is always sqlite"),
        };
        sqlx::query("DELETE FROM datasource_configs WHERE id = 'ds-doomed'")
            .execute(sq)
            .await
            .expect("delete the doomed datasource, cascading to its table_cache row");

        let writes = vec![
            ColumnEmbeddingWrite {
                table_cache_id: valid_id,
                column_name: "id".to_string(),
                data_type: "INTEGER".to_string(),
                description: None,
                name_embedding: fake_embedding(),
                desc_embedding: None,
            },
            ColumnEmbeddingWrite {
                table_cache_id: doomed_id,
                column_name: "id".to_string(),
                data_type: "INTEGER".to_string(),
                description: None,
                name_embedding: fake_embedding(),
                desc_embedding: None,
            },
        ];

        let outcome = write_column_embeddings_with_fk_guard(&db, "ws-1", &writes)
            .await
            .expect(
                "a vanished table_cache row must be skipped and counted, \
                 not propagated as a hard error that aborts the whole batch",
            );

        assert_eq!(
            outcome.embedded, 1,
            "the still-valid table's column must still be written, even though it was \
             queued in the same batch as the vanished one"
        );
        assert_eq!(
            outcome.skipped, 1,
            "the vanished table's column must be counted as skipped, not silently dropped"
        );
        assert!(
            outcome.failure_message().is_some(),
            "a run with a skip must not be reportable as a clean success"
        );

        let embedded_for_valid_table: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM column_embeddings WHERE table_cache_id = $1")
                .bind(valid_id)
                .fetch_one(sq)
                .await
                .expect("count rows for the valid table");
        assert_eq!(embedded_for_valid_table, 1, "the valid column's embedding must actually be persisted");

        let embedded_for_doomed_table: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM column_embeddings WHERE table_cache_id = $1")
                .bind(doomed_id)
                .fetch_one(sq)
                .await
                .expect("count rows for the doomed table");
        assert_eq!(
            embedded_for_doomed_table, 0,
            "the doomed table's id is gone -- there is nothing valid to write a row against"
        );
    }
}

// ---------------------------------------------------------------------------
// Runtime-blocking regression test (KYO-644)
// ---------------------------------------------------------------------------
//
// KYO-644: catalog indexing's post-indexing embedding pass called
// `EmbeddingService::embed_passages` — a synchronous, CPU-bound Candle
// forward pass with no yield points — directly from a `tokio::spawn`ed
// async task. On a datasource with a realistic column count (538 in the
// field reproduction) that single call occupied its executor thread for
// ~34s. In production this correlated with total HTTP unresponsiveness for
// the same window: a bare `GET /api/health` (no DB, no auth, nothing else
// in its way) blocked 34.4s and recovered to 3ms the instant the embedding
// call returned. The precise mechanism connecting one occupied thread to
// whole-pool unresponsiveness on an 8-worker runtime was not conclusively
// isolated — candidates included more concurrent blocking embed calls than
// the CPU sample suggested, and tokio reactor/driver starvation — but this
// fix removes the occupied-thread condition regardless of which mechanism
// was responsible.
//
// The ticket's literal repro steps ("query a datasource, then delete it in
// the same browser session") are NOT what this test drives — the query is
// incidental, merely burning wall clock that happens to land the delete
// inside the stall window, and reproducing it faithfully would need a live
// server and a browser for only indirect evidence. This test drives the
// actual invariant the fix establishes instead: background catalog
// embedding work must not block the async runtime it shares with every
// other request.
#[cfg(test)]
mod runtime_blocking_tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    /// A loaded [`kyomi_embed::EmbeddingService`], cached process-wide so
    /// the (measurably slow) real BERT model load happens at most once no
    /// matter how many tests in this binary need it — same pattern as
    /// `kyomi_agent::test_support::loaded_embedding`.
    fn loaded_embedding() -> kyomi_embed::EmbeddingService {
        static EMBED: std::sync::OnceLock<kyomi_embed::EmbeddingService> = std::sync::OnceLock::new();
        EMBED
            .get_or_init(|| kyomi_embed::EmbeddingService::new().expect("load embedding model for tests"))
            .clone()
    }

    /// Seed a migrated in-memory SQLite pool with a user, workspace,
    /// datasource config, and `num_tables` `datasource_table_cache` rows,
    /// each with `cols_per_table` columns and no descriptions — so only the
    /// column-*name* embedding batch runs, keeping the seeded embedding
    /// work to exactly what this test needs to time.
    async fn seed_datasource_with_columns(
        db: &kyomi_core::DbPool,
        workspace_id: &str,
        datasource_config_id: &str,
        num_tables: usize,
        cols_per_table: usize,
    ) {
        let sq = match db {
            kyomi_core::DbPool::Sqlite(pool) => pool,
            kyomi_core::DbPool::Postgres(_) => unreachable!("test pool is always sqlite"),
        };

        sqlx::query("INSERT INTO users (user_id, email) VALUES ('user-a', 'a@test.local')")
            .execute(sq)
            .await
            .expect("seed user");
        sqlx::query(
            "INSERT INTO workspaces (workspace_id, name, owner_user_id) VALUES ($1, 'Workspace', 'user-a')",
        )
        .bind(workspace_id)
        .execute(sq)
        .await
        .expect("seed workspace");
        sqlx::query(
            "INSERT INTO datasource_configs (id, workspace_id, name, datasource_type, slug) \
             VALUES ($1, $2, 'Test DS', 'postgres', 'test-ds')",
        )
        .bind(datasource_config_id)
        .bind(workspace_id)
        .execute(sq)
        .await
        .expect("seed datasource_configs");

        for t in 0..num_tables {
            let columns: Vec<JsonValue> = (0..cols_per_table)
                .map(|c| serde_json::json!({"name": format!("col_{t}_{c}"), "type": "VARCHAR"}))
                .collect();
            let metadata = serde_json::json!({ "columns": columns }).to_string();
            sqlx::query(
                "INSERT INTO datasource_table_cache \
                     (workspace_id, datasource_config_id, project_id, dataset_id, table_id, table_metadata) \
                 VALUES ($1, $2, 'proj', 'dataset', $3, $4)",
            )
            .bind(workspace_id)
            .bind(datasource_config_id)
            .bind(format!("table_{t}"))
            .bind(&metadata)
            .execute(sq)
            .await
            .expect("seed datasource_table_cache");
        }
    }

    /// Deliberately a **single**-worker runtime. On a multi-core box, a
    /// task blocked on synchronous CPU-bound work does not starve a
    /// *sibling* task the scheduler happens to run on a different worker
    /// thread — verified empirically against tokio 1.x before writing this
    /// test: a `worker_threads = 2` (or higher) runtime does not reproduce
    /// the production failure at all once there are enough real cores to
    /// run both tasks in parallel, because tokio dispatches unrelated
    /// spawned tasks to idle worker threads independently.
    ///
    /// This is a test-detectability choice, not a claim about production's
    /// mechanism: production ran an 8-worker pool, and the field evidence
    /// showed only ~2.7 of 8 cores busy during the stall, yet the whole
    /// server went unresponsive — so "every worker is equally busy" does
    /// not explain what actually happened there (see KYO-644; the
    /// mechanism was not conclusively isolated). A single-worker runtime
    /// sidesteps needing to know that mechanism: with exactly one worker,
    /// the pre-fix synchronous call is *guaranteed* to occupy the only
    /// thread the heartbeat task also needs, which is what makes the block
    /// reliably detectable here regardless of how production's actual
    /// stall propagated.
    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn populate_column_embeddings_does_not_block_the_runtime() {
        const NUM_TABLES: usize = 3;
        const COLS_PER_TABLE: usize = 5;
        const EXPECTED_COLUMNS: usize = NUM_TABLES * COLS_PER_TABLE;

        let db = kyomi_core::db::DbPool::connect("sqlite::memory:")
            .await
            .expect("connect in-memory sqlite and run the full migration chain");
        seed_datasource_with_columns(&db, "ws-1", "ds-1", NUM_TABLES, COLS_PER_TABLE).await;

        let embed = loaded_embedding();

        let worst_gap_ms = Arc::new(AtomicU64::new(0));
        let stop = Arc::new(AtomicBool::new(false));

        let heartbeat = tokio::spawn({
            let worst_gap_ms = worst_gap_ms.clone();
            let stop = stop.clone();
            async move {
                let mut last = Instant::now();
                while !stop.load(Ordering::Relaxed) {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                    let now = Instant::now();
                    worst_gap_ms.fetch_max(
                        now.duration_since(last).as_millis() as u64,
                        Ordering::Relaxed,
                    );
                    last = now;
                }
            }
        });

        // Mirrors production: `CatalogIndexingService::spawn_post_create`
        // runs the embedding pass on its own `tokio::spawn`ed task, sharing
        // the same worker pool every other request runs on.
        let population = tokio::spawn({
            let db = db.clone();
            let embed = embed.clone();
            async move { populate_column_embeddings(&db, &embed, "ws-1", "ds-1").await }
        });

        let result = population.await.expect("population task must not panic");

        stop.store(true, Ordering::Relaxed);
        heartbeat.await.expect("heartbeat task must not panic");

        let outcome = result.expect("populate_column_embeddings must succeed");
        assert_eq!(
            outcome.embedded, EXPECTED_COLUMNS,
            "must embed every seeded column exactly once"
        );
        assert_eq!(outcome.skipped, 0, "nothing raced here -- no column should be skipped");

        let worst_gap_ms = worst_gap_ms.load(Ordering::Relaxed);
        assert!(
            worst_gap_ms < 500,
            "background embedding work must not block the single-worker async runtime \
             for more than 500ms between 10ms heartbeat ticks — observed worst gap \
             {worst_gap_ms}ms. If this fails, embed_passages_chunked's spawn_blocking \
             offload has regressed and the CPU-bound embedding forward pass is running \
             directly on the tokio worker thread again (KYO-644)."
        );
    }
}
