// SPDX-License-Identifier: AGPL-3.0-or-later

//! Agent Thinking Event Processing
//!
//! Manages agent thinking events state — deduplication, sorting, and
//! per-message tracking. Ported from `apps/frontend/src/hooks/useAgentThinking.js`.
//!
//! Features:
//! - Deduplicates events by `event_id`
//! - Handles in-place updates when an event with the same ID arrives
//! - Orders events by occurrence time, independently of opaque event identity
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
    /// Opaque event identity; runtime hashes and legacy timestamp IDs are both valid.
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
    /// True when the full reasoning text is available on demand.
    #[serde(default)]
    pub has_full_text: bool,
}

/// Token usage information from the agent.
///
/// Field names match the WebSocket `token_usage_update` event JSON shape:
/// `{"input_tokens": N, "output_tokens": N, "total_tokens": N, "cost": X.XX,
///   "context_tokens": N, "context_window": N}`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TokenUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    #[serde(default)]
    pub total_tokens: u64,
    #[serde(default)]
    pub cost: f64,
    /// Token count from the most recent LLM request (not cumulative).
    /// Used to compute context window utilisation percentage.
    #[serde(default)]
    pub context_tokens: u64,
    /// Context window size for the model in use (0 = unknown).
    #[serde(default)]
    pub context_window: u64,
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

/// Process an event by occurrence time, independently of its opaque ID.
///
/// Equal instants have no recoverable execution order in stored records. Break
/// ties by opaque ID. Invalid legacy timestamps sort before valid ones, by
/// their raw value. IDs break ties deterministically, not by execution order.
/// Same-ID revisions retain their original occurrence time and card position.
/// Inserting a new event never reorders existing cards, including revised ones.
pub fn process_thinking_event(
    existing: &[ThinkingEvent],
    mut new_event: ThinkingEvent,
) -> Vec<ThinkingEvent> {
    let mut updated = existing.to_vec();
    if let Some(idx) = existing
        .iter()
        .position(|e| e.event_id == new_event.event_id)
    {
        if thinking_event_is_terminal(&existing[idx]) && !thinking_event_is_terminal(&new_event) {
            return updated;
        }
        new_event.timestamp.clone_from(&existing[idx].timestamp);
        updated[idx] = new_event;
    } else {
        let key = |event: &ThinkingEvent| {
            let instant = chrono::DateTime::parse_from_rfc3339(&event.timestamp).ok();
            let invalid = instant.is_none().then(|| event.timestamp.clone());
            (instant, invalid, event.event_id.clone())
        };
        let new_key = key(&new_event);
        let index = updated
            .iter()
            .position(|event| key(event) > new_key)
            .unwrap_or(updated.len());
        updated.insert(index, new_event);
    }
    updated
}

pub(crate) fn thinking_event_is_terminal(event: &ThinkingEvent) -> bool {
    event.duration_ms.is_some()
        || event
            .data
            .as_ref()
            .and_then(|data| data.get("status"))
            .and_then(|status| status.as_str())
            == Some("completed")
}

/// Reference-counted reasoning state owned by a chat run.
#[derive(Clone)]
pub(crate) struct ThinkingData {
    state: ArcRwSignal<HashMap<String, ThinkingState>>,
}

impl ThinkingData {
    pub(crate) fn new() -> Self {
        Self {
            state: ArcRwSignal::new(HashMap::new()),
        }
    }

