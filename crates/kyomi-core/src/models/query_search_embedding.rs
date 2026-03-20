// SPDX-License-Identifier: AGPL-3.0-or-later

//! SQL query search embedding model — maps to the `sql_query_search_embeddings` table.
//!
//! Enables natural language search over query history (e.g., "show me revenue by month").
//! Each embedding indexes a normalized query text for semantic similarity search.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// A search embedding for a SQL query from the `sql_query_search_embeddings` table.
#[derive(Debug, Clone, sqlx::FromRow, Serialize, Deserialize)]
pub struct SqlQuerySearchEmbedding {
    /// Auto-increment primary key.
    pub id: i32,

    /// FK to sql_query_history — which query this embedding indexes.
    pub query_id: String,

    /// FK to workspaces table — denormalized for fast workspace-scoped queries.
    pub workspace_id: String,

    /// FK to users table — denormalized for fast user-scoped queries.
    pub user_id: String,

    /// The normalized query text that was embedded.
    pub search_text: String,

    /// 384-dimension embedding vector (all-MiniLM-L6-v2).
    /// Stored as raw f32 little-endian bytes.
    pub embedding: Vec<u8>,

    /// Record creation timestamp.
    pub created_at: DateTime<Utc>,
}
