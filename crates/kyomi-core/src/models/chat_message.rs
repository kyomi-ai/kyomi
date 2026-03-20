// SPDX-License-Identifier: AGPL-3.0-or-later

//! ChatMessage model — maps to the `chat_messages` table.
//!
//! Matches Python's `ChatMessage` model in `database/models.py`.
//! Message IDs are UUIDs stored as `String` (VARCHAR 50).
//!
//! **Encryption note**: `content` and `extra_metadata` are stored as encrypted
//! TEXT in the database (AES-256-GCM via `EncryptedText` / `EncryptedJSON`
//! column types in the Python backend). In this Rust model they are plain
//! `String` fields that hold the ciphertext. Encryption and decryption happen
//! at the service layer, NOT in the model — matching how credential encryption
//! works in the existing codebase.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::enums::ChatMessageRole;

/// A chat message record from the `chat_messages` table.
#[derive(Debug, Clone, sqlx::FromRow, Serialize, Deserialize)]
pub struct ChatMessage {
    /// Primary key — UUID string.
    pub message_id: String,

    /// FK to chat_sessions table.
    pub session_id: String,

    /// Message role: "user", "assistant", "system", "tool".
    pub role: ChatMessageRole,

    /// Message content — stored as AES-256-GCM encrypted TEXT in the DB.
    /// This field holds the ciphertext; decrypt at the service layer.
    pub content: String,

    /// User who sent this message (for shared conversation attribution).
    /// NULL for assistant/system/tool messages.
    pub sent_by_user_id: Option<String>,

    /// Whether this message is pinned. Server default is false.
    #[sqlx(default)]
    pub pinned: bool,

    /// Message creation timestamp.
    pub created_at: DateTime<Utc>,

    /// User's current time in their timezone (ISO format string with offset).
    /// Used for relative time queries like "last month".
    pub current_time_user_tz: Option<String>,

    /// Additional metadata — stored as AES-256-GCM encrypted TEXT in the DB.
    /// This field holds the ciphertext; decrypt to JSON at the service layer.
    pub extra_metadata: Option<String>,

    /// For role='tool' messages: links result back to the assistant's tool call.
    pub tool_call_id: Option<String>,

    /// For role='tool' messages: name of the tool that was called.
    pub tool_name: Option<String>,

    /// For role='assistant' messages: list of tool calls [{id, name, arguments}, ...].
    /// Stored as JSON (not encrypted).
    pub tool_calls: Option<serde_json::Value>,
}