    pub(crate) fn state(&self) -> ArcReadSignal<HashMap<String, ThinkingState>> {
        self.state.read_only()
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
            let current = map.get(&message_id).cloned().unwrap_or_default();

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

    /// Update only the token usage for a message without modifying events or active state.
    ///
    /// Used by the `token_usage_update` WS event which carries token counts
    /// independently of thinking events.
    pub fn update_token_usage(&self, message_id: &str, usage: TokenUsage) {
        let message_id = message_id.to_string();
        self.state.update(|map| {
            let current = map.get(&message_id).cloned().unwrap_or_default();
            if current.cancelled {
                return;
            }
            map.insert(
                message_id,
                ThinkingState {
                    token_usage: Some(usage),
                    ..current
                },
            );
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
}

/// Page-owned projection of the selected run's reasoning state.
#[derive(Clone)]
pub struct ThinkingManager {
    data: Signal<ThinkingData>,
    state_read: Signal<HashMap<String, ThinkingState>>,
}

impl Default for ThinkingManager {
    fn default() -> Self {
        Self::new()
    }
}

impl ThinkingManager {
    pub fn new() -> Self {
        let data = ThinkingData::new();
        Self::from_source(Signal::derive(move || data.clone()))
    }

    pub(crate) fn from_source(data: Signal<ThinkingData>) -> Self {
        let state_read = Signal::derive(move || {
            data.try_get()
                .and_then(|data| data.state().try_get())
                .unwrap_or_default()
        });
        Self { data, state_read }
    }

    pub fn state(&self) -> Signal<HashMap<String, ThinkingState>> {
        self.state_read
    }

    pub fn handle_thinking_event(
        &self,
        message_id: &str,
        event: ThinkingEvent,
        token_usage: Option<TokenUsage>,
    ) {
        self.data
            .get_untracked()
            .handle_thinking_event(message_id, event, token_usage)
    }

    pub fn complete_thinking(&self, message_id: &str) {
        self.data.get_untracked().complete_thinking(message_id)
    }

    pub fn cancel_thinking(&self, message_id: &str) {
        self.data.get_untracked().cancel_thinking(message_id)
    }

    pub fn update_token_usage(&self, message_id: &str, usage: TokenUsage) {
        self.data
            .get_untracked()
            .update_token_usage(message_id, usage)
    }

    pub fn clear_all(&self) {
        self.data.get_untracked().clear_all()
    }

    pub fn get_for_message(&self, message_id: &str) -> ThinkingState {
        self.data.get_untracked().get_for_message(message_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(id: &str, time: &str, kind: &str) -> ThinkingEvent {
        serde_json::from_value(serde_json::json!({
            "event_id": id, "timestamp": time, "event_type": kind, "title": id,
        }))
        .expect("legacy event without optional fields")
    }

    fn ids(events: &[ThinkingEvent]) -> Vec<&str> {
        events.iter().map(|event| event.event_id.as_str()).collect()
    }

    #[test]
    fn thinking_hash_chronology_survives_out_of_order_delivery() {
        // SHA-256 of compact [conversation-demo, run-demo, event, key] JSON,
        // using the runtime identity format and the five KYO-926 keys.
        let hashes = [
            "674d51b2e97cacd48d9047afecb783fdbd4270ab1de1816a7b4e90cabd41ea43",
            "94b545c768bfca608f621c1c8d8e35c1d690f8c3182a136e096d061f1bf430b0",
            "4e9f0098c7c247c57e61b7c90c99a1990f65a8aff3ef32bb17787285aa5c2753",
            "5c9c08330f4adc14e9f4417d47ec96611a21c8d5f2e4baf475f09781efccdcca",
            "4345b7e436800e36ce7f52c6c645b9dfa4fe4465a20f791effb48d6abd18d682",
        ];
        let kinds = [
            "agent_thought",
            "tool_execution_start",
            "tool_execution_end",
            "agent_thought",
            "agent_thought",
        ];
        for order in [[0, 1, 2, 3, 4], [4, 2, 3, 0, 1]] {
            let mut events = Vec::new();
            for i in order {
                events = process_thinking_event(
                    &events,
                    event(hashes[i], &format!("2026-10-16T05:00:0{i}Z"), kinds[i]),
                );
            }
            assert_eq!(ids(&events), hashes);
            assert_eq!(events.last().expect("preparing").event_id, hashes[4]);
        }
    }

    #[test]
    fn thinking_mixed_legacy_ids_and_offsets_use_occurrence_time() {
        let mut events = Vec::new();
        for (id, time, kind) in [
            ("1000-002", "2026-10-16T05:00:03Z", "agent_complete"),
            (
                "0000hash",
                "2026-10-16T16:00:02+11:00",
                "tool_execution_end",
            ),
            ("9999hash", "2026-10-16T05:00:01Z", "tool_execution_start"),
            ("1000-001", "2026-10-16T05:00:00Z", "agent_start"),
        ] {
            events = process_thinking_event(&events, event(id, time, kind));
        }
        assert_eq!(
            ids(&events),
            ["1000-001", "9999hash", "0000hash", "1000-002"]
        );
    }

    #[test]
    fn thinking_equal_instants_and_invalid_legacy_times_are_deterministic() {
        let input = [
            event("b", "2026-10-16T16:00:00+11:00", "agent_thought"),
            event("a", "2026-10-16T05:00:00Z", "agent_thought"),
            event("0", "2026-10-16T05:00:00Z", "tool_execution_end"),
            event("z", "2026-10-16T05:00:00Z", "agent_start"),
            event("old", "unknown", "agent_thought"),
        ];
        for order in [vec![0, 1, 2, 3, 4], vec![4, 3, 2, 1, 0]] {
            let events = order.into_iter().fold(Vec::new(), |events, i| {
                process_thinking_event(&events, input[i].clone())
            });
            assert_eq!(ids(&events), ["old", "0", "a", "b", "z"]);
        }
    }

    #[test]
    fn thinking_same_id_completion_keeps_card_placement_and_rejects_stale_start() {
        let start = event("tool", "2026-10-16T05:00:00Z", "tool_execution_start");
        let mut events = process_thinking_event(&[], start.clone());
        events = process_thinking_event(
            &events,
            event("z-peer", "2026-10-16T05:00:00Z", "agent_thought"),
        );
        let mut completion = start.clone();
        completion.timestamp = "2026-10-16T05:00:03Z".into();
        completion.event_type = "tool_execution_end".into();
        completion.data = Some(serde_json::json!({"status": "completed"}));
        events = process_thinking_event(&events, completion.clone());
        events = process_thinking_event(
            &events,
            event("later", "2026-10-16T05:00:01Z", "agent_thought"),
        );
        events = process_thinking_event(&events, start);
        events = process_thinking_event(&events, completion);
        assert_eq!(ids(&events), ["tool", "z-peer", "later"]);
        assert_eq!(events[0].timestamp, "2026-10-16T05:00:00Z");
        assert!(thinking_event_is_terminal(&events[0]));
    }
}
