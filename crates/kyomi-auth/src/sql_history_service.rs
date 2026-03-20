// SPDX-License-Identifier: AGPL-3.0-or-later

//! SQL query history service — CRUD operations for the `sql_query_history` table.
//!
//! Wire-compatible with Python's SQL history endpoints.
//! All functions are stateless and take a DB pool reference.

use chrono::Utc;
use kyomi_core::models::SqlQueryHistory;
use kyomi_core::DbPool;

/// Create a new SQL query history record.
///
/// Returns the newly created record.
#[allow(clippy::too_many_arguments)]
pub async fn create_query_history(
    db: &DbPool,
    workspace_id: &str,
    user_id: &str,
    datasource_config_id: Option<&str>,
    query_text: &str,
    execution_time_ms: Option<i32>,
    bytes_processed: Option<i64>,
    row_count: Option<i32>,
    status: &str,
    error_message: Option<&str>,
) -> kyomi_core::Result<SqlQueryHistory> {
    let query_id = uuid::Uuid::new_v4().to_string();
    let now = Utc::now();

    let record = kyomi_core::db_fetch_one!(
        db,
        SqlQueryHistory,
        r#"INSERT INTO sql_query_history
         (query_id, workspace_id, user_id, datasource_config_id, query_text,
          executed_at, execution_time_ms, bytes_processed, row_count, status,
          error_message, is_saved, query_name, tags, created_at, updated_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, false, NULL, NULL, $12, $13)
         RETURNING query_id, workspace_id, user_id, datasource_config_id, query_text,
                   executed_at, execution_time_ms, bytes_processed,
                   row_count, status, error_message,
                   is_saved,
                   query_name, tags,
                   created_at, updated_at"#,
        &query_id,
        workspace_id,
        user_id,
        datasource_config_id,
        query_text,
        now,
        execution_time_ms,
        bytes_processed,
        row_count,
        status,
        error_message,
        now,
        now
    )?;

    Ok(record)
}

/// List SQL query history for a user in a workspace.
///
/// Returns tuples of `(SqlQueryHistory, Option<datasource_slug>)`.
/// Supports pagination, saved-only filtering, and text search on query_text.
pub async fn list_query_history(
    db: &DbPool,
    workspace_id: &str,
    user_id: &str,
    limit: i64,
    offset: i64,
    saved_only: bool,
    search: Option<&str>,
) -> kyomi_core::Result<Vec<(SqlQueryHistory, Option<String>)>> {
    let is_pg = db.is_postgres();
    let bool_true = kyomi_core::sql_compat::bool_true(is_pg);

    // Dynamic SQL — cannot use dispatch macros (conditional search filter + variable param count)
    let mut sql = String::from(
        "SELECT h.query_id, h.workspace_id, h.user_id, h.datasource_config_id, \
         h.query_text, h.executed_at, h.execution_time_ms, h.bytes_processed, \
         h.row_count, h.status, h.error_message, h.is_saved, h.query_name, \
         h.tags, h.created_at, h.updated_at, \
         dc.slug AS datasource_slug \
         FROM sql_query_history h \
         LEFT JOIN datasource_configs dc ON h.datasource_config_id = dc.id \
         WHERE h.workspace_id = $1 AND h.user_id = $2",
    );

    let mut param_index = 3;

    if saved_only {
        sql.push_str(&format!(" AND h.is_saved = {bool_true}"));
    }

    if search.is_some() {
        let ilike = kyomi_core::sql_compat::ilike(is_pg, "h.query_text", &format!("${param_index}"));
        sql.push_str(&format!(" AND {ilike}"));
        param_index += 1;
    }

    sql.push_str(" ORDER BY h.executed_at DESC");
    sql.push_str(&format!(" LIMIT ${param_index}"));
    param_index += 1;
    sql.push_str(&format!(" OFFSET ${param_index}"));

    let search_pattern = search.map(|s| format!("%{s}%"));

    // Use match db blocks for dynamic queries
    let rows: Vec<QueryHistoryRow> = if let Some(ref pattern) = search_pattern {
        match db {
            kyomi_core::db::DbPool::Postgres(pg) =>
                sqlx::query_as::<_, QueryHistoryRow>(&sql)
                    .bind(workspace_id)
                    .bind(user_id)
                    .bind(pattern)
                    .bind(limit)
                    .bind(offset)
                    .fetch_all(pg)
                    .await?,
            kyomi_core::db::DbPool::Sqlite(sq) =>
                sqlx::query_as::<_, QueryHistoryRow>(&sql)
                    .bind(workspace_id)
                    .bind(user_id)
                    .bind(pattern)
                    .bind(limit)
                    .bind(offset)
                    .fetch_all(sq)
                    .await?,
        }
    } else {
        match db {
            kyomi_core::db::DbPool::Postgres(pg) =>
                sqlx::query_as::<_, QueryHistoryRow>(&sql)
                    .bind(workspace_id)
                    .bind(user_id)
                    .bind(limit)
                    .bind(offset)
                    .fetch_all(pg)
                    .await?,
            kyomi_core::db::DbPool::Sqlite(sq) =>
                sqlx::query_as::<_, QueryHistoryRow>(&sql)
                    .bind(workspace_id)
                    .bind(user_id)
                    .bind(limit)
                    .bind(offset)
                    .fetch_all(sq)
                    .await?,
        }
    };

    Ok(rows
        .into_iter()
        .map(|r| {
            let slug = r.datasource_slug.clone();
            (r.into_history(), slug)
        })
        .collect())
}

