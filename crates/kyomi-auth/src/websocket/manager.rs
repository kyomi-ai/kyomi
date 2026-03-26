// SPDX-License-Identifier: AGPL-3.0-or-later

//! WebSocket connection manager with optional Redis pub/sub for multi-replica delivery.
//!
//! Each pod tracks its local WebSocket connections in a `DashMap`.
//!
//! Two operating modes:
//! - **Multi-replica** (`REDIS_URL` set): Cross-pod delivery goes through Redis
//!   PUBLISH/SUBSCRIBE on `ws:user:{user_id}`. Each user with at least one local
//!   connection has a background Redis subscriber. Subscriber delivers incoming
//!   messages to all local WS connections for that user. When the last connection
//!   for a user closes, the subscriber is cancelled.
//! - **Single-instance** (`REDIS_URL` absent): Messages are delivered directly
//!   to local connections — no pub/sub overhead, no external dependency.

use std::collections::HashSet;
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use dashmap::DashMap;
use futures_util::StreamExt;
use kyomi_core::{DbPool, RedisPool, WebSocketMessage};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

/// Maximum number of WebSocket connections allowed per user.
/// Prevents a single user from exhausting server resources.
const MAX_CONNECTIONS_PER_USER: usize = 10;

/// Capacity of the bounded mpsc channel between the WebSocket manager
/// and each connection's outbound task. If the client can't keep up and
/// the buffer fills, new messages are dropped (back-pressure).
const WS_CHANNEL_CAPACITY: usize = 256;

/// Sender half of an mpsc channel — the WS endpoint writes received
/// `axum::extract::ws::Message` items to this.
pub type WsSender = mpsc::Sender<String>;

/// Monotonically increasing ID for connection deduplication.
static NEXT_CONNECTION_ID: AtomicU64 = AtomicU64::new(1);

/// A tracked WebSocket connection (sender + unique ID).
#[derive(Debug, Clone)]
pub struct TrackedConnection {
    pub id: u64,
    pub sender: WsSender,
}

impl PartialEq for TrackedConnection {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl Eq for TrackedConnection {}

impl Hash for TrackedConnection {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.id.hash(state);
    }
}

/// Redis pub/sub channel prefix for user messages.
const REDIS_CHANNEL_PREFIX: &str = "ws:user:";

/// Internal shared state behind Arc (cheap clones).
struct Inner {
    /// Local WS connections per user_id.
    connections: DashMap<String, HashSet<TrackedConnection>>,
    /// Active Redis subscriber tasks per user_id.
    subscribers: DashMap<String, JoinHandle<()>>,
    /// `Some` = multi-replica mode (Redis pub/sub enabled).
    /// `None` = single-instance mode (direct local delivery, no pub/sub).
    /// Tuple is (ConnectionManager for PUBLISH, redis_url for subscriber connections).
    redis: Option<(RedisPool, String)>,
    /// Database pool (for broadcast_to_workspace — queries workspace_users).
    db: DbPool,
}

/// Unified WebSocket manager. Cheaply cloneable (inner Arc).
#[derive(Clone)]
pub struct WebSocketManager {
    inner: Arc<Inner>,
}

impl WebSocketManager {
    /// Create a new manager.
    ///
    /// Pass `Some((pool, url))` to enable multi-replica Redis pub/sub mode.
    /// Pass `None` to run in single-instance mode with direct local delivery.
    pub fn new(redis: Option<(RedisPool, String)>, db: DbPool) -> Self {
        Self {
            inner: Arc::new(Inner {
                connections: DashMap::new(),
                subscribers: DashMap::new(),
                redis,
                db,
            }),
        }
    }

    /// Get a clone of the Redis connection pool, if Redis is configured.
    pub fn redis_pool(&self) -> Option<RedisPool> {
        self.inner.redis.as_ref().map(|(pool, _)| pool.clone())
    }

