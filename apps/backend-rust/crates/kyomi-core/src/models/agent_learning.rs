// SPDX-License-Identifier: AGPL-3.0-or-later

//! Agent learning model — maps to the `agent_learnings` table.
//!
//! Stores insights automatically captured by the AI agent, retrieved via
//! hybrid search (BM25 + pgvector semantic). Supports workspace-scoped and
//! user-scoped learnings, optional datasource binding, and supersession chains.
//!
//! Learning types: `navigation`, `event_context`, `preference`, `metric`.
//! Scopes: `workspace` (visible to all), `user` (visible to creator only).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::enums::{LearningScope, LearningType};

/// An agent learning entry from the `agent_learnings` table.
#[derive(Debug, Clone, sqlx::FromRow, Serialize, Deserialize)]
pub struct AgentLearning {
    /// Primary key — UUID, generated server-side via `gen_random_uuid()`.
    pub learning_id: Uuid,

    /// FK to workspaces table — workspace isolation.
    pub workspace_id: String,

    /// FK to datasource_configs table — `None` means global (applies to all datasources).
    pub datasource_config_id: Option<String>,

    /// The learning content in natural language.
    pub insight: String,

    /// Why/how this learning was captured.
    pub context: Option<String>,

    /// Canonical SQL queries as JSON array.
    pub reference_queries: Option<serde_json::Value>,

    /// Classification: `navigation`, `event_context`, `preference`, `metric`.
    pub learning_type: LearningType,

    /// 384-dimension embedding vector for semantic search.
    /// Stored as raw f32 little-endian bytes (384 × 4 = 1536 bytes).
    /// Use `embedding_compat::bytes_to_embedding()` to convert back to f32 vec.
    #[sqlx(default)]
    pub embedding: Option<Vec<u8>>,

    /// Whether this learning is active (disabled learnings are excluded from search).
    pub enabled: bool,

    /// Visibility scope: `workspace` (all users) or `user` (creator only).
    pub scope: LearningScope,

    /// Session ID where this learning was captured.
    pub learned_from_session: Option<String>,

    /// User ID who triggered/created this learning.
    pub learned_from_user: Option<String>,

    /// Record creation timestamp.
    pub created_at: DateTime<Utc>,

    /// Number of times this learning has been retrieved for context injection.
    pub times_used: i32,

    /// When this learning was last retrieved for context injection.
    pub last_used_at: Option<DateTime<Utc>>,

    /// ID of the learning that supersedes (replaces) this one.
    pub superseded_by: Option<Uuid>,

    /// Whether this learning has been superseded by another.
    pub is_superseded: bool,
}
