// SPDX-License-Identifier: AGPL-3.0-or-later

//! User model — maps to the `users` table.
//!
//! Matches Python's `User` model in `database/models.py`.
//! User IDs are `String` (VARCHAR 50), format `"user-{token_urlsafe(16)}"`.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Core user record from the `users` table.
#[derive(Debug, Clone, sqlx::FromRow, Serialize, Deserialize)]
pub struct User {
    /// Primary key — format: `"user-{token_urlsafe(16)}"`
    pub user_id: String,

    /// Unique email address.
    pub email: String,

    /// Display name (nullable in DB, but defaults to empty string).
    pub name: Option<String>,

    /// Account creation timestamp.
    pub created_at: DateTime<Utc>,

    /// Last profile update timestamp.
    pub updated_at: DateTime<Utc>,

    /// Last login timestamp.
    pub last_login: Option<DateTime<Utc>>,

    /// Whether the account is active.
    pub active: bool,

    /// Whether email has been verified.
    pub verified: bool,

    /// When terms of service were accepted.
    pub terms_accepted_at: Option<DateTime<Utc>>,

    /// Version of terms accepted.
    pub terms_accepted_version: Option<String>,

    /// Whether user consented to marketing emails.
    pub marketing_consent: bool,

    /// Encrypted OAuth data (Google OAuth tokens).
    /// Stored as AES-256-GCM encrypted text in the DB.
    /// Phase 2 does not read/write this field — encryption/decryption
    /// is wired in Phase 3 (Google OAuth) via `kyomi_auth::encryption`.
    pub oauth_data: Option<String>,

    /// Flexible JSON metadata (roles, settings).
    pub extra_metadata: Option<serde_json::Value>,

    /// ChartML configuration for user-level chart styling.
    pub chartml_config: Option<serde_json::Value>,

    /// Last workspace accessed (for workspace selection on login).
    pub last_workspace_id: Option<String>,

    /// User's personal knowledge document (markdown).
    pub knowledge: Option<String>,

    /// BigQuery project to bill queries to.
    pub billing_project: Option<String>,

    /// Default BigQuery project for queries.
    pub default_project: Option<String>,

    /// AI Agent query size limit in GB.
    pub query_size_limit_gb: i32,
}

impl User {
    /// Extract roles from extra_metadata, defaulting to `["user"]`.
    pub fn roles(&self) -> Vec<String> {
        self.extra_metadata
            .as_ref()
            .and_then(|m| m.get("roles"))
            .and_then(|r| serde_json::from_value::<Vec<String>>(r.clone()).ok())
            .unwrap_or_else(|| vec!["user".to_string()])
    }
}
