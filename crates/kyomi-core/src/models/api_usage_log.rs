// SPDX-License-Identifier: AGPL-3.0-or-later

//! API usage log model — maps to the `api_usage_log` table.
//!
//! Records individual LLM API calls for billing and usage tracking.
//! Each row represents one call to an AI provider (Anthropic, etc.).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// An individual LLM API call record for billing/usage tracking.
///
/// The `cost_estimate` field (in USD) is the primary billing metric.
/// Token counts are retained for detailed analytics.
#[derive(Debug, Clone, sqlx::FromRow, Serialize, Deserialize)]
pub struct ApiUsageLog {
    /// Auto-increment primary key.
    pub id: i32,

    /// FK to users table.
    pub user_id: String,

    /// FK to workspaces table.
    pub workspace_id: String,

    /// FK to chat_sessions table (nullable — not all calls are chat-scoped).
    pub session_id: Option<String>,

    /// When the API call was made.
    pub timestamp: DateTime<Utc>,

    /// LLM provider name (e.g. "anthropic", "openai").
    pub provider: String,

    /// Model identifier (e.g. "claude-3-haiku").
    pub model: String,

    /// Number of input tokens.
    pub input_tokens: i32,

    /// Number of output tokens.
    pub output_tokens: i32,

    /// Total tokens (input + output).
    pub total_tokens: i32,

    /// Prompt cache creation tokens (Anthropic-specific).
    #[sqlx(default)]
    pub cache_creation_input_tokens: i32,

    /// Prompt cache read tokens (Anthropic-specific).
    #[sqlx(default)]
    pub cache_read_input_tokens: i32,

    /// Estimated cost in USD for this call.
    pub cost_estimate: Option<f64>,

    /// Component that made the call (e.g. "chat_agent", "sql_copilot", "kyomi_watch").
    pub component: Option<String>,

    /// Unique request identifier for tracing.
    pub request_id: Option<String>,

    /// Additional metadata JSON blob.
    pub extra_metadata: Option<serde_json::Value>,
}
