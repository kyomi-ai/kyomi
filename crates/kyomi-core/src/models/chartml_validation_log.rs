// SPDX-License-Identifier: AGPL-3.0-or-later

//! ChartML validation log model — maps to the `chartml_validation_log` table.
//!
//! Records individual ChartML validation failures for prompt tuning and
//! observability. Each row represents one failed validation attempt, capturing
//! the raw response, error details, and whether a subsequent retry succeeded.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// A single ChartML validation failure record.
///
/// Written on every validation failure (YAML parse error, missing required
/// keys, SQL dry-run error). The `retry_succeeded` field is back-filled once
/// the session ends: `true` if a later retry produced valid ChartML, `false`
/// if the session exhausted retries without success, `None` while in-flight.
#[derive(Debug, Clone, sqlx::FromRow, Serialize, Deserialize)]
pub struct ChartMLValidationLog {
    /// Auto-increment primary key.
    pub id: i32,

    /// FK to chat_sessions (or a synthetic session ID for watch executions).
    pub session_id: String,

    /// FK to workspaces table.
    pub workspace_id: String,

    /// FK to users table.
    pub user_id: String,

    /// The raw LLM response that failed validation (may be large).
    pub raw_response: String,

    /// Human-readable validation error message (e.g. "Block 1: invalid YAML").
    pub error_message: String,

    /// Coarse error category for aggregation (e.g. "yaml_parse", "missing_key",
    /// "sql_error", "unknown").
    pub error_type: String,

    /// Zero-based retry attempt index within this session.
    pub retry_attempt: i32,

    /// Whether a subsequent retry for this session produced valid output.
    /// `None` while the session is still in-flight.
    pub retry_succeeded: Option<bool>,

    /// Which agent component produced the failure. One of `"chat"` (found
    /// within the tool-use loop and sent back to the model for a retry),
    /// `"chat_wrap_up"` (found in the forced wrap-up turn after the
    /// tool-use budget ran out — no retry is possible there, so the
    /// offending block(s) are stripped instead; see KYO-347), or `"watch"`.
    pub component: String,

    /// LLM model that generated the invalid response (e.g. "claude-sonnet-4-5").
    pub model: Option<String>,

    /// When the validation failure was recorded.
    pub created_at: DateTime<Utc>,
}