    /// Register a new WebSocket connection for a user.
    ///
    /// Returns `Ok((connection_id, Receiver))` on success, or `Err(())` if the
    /// user has reached `MAX_CONNECTIONS_PER_USER`. The caller should forward
    /// items from the receiver to the actual WebSocket sink.
    ///
    /// Starts a Redis subscriber if this is the first connection for the user.
    pub fn connect(&self, user_id: &str) -> Result<(u64, mpsc::Receiver<String>), String> {
        // Enforce per-user connection limit.
        if let Some(conns) = self.inner.connections.get(user_id)
            && conns.len() >= MAX_CONNECTIONS_PER_USER
        {
            tracing::warn!(
                user_id,
                limit = MAX_CONNECTIONS_PER_USER,
                "WebSocket connection rejected: per-user limit reached"
            );
            return Err("per-user connection limit reached".into());
        }

        let conn_id = NEXT_CONNECTION_ID.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = mpsc::channel(WS_CHANNEL_CAPACITY);

        let conn = TrackedConnection {
            id: conn_id,
            sender: tx,
        };

        // Add to local connections.
        self.inner
            .connections
            .entry(user_id.to_string())
            .or_default()
            .insert(conn);

        // Start Redis subscriber if first connection for this user (multi-replica mode only).
        if self.inner.redis.is_some() && !self.inner.subscribers.contains_key(user_id) {
            self.start_redis_subscriber(user_id);
        }

        // Send heartbeat immediately (directly, not via Redis).
        let heartbeat = WebSocketMessage::new(kyomi_core::MessageType::Heartbeat);
        if let Ok(json) = serde_json::to_string(&heartbeat) {
            self.deliver_to_local_user(user_id, &json);
        }

        Ok((conn_id, rx))
    }

    /// Unregister a WebSocket connection.
    ///
    /// If this was the last connection for the user, cancels the Redis subscriber.
    ///
    /// Uses `remove_if` to atomically check emptiness and remove, preventing a
    /// TOCTOU race where `connect()` could add a new connection between the
    /// retain and the removal.
    pub fn disconnect(&self, user_id: &str, connection_id: u64) {
        // Step 1: Remove the specific connection (holds shard write lock during get_mut).
        if let Some(mut conns) = self.inner.connections.get_mut(user_id) {
            conns.retain(|c| c.id != connection_id);
        }

        // Step 2: Atomically remove the entry only if still empty.
        // remove_if holds the shard lock while checking the predicate, so if
        // connect() inserted a new connection between step 1 and step 2, the
        // set won't be empty and this is a no-op.
        let removed_last = self
            .inner
            .connections
            .remove_if(user_id, |_, conns| conns.is_empty())
            .is_some();

        if removed_last && self.inner.redis.is_some()
            && let Some((_, handle)) = self.inner.subscribers.remove(user_id)
        {
            handle.abort();
        }
    }

    /// Send a message to a user.
    ///
    /// In multi-replica mode (Redis configured): publishes via Redis so all pods receive it.
    /// In single-instance mode (no Redis): delivers directly to local connections.
    pub async fn send_to_user(&self, user_id: &str, message: WebSocketMessage) {
        let json = match serde_json::to_string(&message) {
            Ok(j) => j,
            Err(e) => {
                tracing::error!("Failed to serialize WS message: {e}");
                return;
            }
        };

        if let Some((redis, _)) = &self.inner.redis {
            // Multi-replica: publish via Redis so all pods receive it.
            let channel = format!("{REDIS_CHANNEL_PREFIX}{user_id}");
            let mut conn = redis.clone();
            if let Err(e) = redis::cmd("PUBLISH")
                .arg(&channel)
                .arg(&json)
                .query_async::<i64>(&mut conn)
                .await
            {
                tracing::error!("Redis PUBLISH to {channel} failed: {e}");
            }
        } else {
            // Single-instance: deliver directly to local connections.
            self.deliver_to_local_user(user_id, &json);
        }
    }

    /// Broadcast a message to all members of a workspace (via Redis PUBLISH for each).
    ///
    /// Optionally excludes one user (typically the sender).
    pub async fn broadcast_to_workspace(
        &self,
        workspace_id: &str,
        message: WebSocketMessage,
        exclude_user_id: Option<&str>,
    ) {
        // Query workspace members from DB.
        let members: Vec<(String,)> = match kyomi_core::db_fetch_all!(
            &self.inner.db,
            (String,),
            "SELECT user_id FROM workspace_users WHERE workspace_id = $1",
            workspace_id
        ) {
            Ok(rows) => rows,
            Err(e) => {
                tracing::error!("Failed to query workspace members for broadcast: {e}");
                return;
            }
        };

        for (member_user_id,) in members {
            if exclude_user_id == Some(member_user_id.as_str()) {
                continue;
            }
            self.send_to_user(&member_user_id, message.clone()).await;
        }
    }

