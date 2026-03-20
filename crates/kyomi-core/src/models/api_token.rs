// SPDX-License-Identifier: AGPL-3.0-or-later

//! API token model — maps to `api_tokens` table.
//!
//! Used for programmatic API access (MCP server, external integrations).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// An API token for programmatic access.
///
/// The raw token is never stored — only its SHA-256 hash (`token_hash`).
/// Tokens can be revoked and have optional expiration.
#[derive(Debug, Clone, sqlx::FromRow, Serialize, Deserialize)]
pub struct ApiToken {
    /// Primary key.
    pub token_id: String,

    /// Owner of the token.
    pub user_id: String,

    /// Human-readable name for the token.
    pub name: String,

    /// SHA-256 hash of the raw token value.
    pub token_hash: String,

    /// Whether the token is currently active.
    pub active: bool,

    /// When the token was created.
    pub created_at: DateTime<Utc>,

    /// Optional expiration timestamp.
    pub expires_at: Option<DateTime<Utc>>,

    /// When the token was last used for authentication.
    pub last_used: Option<DateTime<Utc>>,

    /// When the token was revoked (NULL if still active).
    pub revoked_at: Option<DateTime<Utc>>,

    /// Email of the admin who created the token.
    pub created_by: Option<String>,

    /// Email of the admin who revoked the token.
    pub revoked_by: Option<String>,
}
