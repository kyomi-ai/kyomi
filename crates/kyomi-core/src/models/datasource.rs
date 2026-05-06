// SPDX-License-Identifier: AGPL-3.0-or-later

//! Datasource models — maps to the `datasource_configs`, `user_datasource_credentials`,
//! and `user_datasource_preferences` tables.
//!
//! Matches Python's models in `database/models.py`.
//!
//! - `DatasourceConfig.id` format: `"ds-{uuid}"`
//! - `UserDatasourceCredential.credentials` is stored as AES-256-GCM encrypted text
//!   in a `TEXT` column. Decryption happens in the service layer, not here.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::enums::DatasourceType;

/// Workspace-level datasource configuration from the `datasource_configs` table.
#[derive(Debug, Clone, sqlx::FromRow, Serialize, Deserialize)]
pub struct DatasourceConfig {
    /// Primary key — format: `"ds-{uuid}"`
    pub id: String,

    /// FK to workspaces table.
    pub workspace_id: String,

    /// Human-readable name (e.g., "Production PostgreSQL").
    pub name: String,

    /// URL-friendly slug (e.g., "production-postgres").
    pub slug: String,

    /// Provider type (e.g., "postgres", "bigquery", "clickhouse").
    pub datasource_type: DatasourceType,

    /// Provider-specific connection parameters (JSON).
    pub connection_config: serde_json::Value,

    /// Whether this datasource is active and available for queries.
    pub active: bool,

    /// Connection type: "direct" (credentials in Kyomi) or "connect" (Kyomi Connect).
    pub connection_type: String,

    /// JWT token ID for Connect datasources (used for token revocation).
    /// NULL for direct connections.
    pub connect_token_jti: Option<String>,

    /// Record creation timestamp.
    pub created_at: DateTime<Utc>,

    /// Last modification timestamp.
    pub updated_at: DateTime<Utc>,

    /// When the catalog was last refreshed (null if never).
    pub last_catalog_refresh: Option<DateTime<Utc>>,

    /// When the most recent catalog indexing run *started* (null if never).
    ///
    /// Stamped at the top of [`CatalogIndexingService::index_datasource`]
    /// and checked by callers to skip concurrent runs — if an indexing run
    /// started within the last hour, new runs are rejected unless `force`
    /// is set. Complements [`last_catalog_refresh`] (which is the *finish*
    /// timestamp): the two gates are orthogonal — finish guards against
    /// "just finished, don't re-index", start guards against "just started,
    /// don't double up". Self-healing on panic: the stamp ages out after
    /// 60 minutes so a crashed run is retried automatically.
    ///
    /// [`CatalogIndexingService::index_datasource`]: (see crate `kyomi-agent`)
    /// [`last_catalog_refresh`]: Self::last_catalog_refresh
    pub last_index_started_at: Option<DateTime<Utc>>,

    /// Whether charts are allowed to auto-refresh with this datasource.
    pub auto_refresh_allowed: bool,
}

/// Per-user encrypted credentials for a datasource from the
/// `user_datasource_credentials` table.
#[derive(Debug, Clone, sqlx::FromRow, Serialize, Deserialize)]
pub struct UserDatasourceCredential {
    /// Primary key (autoincrement).
    pub id: i32,

    /// FK to users table.
    pub user_id: String,

    /// FK to datasource_configs table.
    pub datasource_config_id: String,

    /// FK to workspaces table.
    pub workspace_id: String,

    /// Encrypted credentials (AES-256-GCM base64url-encoded text).
    /// Decrypt with `kyomi_auth::encryption::decrypt_json`.
    pub credentials: String,

    /// Whether this credential is enabled for the user.
    pub enabled: bool,

    /// Record creation timestamp.
    pub created_at: DateTime<Utc>,

    /// Last modification timestamp.
    pub updated_at: DateTime<Utc>,
}

/// Per-user datasource preference (enabled/disabled) from the
/// `user_datasource_preferences` table.
///
/// Used for shared-auth datasources where the user does not have individual
/// credentials — they just toggle on/off.
#[derive(Debug, Clone, sqlx::FromRow, Serialize, Deserialize)]
pub struct UserDatasourcePreference {
    /// Primary key (autoincrement).
    pub id: i32,

    /// FK to users table.
    pub user_id: String,

    /// FK to datasource_configs table.
    pub datasource_config_id: String,

    /// Whether this datasource is enabled for the user.
    pub enabled: bool,

    /// Record creation timestamp.
    pub created_at: DateTime<Utc>,

    /// Last modification timestamp.
    pub updated_at: DateTime<Utc>,
}
