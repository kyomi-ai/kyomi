// SPDX-License-Identifier: AGPL-3.0-or-later

//! SQL query history model — maps to the `sql_query_history` table.
//!
//! Matches Python's `SqlQueryHistory` model in `database/models.py`.
//! Query IDs are UUIDs stored as `String` (VARCHAR).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// A recorded SQL query execution from the `sql_query_history` table.
#[derive(Debug, Clone, sqlx::FromRow, Serialize, Deserialize)]
pub struct SqlQueryHistory {
    /// Primary key — UUID string.
    pub query_id: String,

    /// FK to workspaces table.
    pub workspace_id: String,

    /// FK to users table.
    pub user_id: String,

    /// FK to datasource_configs table (nullable — datasource may have been deleted).
    pub datasource_config_id: Option<String>,

    /// The SQL query text that was executed.
    pub query_text: String,

    /// When the query was executed.
    pub executed_at: DateTime<Utc>,

    /// Wall-clock execution time in milliseconds.
    pub execution_time_ms: Option<i32>,

    /// Bytes processed by the query engine (BigQuery, etc.).
    pub bytes_processed: Option<i64>,

    /// Number of rows returned.
    pub row_count: Option<i32>,

    /// Execution status: `"success"` or `"error"`.
    pub status: String,

    /// Error message if status is `"error"`.
    pub error_message: Option<String>,

    /// Whether the user has saved/bookmarked this query.
    pub is_saved: bool,

    /// User-assigned name for saved queries.
    pub query_name: Option<String>,

    /// Comma-separated tags for categorization.
    pub tags: Option<String>,

    /// Record creation timestamp.
    pub created_at: DateTime<Utc>,

    /// Last modification timestamp.
    pub updated_at: DateTime<Utc>,
}
