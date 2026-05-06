// SPDX-License-Identifier: AGPL-3.0-or-later

//! Core types for the catalog indexing system.
//!
//! Mirrors Python's `CatalogIndexResult`, `SearchEntry`, and related types.

use serde::{Deserialize, Serialize};

/// Result of a catalog indexing operation.
///
/// Matches Python's `CatalogIndexResult` dataclass.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogIndexResult {
    /// Status: "completed", "skipped", "error", "running".
    pub status: String,
    /// Number of tables indexed during this run.
    pub tables_indexed: usize,
    /// Number of tables archived (no longer seen on server).
    pub tables_archived: usize,
    /// Error messages encountered during indexing.
    pub errors: Option<Vec<String>>,
    /// ISO 8601 timestamp when indexing started.
    pub start_time: Option<String>,
    /// ISO 8601 timestamp when indexing finished.
    pub end_time: Option<String>,
    /// Datasource config ID this result is for.
    pub datasource_config_id: Option<String>,
    /// Workspace ID this result is for.
    pub workspace_id: Option<String>,
}

impl CatalogIndexResult {
    /// Create a "completed" result.
    pub fn completed(tables_indexed: usize, tables_archived: usize) -> Self {
        Self {
            status: "completed".into(),
            tables_indexed,
            tables_archived,
            errors: None,
            start_time: None,
            end_time: None,
            datasource_config_id: None,
            workspace_id: None,
        }
    }

    /// Create a "skipped" result with a reason.
    pub fn skipped(reason: &str) -> Self {
        Self {
            status: "skipped".into(),
            tables_indexed: 0,
            tables_archived: 0,
            errors: Some(vec![reason.to_string()]),
            start_time: None,
            end_time: None,
            datasource_config_id: None,
            workspace_id: None,
        }
    }

    /// Create an "error" result.
    pub fn error(message: &str) -> Self {
        Self {
            status: "error".into(),
            tables_indexed: 0,
            tables_archived: 0,
            errors: Some(vec![message.to_string()]),
            start_time: None,
            end_time: None,
            datasource_config_id: None,
            workspace_id: None,
        }
    }

    /// Set the timestamps on this result.
    pub fn with_times(mut self, start: &str, end: &str) -> Self {
        self.start_time = Some(start.to_string());
        self.end_time = Some(end.to_string());
        self
    }

    /// Set the datasource and workspace IDs on this result.
    pub fn with_ids(mut self, datasource_config_id: &str, workspace_id: &str) -> Self {
        self.datasource_config_id = Some(datasource_config_id.to_string());
        self.workspace_id = Some(workspace_id.to_string());
        self
    }
}

/// A single search entry for embedding generation.
///
/// Each entry represents one piece of searchable text (table name, column name,
/// description) with a weight that affects search ranking.
#[derive(Debug, Clone)]
pub struct SearchEntry {
    /// The searchable text content.
    pub text: String,
    /// Full table identifier (e.g., "schema.table" or "project.dataset.table").
    pub table_id: String,
    /// Entry type: "schema_table", "table_name", "column_name", "column_description".
    pub entry_type: String,
    /// Search ranking weight (1.0 = highest, 0.4 = lowest).
    pub weight: f64,
    /// Column name, for column-related entries only.
    pub column_name: Option<String>,
}

/// A table entry returned by container discovery.
#[derive(Debug, Clone)]
pub struct TableEntry {
    /// Table name.
    pub name: String,
    /// Table type (e.g., "TABLE", "VIEW"). Optional, defaults to "TABLE".
    pub table_type: Option<String>,
    /// Override for the dataset_id used in caching.
    ///
    /// When set, this value is used instead of the container name as the
    /// `dataset_id` in `cache_table`. Used by datasources with multi-level
    /// container hierarchies (e.g., Snowflake: `"database.schema"`,
    /// Databricks: `"catalog.schema"`).
    pub dataset_override: Option<String>,
}

/// A column entry returned by table column inspection.
#[derive(Debug, Clone)]
pub struct ColumnEntry {
    /// Column name.
    pub name: String,
    /// Column type (e.g., "VARCHAR", "INTEGER"). Optional.
    pub col_type: Option<String>,
    /// Native database type (before simplification). Optional.
    pub native_type: Option<String>,
    /// Column description from database comments/docs. Optional.
    pub description: Option<String>,
}
