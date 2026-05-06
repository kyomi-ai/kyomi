// SPDX-License-Identifier: AGPL-3.0-or-later

//! EmailSubscriber model — maps to the `email_subscribers` table.
//!
//! Stores newsletter/waitlist email subscriptions with company info
//! and marketing consent tracking.

use chrono::NaiveDateTime;
use serde::{Deserialize, Serialize};

/// An email subscriber record.
///
/// Maps to the `email_subscribers` table. Uses `SERIAL` (i32) primary key
/// and `TIMESTAMP` (without timezone) columns matching the original schema.
#[derive(Debug, Clone, sqlx::FromRow, Serialize, Deserialize)]
pub struct EmailSubscriber {
    /// Auto-increment primary key.
    pub id: i32,

    /// Subscriber's email address (unique).
    pub email: String,

    /// Optional company name.
    pub company_name: Option<String>,

    /// Optional company size category (e.g. "1-10", "11-50").
    pub company_size: Option<String>,

    /// Optional use case description.
    pub use_case: Option<String>,

    /// Whether the subscriber consented to marketing emails.
    pub marketing_consent: bool,

    /// When the subscription was created.
    pub created_at: Option<NaiveDateTime>,

    /// When the subscription was last updated.
    pub updated_at: Option<NaiveDateTime>,

    /// Signup source (e.g. "web", "marketing_site", "beta_waitlist").
    pub source: Option<String>,

    /// Whether the subscriber has been notified of launch/updates.
    pub notified: bool,

    /// When the subscriber was notified.
    pub notified_at: Option<NaiveDateTime>,
}
