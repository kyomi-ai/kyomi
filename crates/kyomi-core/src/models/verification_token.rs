// SPDX-License-Identifier: AGPL-3.0-or-later

//! VerificationToken model — maps to the `verification_tokens` table.
//!
//! Used for email verification during passkey signup flow.
//! Token values are bcrypt-hashed (low-entropy tokens need slow hash).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Email verification token record.
#[derive(Debug, Clone, sqlx::FromRow, Serialize, Deserialize)]
pub struct VerificationToken {
    /// Primary key — format: `"vt_{token_urlsafe(16)}"`.
    pub token_id: String,

    /// Email address this token is for.
    pub email: String,

    /// Bcrypt hash of the token value.
    pub token_hash: String,

    /// Token type: `"email_verification"` or `"password_reset"`.
    pub token_type: String,

    /// When this token expires.
    pub expires_at: DateTime<Utc>,

    /// Whether this token has been used.
    pub used: bool,

    /// When this token was created.
    pub created_at: DateTime<Utc>,

    /// When this token was used.
    pub used_at: Option<DateTime<Utc>>,
}
