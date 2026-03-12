// SPDX-License-Identifier: AGPL-3.0-or-later

//! Datasource search embedding model — maps to the `datasource_search_embeddings` table.
//!
//! Stores individual search entries (table names, descriptions, column names, etc.)
//! with pre-computed 384-dim embeddings for semantic search via pgvector.
//!
//! Entry types: `dataset_table`, `table_description`, `column_name`, `column_description`.
//! Weights range from 0.4 (column descriptions) to 1.0 (dataset_table identifiers).
//!
//! ## Deprecation Notice
//!
//! This table has been replaced by pgvector embeddings on `datasource_table_cache`
//! + `column_embeddings` via `kyomi-knowledge`. The `datasource_search_embeddings`
//! table is being dropped.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// A single search embedding entry from the `datasource_search_embeddings` table.
#[derive(Debug, Clone, sqlx::FromRow, Serialize, Deserialize)]
pub struct DatasourceSearchEmbedding {
    /// Auto-increment primary key.
    pub id: i32,

    /// FK to datasource_table_cache — which cached table this entry belongs to.
    pub table_cache_id: i32,

    /// FK to workspaces table — denormalized for fast workspace-scoped queries.
    pub workspace_id: String,

    /// FK to datasource_configs table (nullable for backward compat during migration).
    pub datasource_config_id: Option<String>,

    /// Project/database identifier (denormalized for fast access).
    pub project_id: String,

    /// Dataset/schema identifier (denormalized for fast access).
    pub dataset_id: String,

    /// Table name (denormalized for fast access).
    pub table_id: String,

    /// Type of search entry: `dataset_table`, `table_description`, `column_name`,
    /// `column_description`.
    pub entry_type: String,

    /// The actual text being searched (e.g., table name, column description).
    pub text: String,

    /// Weight for ranking (0.4–1.0). Higher = more important in search results.
    pub weight: f64,

    /// Column name if this is a column-level entry, `None` for table-level entries.
    pub column_name: Option<String>,

    /// 384-dimension embedding vector (all-MiniLM-L6-v2).
    /// Stored as raw f32 little-endian bytes.
    pub embedding: Vec<u8>,

    /// Record creation timestamp.
    pub created_at: DateTime<Utc>,
}
