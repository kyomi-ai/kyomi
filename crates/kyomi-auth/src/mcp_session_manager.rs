// SPDX-License-Identifier: AGPL-3.0-or-later

//! MCP Streamable HTTP Session Manager (KVStore-backed).
//!
//! KVStore-backed session store for MCP Streamable HTTP (spec 2025-03-26).
//! Sessions are shared across all replicas via the KV store, ensuring clients can
//! talk to any pod after `initialize`.
//!
//! Two complementary mechanisms for tool list freshness:
//!
//! 1. **Server restart**: sessions persist in the KV store (with TTL), so clients
//!    continue seamlessly. Sessions expire after 24 hours of inactivity.
//!
//! 2. **Runtime changes** (billing tier): pushes `notifications/tools/list_changed`
//!    via SSE to connected clients on this pod, then invalidates sessions in the KV
//!    store for all pods.

use std::sync::Arc;

use dashmap::DashMap;
use tokio::sync::mpsc;
use uuid::Uuid;

/// Session TTL in the KV store (24 hours).
const SESSION_TTL_SECS: u64 = 86400;

/// KV key prefix for individual session metadata.
/// Value: JSON `{ "workspace_id": "...", "supports_mcp_apps": false }`
fn session_key(session_id: &str) -> String {
    format!("mcp:session:{session_id}")
}

/// KV key for the JSON array of session IDs belonging to a workspace.
/// Used by `invalidate_workspace_sessions` to find all sessions to delete.
fn workspace_sessions_key(workspace_id: &str) -> String {
    format!("mcp:ws_sessions:{workspace_id}")
}

/// Per-session metadata stored in the KV store.
#[derive(serde::Serialize, serde::Deserialize)]
struct SessionData {
    workspace_id: String,
    supports_mcp_apps: bool,
}

/// Manages MCP sessions for Streamable HTTP transport.
///
/// Session metadata is stored in the KV store (shared across replicas).
/// SSE sender channels are stored in a local DashMap (process-local).
///
/// Thread-safe and cheaply cloneable.
#[derive(Clone)]
pub struct MCPSessionManager {
    kv: kyomi_core::KVPool,
    /// Local SSE sender channels — only for clients connected to THIS pod.
    /// session_id → mpsc::Sender<String>
    local_sse_senders: Arc<DashMap<String, mpsc::Sender<String>>>,
}

impl MCPSessionManager {
    /// Create a new session manager backed by a KV store.
    pub fn new(kv: kyomi_core::KVPool) -> Self {
        Self {
            kv,
            local_sse_senders: Arc::new(DashMap::new()),
        }
    }

    /// Create a new session for a workspace. Returns the session ID (UUID v4).
    pub async fn create_session(&self, workspace_id: &str) -> String {
        let session_id = Uuid::new_v4().to_string();
        let data = SessionData {
            workspace_id: workspace_id.to_string(),
            supports_mcp_apps: false,
        };

        let json = serde_json::to_string(&data).expect("SessionData is always serialisable");

        // Store session metadata with TTL
        if let Err(e) = self
            .kv
            .set(&session_key(&session_id), &json, Some(SESSION_TTL_SECS))
            .await
        {
            tracing::error!(session_id = %session_id, error = %e, "Failed to store MCP session in KV store");
            // Return the session_id anyway — worst case it won't validate on another pod
        }

        // Add session ID to workspace session list (for bulk invalidation).
        // sadd is atomic — no read-modify-write race.
        let ws_key = workspace_sessions_key(workspace_id);
        if let Err(e) = self.kv.sadd(&ws_key, &session_id).await {
            tracing::error!(workspace_id, error = %e, "Failed to add session to workspace set");
        }
        // Best-effort TTL on the workspace set: 2× session TTL so expired
        // session IDs don't accumulate indefinitely.
        if let Err(e) = self.kv.expire(&ws_key, SESSION_TTL_SECS * 2).await {
            tracing::debug!(workspace_id, error = %e, "Failed to set TTL on workspace session set");
        }

        tracing::info!(
            workspace_id,
            session_id = %session_id,
            "MCP session created"
        );

        session_id
    }

