// SPDX-License-Identifier: AGPL-3.0-or-later

//! Datasource table cache model — maps to the `datasource_table_cache` table.
//!
//! Stores cached table metadata for all datasource types (BigQuery, PostgreSQL,
//! ClickHouse, Snowflake, etc.) to enable fast catalog browsing and search
//! without hitting the actual datasource.
//!
//! Unique constraint: (workspace_id, datasource_config_id, project_id, dataset_id, table_id).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Cached table metadata from the `datasource_table_cache` table.
#[derive(Debug, Clone, sqlx::FromRow, Serialize, Deserialize)]
pub struct DatasourceTableCache {
    /// Auto-increment primary key.
    pub id: i32,

    /// FK to workspaces table — workspace isolation.
    pub workspace_id: String,

    /// FK to datasource_configs table (nullable for backward compat during migration).
    pub datasource_config_id: Option<String>,

    /// Project/database identifier (e.g., BigQuery project, PostgreSQL database).
    pub project_id: String,

    /// Dataset/schema identifier (e.g., BigQuery dataset, PostgreSQL schema).
    pub dataset_id: String,

    /// Table name.
    pub table_id: String,

    /// Full table metadata as JSON (columns, types, descriptions, row count, etc.).
    pub table_metadata: serde_json::Value,

    /// Column descriptions as JSON map: { "column_name": "description" }.
    pub column_descriptions: Option<serde_json::Value>,

    /// Record creation timestamp.
    pub created_at: DateTime<Utc>,

    /// Last modification timestamp.
    pub updated_at: DateTime<Utc>,

    /// When the table structure (columns, types) was last refreshed from the datasource.
    pub structure_refreshed_at: Option<DateTime<Utc>>,

    /// When column descriptions were last refreshed.
    pub descriptions_refreshed_at: Option<DateTime<Utc>>,

    /// Whether the table no longer exists in the datasource.
    pub is_archived: bool,

    /// Last time we confirmed the table still exists in the datasource.
    pub last_verified: Option<DateTime<Utc>>,
}
