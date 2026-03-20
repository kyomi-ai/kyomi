// SPDX-License-Identifier: AGPL-3.0-or-later

//! Chart model — maps to the `charts` table.
//!
//! Charts are stored separately from message content for independent updates.
//! Chart IDs are UUIDs stored as `String`.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// A chart record from the `charts` table.
#[derive(Debug, Clone, sqlx::FromRow, Serialize, Deserialize)]
pub struct Chart {
    /// Primary key — UUID string.
    pub chart_id: String,

    /// FK to chat_messages table.
    pub message_id: String,

    /// Chart metadata (SQL query, type, config, etc.) as JSON.
    pub chart_data: serde_json::Value,

    /// Record creation timestamp.
    pub created_at: DateTime<Utc>,

    /// Last modification timestamp.
    pub updated_at: DateTime<Utc>,
}
