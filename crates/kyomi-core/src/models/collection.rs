// SPDX-License-Identifier: AGPL-3.0-or-later

//! Collection models — maps to `collections` and `collection_dashboards` tables.
//!
//! Collections group dashboards for organization. The junction table
//! `collection_dashboards` uses a composite PK (collection_id, dashboard_id)
//! with CASCADE deletes.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// A collection from the `collections` table.
#[derive(Debug, Clone, sqlx::FromRow, Serialize, Deserialize)]
pub struct Collection {
    /// Primary key — UUID string.
    pub id: String,

    /// FK to workspaces table — workspace isolation.
    pub workspace_id: String,

    /// FK to users table — who created this collection.
    pub created_by: String,

    /// Collection name (1–255 characters).
    pub name: String,

    /// Optional description text.
    pub description: Option<String>,

    /// Optional hex color code (#RRGGBB).
    pub color: Option<String>,

    /// Whether the collection is visible to all workspace members.
    pub is_public: bool,

    /// Record creation timestamp.
    pub created_at: DateTime<Utc>,

    /// Last update timestamp.
    pub updated_at: DateTime<Utc>,
}

/// A junction row from the `collection_dashboards` table.
#[derive(Debug, Clone, sqlx::FromRow, Serialize, Deserialize)]
pub struct CollectionDashboard {
    /// FK to collections (CASCADE delete).
    pub collection_id: String,

    /// FK to dashboards (CASCADE delete).
    pub dashboard_id: String,

    /// Ordering position within the collection.
    pub position: i32,

    /// When the dashboard was added to the collection.
    pub added_at: DateTime<Utc>,
}
