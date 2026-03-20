// SPDX-License-Identifier: AGPL-3.0-or-later

//! UserAuthMethod model — maps to the `user_auth_methods` table.
//!
//! Each user can have multiple auth methods (password, webauthn, google_oauth).
//! Unique constraint on (user_id, auth_type).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Authentication method record.
#[derive(Debug, Clone, sqlx::FromRow, Serialize, Deserialize)]
pub struct UserAuthMethod {
    /// Auto-increment primary key.
    pub id: i32,

    /// Foreign key to `users.user_id`.
    pub user_id: String,

    /// Auth type: `"password"`, `"webauthn"`, `"google_oauth"`.
    pub auth_type: String,

    /// Method-specific data (JSON). E.g., password hash, passkey credentials.
    pub auth_data: serde_json::Value,

    /// When this auth method was created.
    pub created_at: DateTime<Utc>,

    /// When this auth method was last used.
    pub last_used: Option<DateTime<Utc>>,

    /// Whether this auth method is active.
    pub active: bool,
}
