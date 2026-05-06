// SPDX-License-Identifier: AGPL-3.0-or-later

//! Push subscription model — maps to the `push_subscriptions` table.
//!
//! Stores Web Push (VAPID) subscriptions for individual browser/device registrations.
//! Subscriptions belong to users, not workspaces.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// A Web Push subscription from a user's browser/device.
#[derive(Debug, Clone, sqlx::FromRow, Serialize, Deserialize)]
pub struct PushSubscription {
    /// Auto-incrementing primary key.
    pub id: i32,

    /// FK to users table.
    pub user_id: String,

    /// Push service endpoint URL (unique per device).
    pub endpoint: String,

    /// Base64url-encoded client public key (P-256 ECDH).
    pub p256dh: String,

    /// Base64url-encoded authentication secret.
    pub auth: String,

    /// User-Agent string from the subscribing browser.
    pub user_agent: Option<String>,

    /// User-friendly device label (e.g., "Chrome on macOS").
    pub device_label: Option<String>,

    /// When the subscription was created.
    pub created_at: DateTime<Utc>,

    /// When a push was last successfully delivered to this subscription.
    pub last_used_at: Option<DateTime<Utc>>,

    /// Consecutive delivery failure count. Reset to 0 on success.
    pub failure_count: i32,
}
