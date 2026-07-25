// SPDX-License-Identifier: AGPL-3.0-or-later

//! Watch models — maps to `watches` and `watch_executions` tables.
//!
//! Watches are AI-powered data monitors that run on a cron schedule,
//! querying datasources and alerting users when conditions are met.
//! Executions track each run with status, response, and cost data.
//!
//! Note: `watch_id` uses the `"watch-{uuid}"` format (generated at insert time).
//! Executions snapshot `watch_name`, `mode`, and `workspace_id` so they survive
//! watch deletion.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::enums::{WatchExecutionStatus, WatchMode};

/// A watch from the `watches` table.
#[derive(Debug, Clone, sqlx::FromRow, Serialize, Deserialize)]
pub struct Watch {
    /// Primary key — `"watch-{uuid}"` format.
    pub watch_id: String,

    /// FK to workspaces table — workspace isolation.
    pub workspace_id: String,

    /// FK to users table — who created this watch.
    pub created_by: String,

    /// Short descriptive name (3–255 characters).
    pub name: String,

    /// The monitoring instruction for the AI agent.
    pub prompt: String,

    /// Cron expression (5 fields, UTC).
    pub schedule: String,

    /// Watch mode: `"alert"` (conditional) or `"report"` (always sends).
    pub mode: WatchMode,

    /// Optional datasource filter: `{ "datasources": ["slug1", "slug2"] }`.
    pub datasource_hints: Option<serde_json::Value>,

    /// Optional reference queries: `[{ "comment": "...", "sql": "...", "datasource": "..." }]`.
    pub queries: Option<serde_json::Value>,

    /// Optional comma-separated email addresses for alerts.
    pub alert_emails: Option<String>,

    /// Whether email alerts are enabled.
    pub alert_emails_enabled: bool,

    /// Whether the watch is active (scheduler only runs enabled watches).
    pub enabled: bool,

    /// When the watch last ran.
    pub last_run_at: Option<DateTime<Utc>>,

    /// Status of the last run: `"success"`, `"error"`, `"no_alert"`.
    pub last_run_status: Option<WatchExecutionStatus>,

    /// When the watch should next run (computed from cron schedule).
    pub next_run_at: Option<DateTime<Utc>>,

    /// Record creation timestamp.
    pub created_at: DateTime<Utc>,

    /// Last update timestamp.
    pub updated_at: DateTime<Utc>,
}

/// A watch execution record from the `watch_executions` table.
#[derive(Debug, Clone, sqlx::FromRow, Serialize, Deserialize)]
pub struct WatchExecution {
    /// Primary key — auto-incrementing integer.
    pub id: i32,

    /// FK to watches table (nullable — preserved after watch deletion).
    pub watch_id: Option<String>,

    /// Snapshot of the watch name at execution time (nullable in DB).
    pub watch_name: Option<String>,

    /// Snapshot of the watch mode at execution time (nullable in DB).
    pub mode: Option<WatchMode>,

    /// Snapshot of workspace ID (for authorization after watch deletion, nullable in DB).
    pub workspace_id: Option<String>,

    /// Snapshot of the watch owner's user ID (for ownership filtering after
    /// watch deletion — `watch_id` alone can't be joined back to `watches`
    /// once the parent watch is gone). Nullable: rows whose parent watch was
    /// deleted before this column existed have no way to recover ownership.
    pub created_by: Option<String>,

    /// FK to chat_sessions table (optional — the session used for execution).
    pub session_id: Option<String>,

    /// When the execution started.
    pub started_at: DateTime<Utc>,

    /// When the execution completed (null while running).
    pub completed_at: Option<DateTime<Utc>>,

    /// Execution status: `"running"`, `"success"`, `"error"`, `"no_alert"`.
    pub status: WatchExecutionStatus,

    /// The AI agent's response text.
    pub agent_response: Option<String>,

    /// Error message if execution failed.
    pub error_message: Option<String>,

    /// Number of input tokens consumed (NOT NULL, default 0).
    pub input_tokens: i32,

    /// Number of output tokens generated (NOT NULL, default 0).
    pub output_tokens: i32,

    /// Estimated cost in dollars (maps to DOUBLE PRECISION, not NUMERIC).
    pub cost_estimate: Option<f64>,

    /// Detailed execution trace (tool calls, intermediate results).
    pub execution_trace: Option<serde_json::Value>,

    /// Whether this execution triggered an alert notification.
    pub alert_triggered: bool,

    /// ID of the notification sent (e.g., Slack message ts).
    pub notification_id: Option<String>,

    /// When the alert was first read by a user.
    pub read_at: Option<DateTime<Utc>>,

    /// When the alert was soft-deleted.
    pub deleted_at: Option<DateTime<Utc>>,

    /// Who soft-deleted the alert.
    pub deleted_by: Option<String>,
}
