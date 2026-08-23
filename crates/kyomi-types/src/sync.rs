// SPDX-License-Identifier: AGPL-3.0-or-later

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncAction {
    pub sync_id: i64,
    pub entity_type: String,
    pub entity_id: String,
    pub workspace_id: String,
    pub action: SyncActionType,
    pub data: Option<serde_json::Value>,
    pub timestamp: String, // RFC 3339
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncActionType {
    Insert,
    Update,
    Delete,
}

/// Client→server sync request
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SyncRequest {
    SyncBootstrap,
    SyncDelta { last_sync_id: i64 },
}

/// Server→client sync response
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SyncResponse {
    SyncAction(SyncAction),
    SyncComplete {
        last_sync_id: i64,
        /// Per-entity-type row counts for the caller, keyed by the
        /// `entity_types` constants below (KYO-480). Lets the client detect
        /// a diverged local cache — entities it's missing, or stale extras
        /// it should no longer have — without a new round trip or protocol
        /// message; reconciliation rides the sync_complete the protocol
        /// already sends on every bootstrap and delta.
        ///
        /// Only entity types in `entity_types::RECONCILED` are ever present.
        /// A type can also be *absent* even though it's reconciled, when its
        /// server-side count query failed this cycle — see
        /// `compute_sync_counts` in `apps/server/src/routes/websocket.rs`.
        /// `#[serde(default)]` so an old client mid-rollout that predates
        /// this field still deserializes a `sync_complete` cleanly (reads an
        /// empty map, which correctly means "nothing to reconcile this
        /// cycle" rather than "you now have zero of everything").
        #[serde(default)]
        counts: std::collections::HashMap<String, i64>,
    },
    SyncReset,
}

/// Entity type constants
pub mod entity_types {
    pub const DASHBOARD: &str = "dashboard";
    pub const KNOWLEDGE: &str = "knowledge";
    pub const CHAT_SESSION: &str = "chat_session";
    pub const WATCH: &str = "watch";
    pub const WORKSPACE_SETTINGS: &str = "workspace_settings";

    // Tier 2 — on-demand detail caches (KYO-215).
    // These are written when a detail page resolves a server response and
    // read back on the next visit to skip the loading skeleton.
    // They are invalidated by the sync engine whenever the corresponding
    // Tier 1 list entry is touched (insert/update/delete).

    /// Full dashboard/knowledge-document content payload.
    pub const DASHBOARD_DETAIL: &str = "dashboard_detail";
    /// Ordered messages for a single chat session.
    pub const CHAT_MESSAGES: &str = "chat_messages";

    /// Entity types covered by count-based sync reconciliation (KYO-480).
    /// Shared by the server (`compute_sync_counts`, which computes one
    /// count per entry) and the client (`cache::reconcile::diverged_types`,
    /// which compares against it) so the two sides can't drift apart on
    /// which types participate.
    ///
    /// `workspace_settings` is a workspace-wide singleton fetched by primary
    /// key, not a set — there's no "count" for it to diverge from, so it's
    /// deliberately excluded. Tier 2 detail caches (`dashboard_detail`,
    /// `chat_messages`) are on-demand and invalidated by their Tier 1 parent,
    /// not part of the bootstrap/delta count surface at all.
    pub const RECONCILED: &[&str] = &[DASHBOARD, KNOWLEDGE, CHAT_SESSION, WATCH];
}