    /// Validate a session ID. Returns the workspace_id if the session exists
    /// and its stored data parses cleanly. Returns `None` if the session is
    /// unknown or expired — but also, indistinguishably to the caller, if the
    /// KV store read failed or the stored payload was malformed. Both of
    /// those failure cases are logged at `error!` (never the session
    /// payload — it is auth material) so an outage or a bad deploy is
    /// distinguishable from a genuine expiry in the logs, even though the
    /// return value alone can't tell them apart.
    pub async fn validate_session(&self, session_id: &str) -> Option<String> {
        let value: Option<String> = self
            .kv
            .get(&session_key(session_id))
            .await
            .map_err(|e| {
                tracing::error!(session_id, error = %e, "Failed to read MCP session from KV store");
            })
            .ok()?;

        value.and_then(|json| {
            serde_json::from_str::<SessionData>(&json)
                .map_err(|e| {
                    tracing::error!(
                        session_id,
                        error = %e,
                        "MCP session data is malformed (not merely absent) — treating as invalid"
                    );
                })
                .ok()
                .map(|d| d.workspace_id)
        })
    }

    /// Remove a specific session (client DELETE).
    pub async fn remove_session(&self, session_id: &str) {
        // Get workspace_id first so we can clean up the workspace sessions list
        let workspace_id: Option<String> = self
            .kv
            .get(&session_key(session_id))
            .await
            .map_err(|e| {
                tracing::warn!(
                    session_id,
                    error = %e,
                    "Failed to read MCP session from KV store during removal"
                );
            })
            .ok()
            .and_then(|json: Option<String>| {
                json.and_then(|j| {
                    serde_json::from_str::<SessionData>(&j)
                        .map_err(|e| {
                            tracing::warn!(
                                session_id,
                                error = %e,
                                "MCP session data is malformed during removal"
                            );
                        })
                        .ok()
                        .map(|d| d.workspace_id)
                })
            });

        // Delete session key
        let _ = self.kv.del(&session_key(session_id)).await;

        // Remove from workspace sessions set (atomic).
        if let Some(ws_id) = &workspace_id {
            let ws_key = workspace_sessions_key(ws_id);
            let _ = self.kv.srem(&ws_key, session_id).await;
        }

        // Remove local SSE sender
        self.local_sse_senders.remove(session_id);

        tracing::info!(
            session_id,
            workspace_id = workspace_id.as_deref().unwrap_or("unknown"),
            "MCP session removed by client"
        );
    }

    /// Set the SSE sender channel for a session (called when client opens GET SSE stream).
    ///
    /// SSE channels are inherently process-local, so this is stored in-memory.
    pub fn set_sse_sender(&self, session_id: &str, sender: mpsc::Sender<String>) {
        self.local_sse_senders
            .insert(session_id.to_string(), sender);
        tracing::info!(session_id, "SSE sender set for MCP session");
    }

    /// Mark a session as supporting MCP Apps (interactive UI via iframes).
    pub async fn set_supports_mcp_apps(&self, session_id: &str, supports: bool) {
        let key = session_key(session_id);

        // NOTE: This is a read-modify-write (GET then SET_EX) which has a small race window.
        // This is pre-existing behaviour from the Redis implementation. The flag is typically
        // set once per session, making concurrent calls extremely rare in practice.
        let value: Option<String> = match self.kv.get(&key).await {
            Ok(v) => v,
            Err(_) => return,
        };

        if let Some(json) = value
            && let Ok(mut data) = serde_json::from_str::<SessionData>(&json) {
                data.supports_mcp_apps = supports;
                if let Ok(updated) = serde_json::to_string(&data) {
                    let _ = self.kv.set(&key, &updated, Some(SESSION_TTL_SECS)).await;
                }
            }
    }

    /// Check if a session's client supports MCP Apps.
    pub async fn supports_mcp_apps(&self, session_id: &str) -> bool {
        let value: Option<String> = self
            .kv
            .get(&session_key(session_id))
            .await
            .unwrap_or(None);

        value
            .and_then(|json| serde_json::from_str::<SessionData>(&json).ok())
            .map(|d| d.supports_mcp_apps)
            .unwrap_or(false)
    }