    /// Get the count of local connections across all users on this pod.
    pub fn local_connection_count(&self) -> usize {
        self.inner
            .connections
            .iter()
            .map(|entry| entry.value().len())
            .sum()
    }

    /// Start a Redis subscriber for a user. Runs in a background tokio task.
    ///
    /// IMPORTANT: `ConnectionManager` (RedisPool) does NOT support SUBSCRIBE.
    /// We must create a fresh `redis::Client` for the subscriber connection.
    ///
    /// Only called when `self.inner.redis.is_some()`.
    fn start_redis_subscriber(&self, user_id: &str) {
        let (_, redis_url) = match &self.inner.redis {
            Some(r) => r,
            None => unreachable!("start_redis_subscriber called without Redis configured"),
        };
        let user_id_owned = user_id.to_string();
        let channel = format!("{REDIS_CHANNEL_PREFIX}{user_id_owned}");
        let redis_url = redis_url.clone();
        let manager = self.clone();

        let handle = tokio::spawn(async move {
            let user_id = user_id_owned;
            // Create a dedicated Redis connection for SUBSCRIBE.
            let client = match redis::Client::open(redis_url.as_str()) {
                Ok(c) => c,
                Err(e) => {
                    tracing::error!("Redis subscriber client creation failed for {user_id}: {e}");
                    return;
                }
            };

            let mut pubsub = match client.get_async_pubsub().await {
                Ok(ps) => ps,
                Err(e) => {
                    tracing::error!("Redis SUBSCRIBE connection failed for {user_id}: {e}");
                    return;
                }
            };

            if let Err(e) = pubsub.subscribe(&channel).await {
                tracing::error!("Redis SUBSCRIBE to {channel} failed: {e}");
                return;
            }

            tracing::debug!("Redis subscriber started for {user_id} on {channel}");

            let mut stream = pubsub.on_message();

            while let Some(msg) = stream.next().await {
                let payload: String = match msg.get_payload() {
                    Ok(p) => p,
                    Err(e) => {
                        tracing::warn!("Bad Redis pub/sub payload for {user_id}: {e}");
                        continue;
                    }
                };

                manager.deliver_to_local_user(&user_id, &payload);
            }

            tracing::debug!("Redis subscriber ended for {user_id}");
        });

        self.inner.subscribers.insert(user_id.to_string(), handle);
    }

    /// Deliver a JSON-encoded message to all local WebSocket connections for a user.
    ///
    /// Uses `try_send` — if a connection's bounded channel is full (client can't
    /// keep up), the message is dropped for that connection rather than blocking.
    /// Cleans up stale (closed) connections.
    fn deliver_to_local_user(&self, user_id: &str, json: &str) {
        let mut stale_ids: Vec<u64> = Vec::new();

        if let Some(conns) = self.inner.connections.get(user_id) {
            for conn in conns.value().iter() {
                match conn.sender.try_send(json.to_string()) {
                    Ok(()) => {}
                    Err(mpsc::error::TrySendError::Closed(_)) => {
                        stale_ids.push(conn.id);
                    }
                    Err(mpsc::error::TrySendError::Full(_)) => {
                        tracing::warn!(
                            user_id,
                            connection_id = conn.id,
                            "WebSocket send buffer full, dropping message"
                        );
                    }
                }
            }
        }

        // Clean up stale connections.
        if !stale_ids.is_empty()
            && let Some(mut conns) = self.inner.connections.get_mut(user_id)
        {
            conns.retain(|c| !stale_ids.contains(&c.id));
        }
    }
}

impl std::fmt::Debug for WebSocketManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WebSocketManager")
            .field("local_users", &self.inner.connections.len())
            .field("local_connections", &self.local_connection_count())
            .finish()
    }
}
