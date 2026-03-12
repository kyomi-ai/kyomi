// SPDX-License-Identifier: AGPL-3.0-or-later

//! OAuthClient model — maps to the `oauth_clients` table.
//!
//! Registered OAuth clients (MCP clients like Cursor, Claude Desktop).
//! Public clients (no client_secret) use Dynamic Client Registration (RFC 7591).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// OAuth client record.
#[derive(Debug, Clone, sqlx::FromRow, Serialize, Deserialize)]
pub struct OAuthClient {
    /// Primary key (UUID).
    pub id: Uuid,

    /// Unique client identifier (e.g., "mcp-<random>").
    pub client_id: String,

    /// Optional hashed client secret (public clients have None).
    pub client_secret_hash: Option<String>,

    /// Display name (e.g., "Cursor IDE").
    pub name: String,

    /// Allowed redirect URIs (JSONB array).
    pub redirect_uris: serde_json::Value,

    /// Allowed scopes (JSONB array).
    pub scopes: serde_json::Value,

    /// Client type: "public" or "confidential".
    pub client_type: String,

    /// Whether this client is active.
    pub active: bool,

    /// When this client was registered.
    pub created_at: DateTime<Utc>,
}