    /// Push `notifications/tools/list_changed` to SSE clients on THIS pod for a workspace.
    ///
    /// Dead connections (closed senders) are automatically cleaned up.
    /// This only reaches clients connected to this specific replica. Call
    /// `invalidate_workspace_sessions` separately to force clients on other
    /// pods to re-initialize on their next request.
    pub async fn notify_tools_changed(&self, workspace_id: &str) {
        // Find local SSE senders that belong to sessions in this workspace
        let mut senders: Vec<(String, mpsc::Sender<String>)> = Vec::new();

        for entry in self.local_sse_senders.iter() {
            let session_id = entry.key().clone();
            // Check if this session belongs to the workspace
            if let Some(ws_id) = self.validate_session(&session_id).await
                && ws_id == workspace_id {
                    senders.push((session_id, entry.value().clone()));
                }
        }

        if senders.is_empty() {
            tracing::debug!(
                workspace_id,
                "No local SSE connections for workspace, skipping notification"
            );
            return;
        }

        let notification =
            r#"{"jsonrpc":"2.0","method":"notifications/tools/list_changed"}"#.to_string();

        let mut dead_sessions = Vec::new();
        for (session_id, sender) in &senders {
            if sender.send(notification.clone()).await.is_err() {
                dead_sessions.push(session_id.clone());
            } else {
                tracing::info!(
                    workspace_id,
                    session_id = %session_id,
                    "Sent tools/list_changed notification via SSE"
                );
            }
        }

        // Clear dead SSE senders
        for session_id in &dead_sessions {
            self.local_sse_senders.remove(session_id);
        }
    }

