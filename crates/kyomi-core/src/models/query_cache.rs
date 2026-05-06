// SPDX-License-Identifier: AGPL-3.0-or-later

//! QueryCache model — maps to the `query_cache` table.
//!
//! Temporary SQL query cache for chart data with auto-cleanup after 60 days.
//! Query IDs are SHA-256 hashes (64-char hex strings).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// A cached SQL query from the `query_cache` table.
#[derive(Debug, Clone, sqlx::FromRow, Serialize, Deserialize)]
pub struct QueryCache {
    /// Primary key — SHA-256 hash of the SQL query (64-char hex string).
    pub query_id: String,

    /// The SQL query text.
    pub sql: String,

    /// Last time this cached query was accessed (for cleanup).
    pub last_accessed_at: DateTime<Utc>,

    /// Record creation timestamp.
    pub created_at: DateTime<Utc>,
}