/// Get a single SQL query history record.
///
/// Returns `(SqlQueryHistory, Option<datasource_slug>)`.
pub async fn get_query_history(
    db: &DbPool,
    query_id: &str,
    workspace_id: &str,
    user_id: &str,
) -> kyomi_core::Result<Option<(SqlQueryHistory, Option<String>)>> {
    let row = kyomi_core::db_fetch_optional!(
        db,
        QueryHistoryRow,
        r#"SELECT h.query_id, h.workspace_id, h.user_id, h.datasource_config_id,
           h.query_text, h.executed_at, h.execution_time_ms,
           h.bytes_processed, h.row_count, h.status, h.error_message,
           h.is_saved, h.query_name, h.tags,
           h.created_at, h.updated_at,
           dc.slug AS datasource_slug
         FROM sql_query_history h
         LEFT JOIN datasource_configs dc ON h.datasource_config_id = dc.id
         WHERE h.query_id = $1 AND h.workspace_id = $2 AND h.user_id = $3"#,
        query_id,
        workspace_id,
        user_id
    )?;

    Ok(row.map(|r| {
        let slug = r.datasource_slug.clone();
        (r.into_history(), slug)
    }))
}

/// Update a SQL query history record (is_saved, query_name, tags).
///
/// Returns the updated `(SqlQueryHistory, Option<datasource_slug>)`.
pub async fn update_query_history(
    db: &DbPool,
    query_id: &str,
    workspace_id: &str,
    user_id: &str,
    is_saved: Option<bool>,
    query_name: Option<&str>,
    tags: Option<&str>,
) -> kyomi_core::Result<Option<(SqlQueryHistory, Option<String>)>> {
    let now = Utc::now();

    // Dynamic SQL — cannot use dispatch macros (conditional SET clause + variable param count)
    let mut set_parts = Vec::new();
    let mut param_index = 4u32; // $1=query_id, $2=workspace_id, $3=user_id

    if is_saved.is_some() {
        set_parts.push(format!("is_saved = ${param_index}"));
        param_index += 1;
    }
    if query_name.is_some() {
        set_parts.push(format!("query_name = ${param_index}"));
        param_index += 1;
    }
    if tags.is_some() {
        set_parts.push(format!("tags = ${param_index}"));
        param_index += 1;
    }

    set_parts.push(format!("updated_at = ${param_index}"));

    if set_parts.is_empty() {
        // Nothing to update, just fetch
        return get_query_history(db, query_id, workspace_id, user_id).await;
    }

    let update_sql = format!(
        "UPDATE sql_query_history SET {} WHERE query_id = $1 AND workspace_id = $2 AND user_id = $3",
        set_parts.join(", ")
    );

    // Build and execute dynamically via match db
    match db {
        kyomi_core::db::DbPool::Postgres(pg) => {
            let mut query = sqlx::query(&update_sql)
                .bind(query_id)
                .bind(workspace_id)
                .bind(user_id);
            if let Some(saved) = is_saved {
                query = query.bind(saved);
            }
            if let Some(name) = query_name {
                query = query.bind(name);
            }
            if let Some(t) = tags {
                query = query.bind(t);
            }
            query = query.bind(now);
            query.execute(pg).await?;
        }
        kyomi_core::db::DbPool::Sqlite(sq) => {
            let mut query = sqlx::query(&update_sql)
                .bind(query_id)
                .bind(workspace_id)
                .bind(user_id);
            if let Some(saved) = is_saved {
                query = query.bind(saved);
            }
            if let Some(name) = query_name {
                query = query.bind(name);
            }
            if let Some(t) = tags {
                query = query.bind(t);
            }
            query = query.bind(now);
            query.execute(sq).await?;
        }
    }

    // Fetch the updated record with slug
    get_query_history(db, query_id, workspace_id, user_id).await
}

/// Delete a SQL query history record.
pub async fn delete_query_history(
    db: &DbPool,
    query_id: &str,
    workspace_id: &str,
    user_id: &str,
) -> kyomi_core::Result<bool> {
    let result = kyomi_core::db_execute!(
        db,
        "DELETE FROM sql_query_history \
         WHERE query_id = $1 AND workspace_id = $2 AND user_id = $3",
        query_id,
        workspace_id,
        user_id
    )?;

    Ok(result.rows_affected() > 0)
}

// ---------------------------------------------------------------------------
// Internal row type for joined queries
// ---------------------------------------------------------------------------

/// Internal row type that captures both the history fields and the joined slug.
#[derive(Debug, sqlx::FromRow)]
struct QueryHistoryRow {
    pub query_id: String,
    pub workspace_id: String,
    pub user_id: String,
    pub datasource_config_id: Option<String>,
    pub query_text: String,
    pub executed_at: chrono::DateTime<chrono::Utc>,
    pub execution_time_ms: Option<i32>,
    pub bytes_processed: Option<i64>,
    pub row_count: Option<i32>,
    pub status: String,
    pub error_message: Option<String>,
    pub is_saved: bool,
    pub query_name: Option<String>,
    pub tags: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    pub datasource_slug: Option<String>,
}

impl QueryHistoryRow {
    fn into_history(self) -> SqlQueryHistory {
        SqlQueryHistory {
            query_id: self.query_id,
            workspace_id: self.workspace_id,
            user_id: self.user_id,
            datasource_config_id: self.datasource_config_id,
            query_text: self.query_text,
            executed_at: self.executed_at,
            execution_time_ms: self.execution_time_ms,
            bytes_processed: self.bytes_processed,
            row_count: self.row_count,
            status: self.status,
            error_message: self.error_message,
            is_saved: self.is_saved,
            query_name: self.query_name,
            tags: self.tags,
            created_at: self.created_at,
            updated_at: self.updated_at,
        }
    }
}