    /// Invalidate all sessions for a workspace (billing tier changes, etc.).
    ///
    /// Removes sessions from the KV store so that ANY pod will return 404 for these
    /// session IDs, forcing clients to re-initialize.
    pub async fn invalidate_workspace_sessions(&self, workspace_id: &str) {
        let ws_key = workspace_sessions_key(workspace_id);

        // Get all session IDs for this workspace (atomic read from set).
        let session_ids = match self.kv.smembers(&ws_key).await {
            Ok(ids) if ids.is_empty() => return,
            Ok(ids) => ids,
            Err(e) => {
                tracing::error!(workspace_id, error = %e, "Failed to read workspace session set");
                return;
            }
        };

        // Delete each session key
        for session_id in &session_ids {
            let _ = self.kv.del(&session_key(session_id)).await;
        }

        // Delete the workspace sessions set itself.
        let _ = self.kv.sdel(&ws_key).await;

        // Clean up local SSE senders for these sessions
        for session_id in &session_ids {
            self.local_sse_senders.remove(session_id);
        }

        tracing::info!(
            workspace_id,
            removed = session_ids.len(),
            "MCP sessions invalidated for workspace"
        );
    }

}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use kyomi_core::kv_store::KVStore;
    use tracing::Level;

    use kyomi_test_tracing::capture_tracing;

    use super::*;

    /// Minimal `KVStore` whose `get` always fails, to exercise the KV-outage
    /// path in `validate_session`. No other method is reachable through that
    /// path, so they panic if called — a test that somehow hits them should
    /// fail loudly rather than silently succeed against unintended behaviour.
    struct FailingKVStore;

    #[async_trait::async_trait]
    impl KVStore for FailingKVStore {
        async fn set(&self, _key: &str, _value: &str, _ttl_secs: Option<u64>) -> kyomi_core::Result<()> {
            unimplemented!("not exercised by the KV-outage test")
        }

        async fn get(&self, _key: &str) -> kyomi_core::Result<Option<String>> {
            Err(kyomi_core::Error::ServiceUnavailable(
                "simulated KV outage".to_string(),
            ))
        }

        async fn del(&self, _key: &str) -> kyomi_core::Result<()> {
            unimplemented!("not exercised by the KV-outage test")
        }

        async fn getdel(&self, _key: &str) -> kyomi_core::Result<Option<String>> {
            unimplemented!("not exercised by the KV-outage test")
        }

        async fn incr(&self, _key: &str) -> kyomi_core::Result<i64> {
            unimplemented!("not exercised by the KV-outage test")
        }

        async fn expire(&self, _key: &str, _ttl_secs: u64) -> kyomi_core::Result<()> {
            unimplemented!("not exercised by the KV-outage test")
        }

        async fn sadd(&self, _key: &str, _member: &str) -> kyomi_core::Result<()> {
            unimplemented!("not exercised by the KV-outage test")
        }

        async fn srem(&self, _key: &str, _member: &str) -> kyomi_core::Result<()> {
            unimplemented!("not exercised by the KV-outage test")
        }

        async fn smembers(&self, _key: &str) -> kyomi_core::Result<Vec<String>> {
            unimplemented!("not exercised by the KV-outage test")
        }

        async fn sdel(&self, _key: &str) -> kyomi_core::Result<()> {
            unimplemented!("not exercised by the KV-outage test")
        }
    }

    async fn test_manager() -> MCPSessionManager {
        let kv = kyomi_core::kv_store::create_kv_store(None)
            .await
            .expect("in-memory KV store should initialize");
        MCPSessionManager::new(kv)
    }

    /// Clean up test keys to avoid cross-test contamination.
    async fn cleanup(mgr: &MCPSessionManager, session_ids: &[&str], workspace_ids: &[&str]) {
        for sid in session_ids {
            let _ = mgr.kv.del(&session_key(sid)).await;
        }
        for wid in workspace_ids {
            // Workspace session lists are now stored as sets.
            let _ = mgr.kv.sdel(&workspace_sessions_key(wid)).await;
        }
    }

    #[tokio::test]
    async fn new_manager_has_no_sessions() {
        let mgr = test_manager().await;
        assert!(mgr.validate_session("nonexistent").await.is_none());
    }

    #[tokio::test]
    async fn create_and_validate_session() {
        let mgr = test_manager().await;
        let session_id = mgr.create_session("ws-test-1").await;

        let ws = mgr.validate_session(&session_id).await;
        assert_eq!(ws, Some("ws-test-1".to_string()));

        cleanup(&mgr, &[&session_id], &["ws-test-1"]).await;
    }

    /// The normal "no such session" path must stay silent. This is the load-
    /// bearing counterpart to the two tests below: without it, logging
    /// unconditionally on every `None` would pass those tests while making
    /// the ordinary "session expired" case as noisy as a real outage.
    #[tokio::test(flavor = "current_thread")]
    async fn validate_session_absent_does_not_log_error() {
        let mgr = test_manager().await;

        let logs = capture_tracing();

        let result = mgr.validate_session("nonexistent-session").await;

        assert!(result.is_none(), "an absent session should validate to None");

        let error_events = logs.events_at(Level::ERROR);
        assert!(
            error_events.is_empty(),
            "a merely-absent session must not log an error; captured: {:?}",
            logs.events()
        );
    }

    /// A malformed (but present) session payload must return `None` exactly
    /// like an absent session, but unlike the absent case it must log an
    /// `error!` that identifies the data as malformed — and must never leak
    /// the stored payload itself, since `SessionData` is auth material.
    #[tokio::test(flavor = "current_thread")]
    async fn validate_session_malformed_payload_logs_error() {
        let mgr = test_manager().await;
        let session_id = "malformed-session";
        let bogus_payload = r#"{"totally":"not-session-data","secret":"sh-should-not-leak"}"#;

        mgr.kv
            .set(&session_key(session_id), bogus_payload, Some(60))
            .await
            .expect("in-memory KV set should succeed");

        let logs = capture_tracing();

        let result = mgr.validate_session(session_id).await;

        assert!(
            result.is_none(),
            "malformed session data must be treated as invalid, not surfaced to the caller"
        );

        let error_events = logs.events_at(Level::ERROR);
        assert!(
            !error_events.is_empty(),
            "malformed session data must emit an error!-level log; captured: {:?}",
            logs.events()
        );
        assert!(
            error_events.iter().any(|(_, message)| message.contains("malformed")),
            "expected an error log identifying the data as malformed (not merely absent); \
             captured: {:?}",
            logs.events()
        );
        for (_, message) in &error_events {
            assert!(
                !message.contains(bogus_payload) && !message.contains("sh-should-not-leak"),
                "session-data error logs must never contain the stored payload \
                 (SessionData is auth material); got: {message}"
            );
        }

        cleanup(&mgr, &[session_id], &[]).await;
    }

    /// A KV store outage must return `None` — the safe, fail-closed
    /// direction — but must be logged at `error!` so it is distinguishable
    /// from a genuine session expiry. Without this test, a Redis blip would
    /// look identical to every client simply having an expired session, and
    /// nothing would tell an operator the store was unreachable.
    #[tokio::test(flavor = "current_thread")]
    async fn validate_session_kv_error_logs_error() {
        let mgr = MCPSessionManager::new(Arc::new(FailingKVStore));

        let logs = capture_tracing();

        let result = mgr.validate_session("any-session").await;

        assert!(
            result.is_none(),
            "a KV store error must deny access (fail closed), not surface to the caller"
        );

        let error_events = logs.events_at(Level::ERROR);
        assert!(
            !error_events.is_empty(),
            "a KV store error must emit an error!-level log; captured: {:?}",
            logs.events()
        );
    }

    #[tokio::test]
    async fn remove_session_invalidates_it() {
        let mgr = test_manager().await;
        let session_id = mgr.create_session("ws-test-2").await;

        mgr.remove_session(&session_id).await;
        assert!(mgr.validate_session(&session_id).await.is_none());

        cleanup(&mgr, &[&session_id], &["ws-test-2"]).await;
    }

    #[tokio::test]
    async fn remove_unknown_session_is_noop() {
        let mgr = test_manager().await;
        mgr.remove_session("nonexistent").await; // should not panic
    }

    #[tokio::test]
    async fn invalidate_workspace_clears_all_workspace_sessions() {
        let mgr = test_manager().await;
        let s1 = mgr.create_session("ws-test-3").await;
        let s2 = mgr.create_session("ws-test-3").await;
        let s3 = mgr.create_session("ws-test-4").await;

        mgr.invalidate_workspace_sessions("ws-test-3").await;

        assert!(mgr.validate_session(&s1).await.is_none());
        assert!(mgr.validate_session(&s2).await.is_none());
        // Different workspace should be unaffected
        assert_eq!(mgr.validate_session(&s3).await, Some("ws-test-4".to_string()));

        cleanup(&mgr, &[&s1, &s2, &s3], &["ws-test-3", "ws-test-4"]).await;
    }

    #[tokio::test]
    async fn invalidate_empty_workspace_is_noop() {
        let mgr = test_manager().await;
        mgr.invalidate_workspace_sessions("ws-nonexistent").await; // should not panic
    }

    #[tokio::test]
    async fn clone_shares_state() {
        let mgr1 = test_manager().await;
        let mgr2 = mgr1.clone();

        let session_id = mgr1.create_session("ws-test-5").await;
        assert_eq!(
            mgr2.validate_session(&session_id).await,
            Some("ws-test-5".to_string())
        );

        cleanup(&mgr1, &[&session_id], &["ws-test-5"]).await;
    }

    #[tokio::test]
    async fn session_ids_are_unique() {
        let mgr = test_manager().await;
        let s1 = mgr.create_session("ws-test-6").await;
        let s2 = mgr.create_session("ws-test-6").await;
        assert_ne!(s1, s2);

        cleanup(&mgr, &[&s1, &s2], &["ws-test-6"]).await;
    }

    #[tokio::test]
    async fn mcp_apps_support_default_false() {
        let mgr = test_manager().await;
        let session_id = mgr.create_session("ws-test-7").await;
        assert!(!mgr.supports_mcp_apps(&session_id).await);

        cleanup(&mgr, &[&session_id], &["ws-test-7"]).await;
    }

    #[tokio::test]
    async fn set_and_check_mcp_apps_support() {
        let mgr = test_manager().await;
        let session_id = mgr.create_session("ws-test-8").await;

        mgr.set_supports_mcp_apps(&session_id, true).await;
        assert!(mgr.supports_mcp_apps(&session_id).await);

        cleanup(&mgr, &[&session_id], &["ws-test-8"]).await;
    }

    #[tokio::test]
    async fn mcp_apps_support_unknown_session_returns_false() {
        let mgr = test_manager().await;
        assert!(!mgr.supports_mcp_apps("nonexistent").await);
    }

    #[tokio::test]
    async fn set_mcp_apps_support_unknown_session_is_noop() {
        let mgr = test_manager().await;
        mgr.set_supports_mcp_apps("nonexistent", true).await; // should not panic
    }

    #[tokio::test]
    async fn set_sse_sender_on_valid_session() {
        let mgr = test_manager().await;
        let session_id = mgr.create_session("ws-test-9").await;

        let (tx, _rx) = mpsc::channel::<String>(8);
        mgr.set_sse_sender(&session_id, tx);

        // Session should still be valid
        assert_eq!(
            mgr.validate_session(&session_id).await,
            Some("ws-test-9".to_string())
        );

        cleanup(&mgr, &[&session_id], &["ws-test-9"]).await;
    }

    #[tokio::test]
    async fn set_sse_sender_on_unknown_session_is_noop() {
        let mgr = test_manager().await;
        let (tx, _rx) = mpsc::channel::<String>(8);
        mgr.set_sse_sender("nonexistent", tx); // should not panic
    }

    #[tokio::test]
    async fn notify_sends_to_sse_connections() {
        let mgr = test_manager().await;
        let s1 = mgr.create_session("ws-test-10").await;
        let s2 = mgr.create_session("ws-test-10").await;

        let (tx1, mut rx1) = mpsc::channel::<String>(8);
        let (tx2, mut rx2) = mpsc::channel::<String>(8);
        mgr.set_sse_sender(&s1, tx1);
        mgr.set_sse_sender(&s2, tx2);

        mgr.notify_tools_changed("ws-test-10").await;

        let msg1 = rx1.recv().await.expect("should receive notification");
        let msg2 = rx2.recv().await.expect("should receive notification");

        assert!(msg1.contains("notifications/tools/list_changed"));
        assert!(msg2.contains("notifications/tools/list_changed"));

        cleanup(&mgr, &[&s1, &s2], &["ws-test-10"]).await;
    }

    #[tokio::test]
    async fn notify_skips_sessions_without_sse() {
        let mgr = test_manager().await;
        let _s1 = mgr.create_session("ws-test-11").await; // No SSE sender
        let s2 = mgr.create_session("ws-test-11").await;

        let (tx2, mut rx2) = mpsc::channel::<String>(8);
        mgr.set_sse_sender(&s2, tx2);

        mgr.notify_tools_changed("ws-test-11").await;

        let msg = rx2.recv().await.expect("should receive notification");
        assert!(msg.contains("notifications/tools/list_changed"));

        cleanup(&mgr, &[&_s1, &s2], &["ws-test-11"]).await;
    }

    #[tokio::test]
    async fn notify_cleans_dead_sse_senders() {
        let mgr = test_manager().await;
        let s1 = mgr.create_session("ws-test-12").await;
        let s2 = mgr.create_session("ws-test-12").await;

        let (tx1, rx1) = mpsc::channel::<String>(8);
        let (tx2, _rx2) = mpsc::channel::<String>(8);
        mgr.set_sse_sender(&s1, tx1);
        mgr.set_sse_sender(&s2, tx2);

        // Drop rx1 — tx1 becomes a dead sender
        drop(rx1);

        mgr.notify_tools_changed("ws-test-12").await;

        // Session s1 should still exist in KV store (we only clear the local sender)
        assert!(mgr.validate_session(&s1).await.is_some());
        // Session s2 should still exist
        assert!(mgr.validate_session(&s2).await.is_some());

        cleanup(&mgr, &[&s1, &s2], &["ws-test-12"]).await;
    }

    #[tokio::test]
    async fn notify_no_connections_is_noop() {
        let mgr = test_manager().await;
        // Should not panic or error
        mgr.notify_tools_changed("ws-nonexistent").await;
    }

    #[tokio::test]
    async fn notify_valid_json_rpc() {
        let mgr = test_manager().await;
        let session_id = mgr.create_session("ws-test-13").await;

        let (tx, mut rx) = mpsc::channel::<String>(8);
        mgr.set_sse_sender(&session_id, tx);

        mgr.notify_tools_changed("ws-test-13").await;

        let msg = rx.recv().await.unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&msg).unwrap();

        assert_eq!(parsed["jsonrpc"], "2.0");
        assert_eq!(parsed["method"], "notifications/tools/list_changed");
        assert!(parsed.get("id").is_none());

        cleanup(&mgr, &[&session_id], &["ws-test-13"]).await;
    }

    #[tokio::test]
    async fn notify_does_not_affect_other_workspaces() {
        let mgr = test_manager().await;
        let s1 = mgr.create_session("ws-test-14").await;
        let s2 = mgr.create_session("ws-test-15").await;

        let (tx1, mut rx1) = mpsc::channel::<String>(8);
        let (tx2, mut rx2) = mpsc::channel::<String>(8);
        mgr.set_sse_sender(&s1, tx1);
        mgr.set_sse_sender(&s2, tx2);

        mgr.notify_tools_changed("ws-test-14").await;

        // ws-test-14 should receive notification
        let msg = rx1.recv().await.expect("ws-test-14 should receive notification");
        assert!(msg.contains("notifications/tools/list_changed"));

        // ws-test-15 should NOT receive anything
        assert!(
            rx2.try_recv().is_err(),
            "ws-test-15 should not receive notification"
        );

        cleanup(&mgr, &[&s1, &s2], &["ws-test-14", "ws-test-15"]).await;
    }
}
