// SPDX-License-Identifier: AGPL-3.0-or-later

//! Feedback model — maps to the `feedback` table.
//!
//! Stores user feedback submissions with optional technical context.
//! Supports bug reports, feature requests, and questions.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::enums::{FeedbackStatus, FeedbackType};

/// A feedback submission from a user.
///
/// IDs use the `fb-{short_hex}` format matching the Python implementation.
#[derive(Debug, Clone, sqlx::FromRow, Serialize, Deserialize)]
pub struct Feedback {
    /// Primary key — format: `"fb-{hex}"`.
    pub id: String,

    /// FK to users table.
    pub user_id: String,

    /// FK to workspaces table (nullable — feedback can be workspace-scoped).
    pub workspace_id: Option<String>,

    /// Feedback type: "bug", "feature", "question".
    /// Database column is `type`, renamed to `feedback_type` to avoid Rust keyword.
    #[sqlx(rename = "type")]
    #[serde(rename = "type")]
    pub feedback_type: FeedbackType,

    /// User's description of the feedback.
    pub description: String,

    /// URL to an uploaded screenshot (nullable).
    pub screenshot_url: Option<String>,

    /// Whether the user consented to include technical context.
    /// Database column has `server_default=text("true")` so rows always have a value.
    pub include_context: bool,

    /// Technical context JSON blob (only populated if `include_context` is true).
    pub context: Option<serde_json::Value>,

    /// Status: "new", "reviewed", "resolved", "closed".
    pub status: FeedbackStatus,

    /// When the feedback was submitted.
    pub created_at: DateTime<Utc>,

    /// When the feedback was resolved (nullable).
    pub resolved_at: Option<DateTime<Utc>>,

    /// Notes from the resolver (nullable).
    pub resolution_notes: Option<String>,

    /// User ID of the person who resolved this feedback (nullable).
    pub resolved_by: Option<String>,
}

