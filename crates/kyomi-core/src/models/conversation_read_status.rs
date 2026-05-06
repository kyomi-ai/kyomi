// SPDX-License-Identifier: AGPL-3.0-or-later

//! ConversationReadStatus model — maps to the `conversation_read_status` table.
//!
//! Tracks read/unread state per user per conversation for shared sessions.
//! Has a UNIQUE constraint on (session_id, user_id).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Read status record for a user in a shared conversation.
#[derive(Debug, Clone, sqlx::FromRow, Serialize, Deserialize)]
pub struct ConversationReadStatus {
    /// Auto-increment primary key.
    pub id: i32,

    /// FK to chat_sessions table.
    pub session_id: String,

    /// FK to users table.
    pub user_id: String,

    /// When the user last read this conversation.
    pub last_read_at: DateTime<Utc>,

    /// ID of the last message the user has read (nullable).
    pub last_read_message_id: Option<String>,
}
