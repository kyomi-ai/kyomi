// SPDX-License-Identifier: AGPL-3.0-or-later

//! Dashboard models — maps to `dashboards`, `dashboard_views`, and `dashboard_versions` tables.
//!
//! Dashboards are markdown documents with embedded ChartML visualizations.
//! Views track popularity, versions track edit history with SHA-256 dedup.
//!
//! Note: `search_vector` (tsvector) is maintained by PG triggers and is intentionally
//! omitted — all queries use explicit column lists (not `SELECT *`).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Document type for the unified dashboards table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DocType {
    Dashboard,
    Knowledge,
}

impl DocType {
    /// SQL-storable string representation.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Dashboard => "dashboard",
            Self::Knowledge => "knowledge",
        }
    }

    /// Parse from database string. Returns `Dashboard` for unrecognized values.
    pub fn from_str_or_default(s: &str) -> Self {
        match s {
            "knowledge" => Self::Knowledge,
            _ => Self::Dashboard,
        }
    }

    pub fn is_dashboard(&self) -> bool {
        matches!(self, Self::Dashboard)
    }

    pub fn is_knowledge(&self) -> bool {
        matches!(self, Self::Knowledge)
    }
}

impl std::fmt::Display for DocType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A dashboard from the `dashboards` table.
///
/// Covers both traditional dashboards (`doc_type = "dashboard"`) and
/// knowledge documents (`doc_type = "knowledge"`).
#[derive(Debug, Clone, sqlx::FromRow, Serialize, Deserialize)]
pub struct Dashboard {
    /// Primary key — UUID string.
    pub dashboard_id: String,

    /// FK to users table — the dashboard owner.
    pub user_id: String,

    /// FK to workspaces table — workspace isolation.
    pub workspace_id: String,

    /// Dashboard title (3–255 characters).
    pub title: String,

    /// Markdown content with optional ChartML fenced blocks.
    pub content: String,

    /// Document type: "dashboard" or "knowledge".
    pub doc_type: String,

    /// SHA-256 hash of content (first 16 hex chars) for optimistic concurrency.
    pub content_hash: Option<String>,

    /// Summary of most recent changes (auto-generated or user-provided).
    pub last_change_summary: Option<String>,

    /// User ID who created this document.
    pub created_by: Option<String>,

    /// User ID who last updated this document.
    pub updated_by: Option<String>,

    /// 384-dimension embedding vector for semantic search.
    /// Stored as raw f32 little-endian bytes.
    #[sqlx(default)]
    pub embedding: Option<Vec<u8>>,

    /// Record creation timestamp.
    pub created_at: DateTime<Utc>,

    /// Last update timestamp.
    pub updated_at: DateTime<Utc>,
}

impl Dashboard {
    /// Parse the `doc_type` string field into a `DocType` enum.
    pub fn doc_type(&self) -> DocType {
        DocType::from_str_or_default(&self.doc_type)
    }
}

/// A dashboard view record from the `dashboard_views` table.
#[derive(Debug, Clone, sqlx::FromRow, Serialize, Deserialize)]
pub struct DashboardView {
    /// Primary key — `view-{hex20}` format.
    pub view_id: String,

    /// FK to dashboards table.
    pub dashboard_id: String,

    /// FK to users table — who viewed it.
    pub user_id: String,

    /// FK to workspaces table.
    pub workspace_id: String,

    /// When the view occurred.
    pub viewed_at: DateTime<Utc>,
}

/// A dashboard version snapshot from the `dashboard_versions` table.
#[derive(Debug, Clone, sqlx::FromRow, Serialize, Deserialize)]
pub struct DashboardVersion {
    /// Primary key — auto-incrementing integer.
    pub version_id: i32,

    /// FK to dashboards table.
    pub dashboard_id: String,

    /// FK to users table — who created this version.
    pub created_by: String,

    /// Monotonically increasing version number per dashboard.
    pub version_number: i32,

    /// Full content snapshot at this version.
    pub content: String,

    /// Title at this version.
    pub title: String,

    /// Human-readable description of changes.
    pub change_summary: Option<String>,

    /// SHA-256 hash of content for dedup.
    pub content_hash: Option<String>,

    /// Byte size of content.
    pub byte_size: Option<i32>,

    /// When this version was created.
    pub created_at: DateTime<Utc>,
}
