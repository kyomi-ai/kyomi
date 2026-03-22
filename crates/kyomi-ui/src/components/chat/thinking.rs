// SPDX-License-Identifier: AGPL-3.0-or-later

//! Agent Thinking Event Processing
//!
//! Manages agent thinking events state — deduplication, sorting, and
//! per-message tracking. Ported from `apps/frontend/src/hooks/useAgentThinking.js`.
//!
//! Features:
//! - Deduplicates events by `event_id`
//! - Handles in-place updates when an event with the same ID arrives
//! - Maintains chronological sort order using lexicographic `event_id` comparison
//!   (event_id format: `{unix_millis}-{counter}` ensures sort order)
//! - Manages token usage tracking per message
//! - Tracks active/cancelled state per message
//!
//! Note: WebSocket subscription is NOT handled here — that belongs in the
//! component/hook that wires thinking events to the WebSocket. This module
//! provides the pure state management and event processing logic.

use leptos::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// A single thinking event from the agent.
///
/// Matches the event structure sent by the backend via WebSocket
/// inside `agent_thinking` messages.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ThinkingEvent {
    /// Unique event identifier (format: `{unix_millis}-{counter}`).
    pub event_id: String,
    /// Event type: `agent_start`, `agent_thought`, `tool_execution_start`, etc.
    pub event_type: String,
    /// ISO 8601 timestamp of when the event occurred.
    pub timestamp: String,
    /// Human-readable title for the event.
    pub title: String,
    /// Optional description with more detail.
    #[serde(default)]
    pub description: Option<String>,
    /// Optional structured data payload.
    #[serde(default)]
    pub data: Option<serde_json::Value>,
    /// Optional duration in milliseconds (for completed events).
    #[serde(default)]
    pub duration_ms: Option<u64>,
}

/// Token usage information from the agent.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TokenUsage {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
}

/// Thinking state for a single message.
///
/// Matches React's per-message state shape:
/// `{ events: [], isActive: false, cancelled: false, tokenUsage: null }`
#[derive(Clone, Debug, Default)]
pub struct ThinkingState {
    /// Ordered list of thinking events for this message.
    pub events: Vec<ThinkingEvent>,
    /// Whether the agent is currently thinking for this message.
    pub is_active: bool,
    /// Whether thinking was cancelled for this message.
    pub cancelled: bool,
    /// Latest token usage snapshot for this message.
    pub token_usage: Option<TokenUsage>,
}

/// Process a thinking event into an existing events list.
///
/// Deduplicates by `event_id` (all events have unique IDs).
/// Maintains chronological sort order using lexicographic comparison
/// (event_id format: `{unix_millis}-{counter}` ensures sort order).
///
/// Matches React's `processThinkingEvent()` exactly:
/// - If event_id exists: update in place
/// - If new: append and re-sort by event_id
pub fn process_thinking_event(
    existing: &[ThinkingEvent],
    new_event: ThinkingEvent,
) -> Vec<ThinkingEvent> {
    // Check if this event_id already exists
    if let Some(idx) = existing.iter().position(|e| e.event_id == new_event.event_id) {
        // Event exists — update it in place
        let mut updated = existing.to_vec();
        updated[idx] = new_event;
        updated
    } else {
        // New event — add and maintain sort order
        let mut updated = existing.to_vec();
        updated.push(new_event);
        updated.sort_by(|a, b| a.event_id.cmp(&b.event_id));
        updated
    }
}

/// Reactive thinking state manager — Leptos equivalent of `useAgentThinking()`.
///
/// Stores per-message thinking state in a `HashMap<String, ThinkingState>`
/// keyed by message_id. Provides methods matching the React hook's API.
#[derive(Clone)]
pub struct ThinkingManager {
    /// Per-message thinking state.
    state: RwSignal<HashMap<String, ThinkingState>>,
}

impl ThinkingManager {
    /// Create a new thinking manager with empty state.
    pub fn new() -> Self {
        Self {
            state: RwSignal::new(HashMap::new()),
        }
    }

    /// Handle an incoming thinking event for a message.
    ///
    /// Matches React's `handleThinkingEvent` logic (after filtering):
    /// - Skips processing if the message was cancelled
    /// - Creates new state entry if this is the first event for the message
    /// - Deduplicates and sorts events via `process_thinking_event`
    /// - Updates token usage if provided
    /// - Sets `is_active = true`
    pub fn handle_thinking_event(
        &self,
        message_id: &str,
        event: ThinkingEvent,
        token_usage: Option<TokenUsage>,
    ) {
        let message_id = message_id.to_string();
        self.state.update(|map| {
            let current = map
                .get(&message_id)
                .cloned()
                .unwrap_or_default();

            // Don't process if message was cancelled — matches React.
            if current.cancelled {
                return;
            }

            let updated_events = process_thinking_event(&current.events, event);

            map.insert(
                message_id,
                ThinkingState {
                    events: updated_events,
                    is_active: true,
                    cancelled: false,
                    token_usage: token_usage.or(current.token_usage),
                },
            );
        });
    }

    /// Mark thinking as complete for a message. Sets `is_active = false`.
    ///
    /// Matches React's `completeThinking(messageId)`.
    pub fn complete_thinking(&self, message_id: &str) {
        let message_id = message_id.to_string();
        self.state.update(|map| {
            if let Some(entry) = map.get_mut(&message_id) {
                entry.is_active = false;
            }
        });
    }

    /// Mark thinking as cancelled for a message.
    /// Sets `is_active = false` and `cancelled = true`.
    ///
    /// Matches React's `cancelThinking(messageId)`.
    pub fn cancel_thinking(&self, message_id: &str) {
        let message_id = message_id.to_string();
        self.state.update(|map| {
            if let Some(entry) = map.get_mut(&message_id) {
                entry.is_active = false;
                entry.cancelled = true;
            }
        });
    }

    /// Clear all thinking state (e.g., when switching sessions).
    ///
    /// Matches React's `clearThinking()`. Note: React also resets
    /// `seenFirstEventRef` here — if a `first_event_seen` tracker is added
    /// to `ThinkingManager` in Phase 8 (component integration), this method
    /// must be updated to reset it too.
    pub fn clear_all(&self) {
        self.state.set(HashMap::new());
    }

    /// Get the thinking state for a specific message.
    ///
    /// Returns a default (empty, inactive) state if no events exist for this message.
    /// Matches React's `getThinkingForMessage(messageId)`.
    pub fn get_for_message(&self, message_id: &str) -> ThinkingState {
        self.state
            .get_untracked()
            .get(message_id)
            .cloned()
            .unwrap_or_default()
    }

    /// Read signal for the full thinking state map.
    ///
    /// Useful for reactive access to the entire map (e.g., in derived signals).
    pub fn state(&self) -> ReadSignal<HashMap<String, ThinkingState>> {
        self.state.read_only()
    }
}
