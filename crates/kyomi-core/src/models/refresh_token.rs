// SPDX-License-Identifier: AGPL-3.0-or-later

//! RefreshToken model — maps to the `refresh_tokens` table.
//!
//! Refresh tokens are opaque `rt_<base64url(32 bytes)>` strings.
//! Stored as SHA-256 hashes for efficient lookup of high-entropy tokens.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Refresh token record.
#[derive(Debug, Clone, sqlx::FromRow, Serialize, Deserialize)]
pub struct RefreshToken {
    /// Primary key — format: `"rt_{token_urlsafe(16)}"`.
    pub token_id: String,

    /// Foreign key to `users.user_id`.
    pub user_id: String,

    /// SHA-256 hash of the opaque token value.
    pub token_hash: String,

    /// DEMO mode only: stores unhashed token for e2e testing.
    pub demo_token_value: Option<String>,

    /// When this token expires.
    pub expires_at: DateTime<Utc>,

    /// Whether this token is still active (not revoked).
    pub is_active: bool,

    /// When this token was revoked (audit trail).
    pub revoked_at: Option<DateTime<Utc>>,

    /// When this token was created.
    pub created_at: DateTime<Utc>,

    /// When this token was last used.
    pub last_used: Option<DateTime<Utc>>,

    /// Browser user agent string.
    pub user_agent: Option<String>,

    /// Client IP address.
    pub ip_address: Option<String>,

    /// Client country code (from Cloudflare).
    pub country_code: Option<String>,

    /// OAuth client ID (for API token grants).
    pub oauth_client_id: Option<String>,
}
