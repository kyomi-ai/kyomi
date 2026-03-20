// SPDX-License-Identifier: AGPL-3.0-or-later

//! Global registry mapping (user_id, session_id) → CancellationToken.
//!
//! Allows WebSocket `cancel_request` messages to cancel in-flight agent tasks.
//! Each chat/copilot handler registers its token before spawning the agent task,
//! and removes it when the task completes. The WebSocket handler calls `cancel()`
//! when it receives a `cancel_request` message from the client.

use dashmap::DashMap;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

/// Registry for in-flight agent cancellation tokens.
///
/// Cheaply cloneable (inner `Arc`). Thread-safe via `DashMap`.
#[derive(Clone, Default)]
pub struct CancelRegistry {
    tokens: Arc<DashMap<(String, String), CancellationToken>>,
}

impl CancelRegistry {
    /// Register a cancellation token for an in-flight agent task.
    pub fn register(&self, user_id: &str, session_id: &str, token: CancellationToken) {
        self.tokens
            .insert((user_id.to_string(), session_id.to_string()), token);
    }

    /// Cancel an in-flight agent task. Returns `true` if a token was found and cancelled.
    pub fn cancel(&self, user_id: &str, session_id: &str) -> bool {
        let key = (user_id.to_string(), session_id.to_string());
        if let Some((_k, token)) = self.tokens.remove(&key) {
            token.cancel();
            true
        } else {
            false
        }
    }

    /// Remove a token after the agent task completes (prevents stale entries).
    pub fn remove(&self, user_id: &str, session_id: &str) {
        let key = (user_id.to_string(), session_id.to_string());
        self.tokens.remove(&key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_and_cancel() {
        let registry = CancelRegistry::default();
        let token = CancellationToken::new();
        let child = token.clone();

        registry.register("user-1", "session-1", token);
        assert!(!child.is_cancelled());

        let cancelled = registry.cancel("user-1", "session-1");
        assert!(cancelled);
        assert!(child.is_cancelled());
    }

    #[test]
    fn cancel_unknown_returns_false() {
        let registry = CancelRegistry::default();
        assert!(!registry.cancel("user-1", "session-1"));
    }

    #[test]
    fn remove_prevents_cancel() {
        let registry = CancelRegistry::default();
        let token = CancellationToken::new();
        let child = token.clone();

        registry.register("user-1", "session-1", token);
        registry.remove("user-1", "session-1");

        let cancelled = registry.cancel("user-1", "session-1");
        assert!(!cancelled);
        assert!(!child.is_cancelled());
    }
}
