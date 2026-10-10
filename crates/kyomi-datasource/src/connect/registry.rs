// SPDX-License-Identifier: AGPL-3.0-or-later

//! ConnectRegistry — manages WebSocket connections from Kyomi Connect instances.
//!
//! Maps `datasource_config_id` to the mpsc channel used to send commands to the
//! WebSocket handler task for that connection.  Redis keys provide cross-replica
//! awareness (a pod can check whether a Connect instance is online even if the
//! WebSocket lands on a different replica).
//!
//! ## Cross-replica routing
//!
//! When `send_command()` is called on a pod that does NOT hold the WebSocket
//! connection for a given datasource:
//!
//! 1. It checks the Redis presence key to confirm the connection exists somewhere.
//! 2. It publishes the `ConnectRequest` JSON to `connect:cmd:{datasource_config_id}`.
//! 3. It subscribes to `connect:res:{request_id}` for the one-shot response.
//! 4. The pod holding the WebSocket has a background subscriber on
//!    `connect:cmd:{datasource_config_id}` that forwards the command through the
//!    local mpsc channel and publishes the response back.

use std::collections::VecDeque;
use std::path::Path;
use std::sync::Arc;

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use dashmap::DashMap;
use futures_util::StreamExt;
use kyomi_core::connect_protocol::{ConnectRequest, ConnectResponse};
use kyomi_core::RedisPool;
use tokio::sync::{mpsc, oneshot, watch, Mutex};
use tokio::task::JoinHandle;

use super::owner_recovery::OwnerRecovery;

/// Channel for routing responses back to the caller.
///
/// `Once` is used for non-streaming operations (test_connection, dry_run, etc.)
/// and small queries. `Stream` is used for streaming query results where the
/// agent sends multiple response messages (Header → Chunk* → Complete).
pub enum ResponseChannel {
    /// Single response — used for non-streaming operations.
    Once(oneshot::Sender<ConnectResponse>),
    /// Multi-message streaming — used for streaming query results.
    Stream(mpsc::Sender<ConnectResponse>),
}

impl ResponseChannel {
    /// Send a single response. For `Once`, consumes the channel.
    /// For `Stream`, sends without closing (caller must drop to close).
    pub fn send(self, response: ConnectResponse) -> Result<(), ConnectResponse> {
        match self {
            ResponseChannel::Once(tx) => tx.send(response),
            ResponseChannel::Stream(tx) => tx.try_send(response).map_err(|e| e.into_inner()),
        }
    }
}

/// Payload sent through the per-connection mpsc channel.
///
/// The handler task receives `(request, response_channel)`, forwards the request
/// over the WebSocket, and sends response(s) back through the channel.
pub type CommandPayload = (ConnectRequest, ResponseChannel);

/// Sender half of the per-connection command channel.
pub type CommandSender = mpsc::Sender<CommandPayload>;

/// Redis channel prefix for command forwarding.
const CMD_CHANNEL_PREFIX: &str = "connect:cmd:";

/// Redis channel prefix for response routing.
const RES_CHANNEL_PREFIX: &str = "connect:res:";
const REVOKE_CHANNEL_PREFIX: &str = "connect:revoke:";

struct Session {
    connection_id: u64,
    owner: String,
    jti: String,
    revoked: watch::Sender<bool>,
    sender: CommandSender,
}

#[derive(Clone)]
struct ConnectionLifecycle {
    datasource_config_id: String,
    owner: String,
    jti: String,
}

/// Registry of active Kyomi Connect WebSocket connections.
///
/// Cheaply cloneable (all fields are `Arc`-wrapped or `Clone`).
#[derive(Clone)]
pub struct ConnectRegistry {
    /// Local connections on this pod, including their token generation.
    /// The connection_id prevents a stale unregister from removing a newer connection.
    connections: Arc<DashMap<String, Session>>,
    next_connection_id: Arc<AtomicU64>,
    /// Cleanup identity survives replacement of the current routing entry.
    lifecycle: Arc<DashMap<u64, ConnectionLifecycle>>,
    /// Serializes authenticated registration across its Redis await points.
    registration_lock: Arc<Mutex<()>>,
    /// Active Redis command subscriber tasks per datasource_config_id.
    /// Stores `(connection_id, handle)` so stale cleanup cannot kill a newer subscriber.
    subscribers: Arc<DashMap<String, (u64, JoinHandle<()>)>>,
    /// Redis connection for cross-replica presence keys and PUBLISH.
    /// `None` in single-instance mode — cross-replica routing is disabled, but local
    /// register/unregister/send still work for same-pod connections.
    redis: Option<RedisPool>,
    /// Redis URL for creating dedicated pub/sub connections (ConnectionManager
    /// does not support SUBSCRIBE). `None` in single-instance mode.
    redis_url: Option<String>,
    /// Verified recovery scope, with a lock retained until process exit.
    owner_recovery: Option<Arc<OwnerRecovery>>,
}

impl ConnectRegistry {
    /// Create a new registry backed by the given Redis connection.
    ///
    /// Cross-replica routing (pub/sub command forwarding) and Redis presence
    /// keys are enabled. Use this when `REDIS_URL` is configured.
    pub fn new(redis: RedisPool, redis_url: String) -> Self {
        Self {
            connections: Arc::new(DashMap::new()),
            next_connection_id: Arc::new(AtomicU64::new(1)),
            lifecycle: Arc::new(DashMap::new()),
            registration_lock: Arc::new(Mutex::new(())),
            subscribers: Arc::new(DashMap::new()),
            redis: Some(redis),
            redis_url: Some(redis_url),
            owner_recovery: None,
        }
    }

    /// Enable verified crash recovery in a persistent local filesystem scope.
    ///
    /// The directory must already exist. Keep its marker and lock files forever;
    /// every participating replica must see the same local filesystem inodes.
    /// Requires Linux and a Tokio runtime. Initialization errors fail startup.
    /// Isolated volumes and unknown scopes retain owners for operator recovery.
    pub fn with_owner_recovery(mut self, directory: &Path) -> std::io::Result<Self> {
        self.owner_recovery = Some(OwnerRecovery::for_process(directory)?);
        if self.redis.is_some() {
            let registry = self.clone();
            tokio::spawn(async move {
                registry.run_owner_recovery().await;
            });
        }
        Ok(self)
    }

    fn new_socket_owner(&self) -> String {
        self.owner_recovery.as_ref().map_or_else(
            || uuid::Uuid::new_v4().to_string(),
            |owner| owner.new_socket_owner(),
        )
    }

    /// Verify one bounded Redis scan page and remove only exact dead owners.
    /// The durable revoked marker and replacement owners are never touched.
    async fn recover_owner_page(&self, key: &str, cursor: u64) -> kyomi_core::Result<u64> {
        let (Some(redis), Some(recovery)) = (&self.redis, &self.owner_recovery) else {
            return Ok(0);
        };
        let mut conn = redis.clone();
        let (next, owners): (u64, Vec<String>) = redis::cmd("SSCAN")
            .arg(key)
            .arg(cursor)
            .arg("COUNT")
            .arg(64)
            .query_async(&mut conn)
            .await
            .map_err(|e| {
                kyomi_core::Error::ServiceUnavailable(format!("Connect owner scan failed: {e}"))
            })?;
        for owner in owners {
            let proof = match recovery.prove_dead(&owner) {
                Ok(proof) => proof,
                Err(error) => {
                    tracing::warn!(key, owner, %error, "Connect owner verification failed; owner retained");
                    continue;
                }
            };
            if let Some(_death_guard) = proof {
                redis::cmd("SREM")
                    .arg(key)
                    .arg(&owner)
                    .query_async::<i64>(&mut conn)
                    .await
                    .map_err(|e| {
                        kyomi_core::Error::ServiceUnavailable(format!(
                            "Connect dead owner cleanup failed: {e}"
                        ))
                    })?;
            }
        }
        Ok(next)
    }

    // Every configured scope reconciles independently, including when a mutation
    // is routed to another host. Resume Redis cursors one page per tick rather
    // than sweeping the entire keyspace in a request or one unbounded task turn.
    async fn run_owner_recovery(&self) {
        let Some(redis) = &self.redis else {
            return;
        };
        let mut key_cursor = 0;
        let mut keys = VecDeque::<String>::new();
        let mut member_cursor = 0;
        let mut interval = tokio::time::interval(Duration::from_secs(1));
        loop {
            interval.tick().await;
            let result = tokio::time::timeout(Duration::from_secs(5), async {
                if keys.is_empty() {
                    let mut conn = redis.clone();
                    let (next, page): (u64, Vec<String>) = redis::cmd("SCAN")
                        .arg(key_cursor)
                        .arg("MATCH")
                        .arg("connect:active:*")
                        .arg("COUNT")
                        .arg(64)
                        .query_async(&mut conn)
                        .await
                        .map_err(|e| {
                            kyomi_core::Error::ServiceUnavailable(format!(
                                "Connect recovery key scan failed: {e}"
                            ))
                        })?;
                    key_cursor = next;
                    keys.extend(page);
                }
                if let Some(key) = keys.front() {
                    member_cursor = self.recover_owner_page(key, member_cursor).await?;
                    if member_cursor == 0 {
                        keys.pop_front();
                    }
                }
                Ok::<(), kyomi_core::Error>(())
            })
            .await;
            match result {
                Ok(Ok(())) => {}
                Ok(Err(error)) => {
                    tracing::warn!(%error, "Connect owner recovery retained unverified owners");
                    keys.pop_front();
                    member_cursor = 0;
                }
                Err(error) => {
                    tracing::warn!(%error, "Connect owner recovery timed out; owners retained");
                    keys.pop_front();
                    member_cursor = 0;
                }
            }
        }
    }

    /// Create a local-only registry with no Redis backing.
    ///
    /// Register/unregister/send work for connections on this pod, but cross-replica
    /// routing (pub/sub forwarding) and Redis presence keys are disabled.
    /// Use this in single-instance mode when `REDIS_URL` is not set.
    pub fn new_local() -> Self {
        Self {
            connections: Arc::new(DashMap::new()),
            next_connection_id: Arc::new(AtomicU64::new(1)),
            lifecycle: Arc::new(DashMap::new()),
            registration_lock: Arc::new(Mutex::new(())),
            subscribers: Arc::new(DashMap::new()),
            redis: None,
            redis_url: None,
            owner_recovery: None,
        }
    }

    /// Register a Connect instance for the given datasource config.
    ///
    /// Stores the command sender in the local `DashMap` and sets a Redis key
    /// `connect:{dsid}` with a 60-second TTL for cross-replica discovery.
    ///
    /// Returns a connection ID that must be passed to `unregister()` so that a
    /// stale disconnect cannot remove a newer connection's entry.
    pub async fn register(&self, datasource_config_id: &str, sender: CommandSender) -> u64 {
        let connection_id = self.next_connection_id.fetch_add(1, Ordering::Relaxed);
        let owner = self.new_socket_owner();
        let (revoked, _) = watch::channel(false);
        self.lifecycle.insert(connection_id, ConnectionLifecycle {
            datasource_config_id: datasource_config_id.to_owned(),
            owner: owner.clone(),
            jti: String::new(),
        });
        self.connections.insert(datasource_config_id.to_string(), Session {
            connection_id,
            owner: owner.clone(),
            jti: String::new(),
            revoked,
            sender,
        });

        self.set_presence(datasource_config_id, &owner, "").await;

        tracing::info!(datasource_config_id, connection_id, "Connect instance registered");

        connection_id
    }

    /// Register an authenticated WebSocket and retain its token generation.
    pub async fn register_authenticated(
        &self,
        datasource_config_id: &str,
        jti: &str,
        sender: CommandSender,
    ) -> kyomi_core::Result<(u64, watch::Receiver<bool>)> {
        let _registration = self.registration_lock.lock().await;
        if self.is_revoked(datasource_config_id, jti).await? {
            return Err(kyomi_core::Error::ServiceUnavailable("Connect token revoked".into()));
        }
        let (revoked, receiver) = watch::channel(false);
        let connection_id = self.next_connection_id.fetch_add(1, Ordering::Relaxed);
        let owner = self.new_socket_owner();
        self.lifecycle.insert(connection_id, ConnectionLifecycle {
            datasource_config_id: datasource_config_id.to_owned(),
            owner: owner.clone(),
            jti: jti.to_owned(),
        });
        self.connections.insert(datasource_config_id.to_owned(), Session {
            connection_id,
            owner: owner.clone(),
            jti: jti.to_owned(),
            revoked,
            sender,
        });
        self.set_presence(datasource_config_id, &owner, jti).await;
        if let Err(e) = self.add_active_generation(datasource_config_id, jti, &owner).await {
            self.unregister(datasource_config_id, connection_id).await;
            return Err(e);
        }
        // Cover a revocation that raced with registration.
        match self.is_revoked(datasource_config_id, jti).await {
            Ok(false) => {}
            Ok(true) => {
                self.unregister(datasource_config_id, connection_id).await;
                return Err(kyomi_core::Error::ServiceUnavailable("Connect token revoked".into()));
            }
            Err(e) => {
                self.unregister(datasource_config_id, connection_id).await;
                return Err(e);
            }
        }
        Ok((connection_id, receiver))
    }

    async fn add_active_generation(&self, datasource_config_id: &str, jti: &str, owner: &str) -> kyomi_core::Result<()> {
        let Some(redis) = &self.redis else { return Ok(()); };
        let mut conn = redis.clone();
        let key = active_generation_key(datasource_config_id, jti);
        // No TTL: a blocked WebSocket send can outlive heartbeat/presence
        // leases. Only owner unregister proves it can no longer flush work.
        redis::cmd("SADD").arg(&key).arg(owner).query_async::<i64>(&mut conn).await
            .map_err(|e| kyomi_core::Error::ServiceUnavailable(format!("Connect active session registration failed: {e}")))?;
        Ok(())
    }

    async fn set_presence(&self, datasource_config_id: &str, owner: &str, jti: &str) {
        if let Some(ref redis_pool) = self.redis {
            let mut conn = redis_pool.clone();
            if let Err(e) = redis::cmd("SET")
                .arg(presence_key(datasource_config_id))
                .arg(presence_value(owner, jti))
                .arg("EX")
                .arg(60)
                .query_async::<()>(&mut conn)
                .await
            {
                tracing::warn!(datasource_config_id, error = %e, "Failed to set Redis presence key for Connect");
            }
        }
    }

    /// Revoke one token generation locally and across Redis replicas. Redis
    /// stores a durable marker, so a lost pub/sub event cannot authorize work.
    pub async fn revoke_generation(&self, datasource_config_id: &str, jti: &str) -> kyomi_core::Result<()> {
        self.revoke_local(datasource_config_id, jti);
        if let Some(redis) = &self.redis {
            let mut conn = redis.clone();
            redis::cmd("SET")
                .arg(revoked_key(datasource_config_id, jti))
                .arg(1)
                .query_async::<()>(&mut conn)
                .await
                .map_err(|e| kyomi_core::Error::ServiceUnavailable(format!("Connect revocation storage failed: {e}")))?;
            redis::cmd("PUBLISH")
                .arg(format!("{REVOKE_CHANNEL_PREFIX}{datasource_config_id}"))
                .arg(jti)
                .query_async::<i64>(&mut conn)
                .await
                .map_err(|e| kyomi_core::Error::ServiceUnavailable(format!("Connect revocation publish failed: {e}")))?;
        }
        // A successful mutation response means the old handler has dropped its
        // pending channels and can no longer send a queued WebSocket command.
        tokio::time::timeout(Duration::from_secs(5), async {
            let mut recovery_cursor = 0;
            loop {
                recovery_cursor = self.recover_owner_page(
                    &active_generation_key(datasource_config_id, jti), recovery_cursor,
                ).await?;
                // A reconnect replaces the routing entry before its old
                // handler has necessarily finished a blocked WebSocket send.
                let local_gone = !self.lifecycle.iter().any(|entry| {
                    entry.datasource_config_id == datasource_config_id && entry.jti == jti
                });
                let remote_gone = if let Some(redis) = &self.redis {
                    let mut conn = redis.clone();
                    let count: i64 = redis::cmd("SCARD")
                        .arg(active_generation_key(datasource_config_id, jti))
                        .query_async(&mut conn).await
                        .map_err(|e| kyomi_core::Error::ServiceUnavailable(format!("Connect revocation confirmation failed: {e}")))?;
                    count == 0
                } else { true };
                if local_gone && remote_gone { return Ok(()); }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }).await.map_err(|_| kyomi_core::Error::ServiceUnavailable("Connect revocation was not confirmed before timeout".into()))?
    }

    fn revoke_local(&self, datasource_config_id: &str, jti: &str) {
        if let Some(session) = self.connections.get(datasource_config_id) {
            if session.jti != jti { return; }
            let _ = session.revoked.send(true);
        }
    }

    /// Fail closed when Redis cannot confirm whether this generation is revoked.
    pub async fn is_revoked(&self, datasource_config_id: &str, jti: &str) -> kyomi_core::Result<bool> {
        let Some(redis) = &self.redis else { return Ok(false); };
        let mut conn = redis.clone();
        redis::cmd("EXISTS")
            .arg(revoked_key(datasource_config_id, jti))
            .query_async::<bool>(&mut conn)
            .await
            .map_err(|e| kyomi_core::Error::ServiceUnavailable(format!("Connect revocation check failed: {e}")))
    }

    /// Start a Redis command subscriber for the given datasource.
    ///
    /// The subscriber listens on `connect:cmd:{datasource_config_id}` for
    /// commands published by other pods. When it receives a command, it
    /// forwards it through the local mpsc channel to the WebSocket handler,
    /// waits for the response, and publishes it back on
    /// `connect:res:{request_id}`.
    ///
    /// Must be called after `register()`. The subscriber is cleaned up by
    /// `unregister()` when the WebSocket disconnects.
    ///
    /// If a subscriber already exists for this datasource (e.g. rapid
    /// reconnect), the old one is aborted first.
    pub fn start_command_subscriber(&self, datasource_config_id: &str, connection_id: u64) {
        // Abort any existing subscriber for this datasource to prevent duplicates
        if let Some((_, (_, old_handle))) = self.subscribers.remove(datasource_config_id) {
            old_handle.abort();
            tracing::debug!(
                datasource_config_id,
                "Aborted previous command subscriber (superseded by reconnect)"
            );
        }
        let dsid = datasource_config_id.to_string();
        let channel = format!("{CMD_CHANNEL_PREFIX}{dsid}");
        let revoke_channel = format!("{REVOKE_CHANNEL_PREFIX}{dsid}");
        // In single-instance mode (no Redis) cross-replica subscriber is a no-op.
        let Some(redis_url) = self.redis_url.clone() else {
            tracing::debug!(
                datasource_config_id,
                "Connect command subscriber skipped — no Redis configured (single-instance mode)"
            );
            self.subscribers.insert(
                datasource_config_id.to_string(),
                (connection_id, tokio::spawn(async {})),
            );
            return;
        };
        let redis_pool = self.redis.clone().expect("redis_url implies redis pool");
        let connections = self.connections.clone();
        let registry = self.clone();

        let handle = tokio::spawn(async move {
            // Create a dedicated Redis connection for SUBSCRIBE.
            let client = match redis::Client::open(redis_url.as_str()) {
                Ok(c) => c,
                Err(e) => {
                    tracing::error!(
                        datasource_config_id = %dsid,
                        error = %e,
                        "Redis subscriber client creation failed for Connect command channel"
                    );
                    return;
                }
            };

            let mut pubsub = match client.get_async_pubsub().await {
                Ok(ps) => ps,
                Err(e) => {
                    tracing::error!(
                        datasource_config_id = %dsid,
                        error = %e,
                        "Redis SUBSCRIBE connection failed for Connect command channel"
                    );
                    return;
                }
            };

            if let Err(e) = pubsub.subscribe(&channel).await {
                tracing::error!(
                    datasource_config_id = %dsid,
                    error = %e,
                    "Redis SUBSCRIBE to {channel} failed"
                );
                return;
            }
            if let Err(e) = pubsub.subscribe(&revoke_channel).await {
                tracing::error!(datasource_config_id = %dsid, error = %e, "Redis SUBSCRIBE to revocation channel failed");
                return;
            }

            tracing::debug!(
                datasource_config_id = %dsid,
                "Connect command subscriber started on {channel}"
            );

            let mut stream = pubsub.on_message();

            while let Some(msg) = stream.next().await {
                if msg.get_channel_name() == revoke_channel {
                    if let Ok(jti) = msg.get_payload::<String>() {
                        registry.revoke_local(&dsid, &jti);
                    }
                    continue;
                }
                let payload: String = match msg.get_payload() {
                    Ok(p) => p,
                    Err(e) => {
                        tracing::warn!(
                            datasource_config_id = %dsid,
                            error = %e,
                            "Bad Redis pub/sub payload on Connect command channel"
                        );
                        continue;
                    }
                };

                // Deserialize the command
                let request: ConnectRequest = match serde_json::from_str(&payload) {
                    Ok(r) => r,
                    Err(e) => {
                        tracing::warn!(
                            datasource_config_id = %dsid,
                            error = %e,
                            "Failed to deserialize ConnectRequest from Redis"
                        );
                        continue;
                    }
                };

                let request_id = request.id.clone();

                // A durable marker protects against missed revocation messages.
                if let Some(session) = registry.connections.get(&dsid) {
                    let jti = session.jti.clone();
                    drop(session);
                    if !matches!(registry.is_revoked(&dsid, &jti).await, Ok(false)) {
                        registry.revoke_local(&dsid, &jti);
                        publish_response(&redis_pool, &request_id, &ConnectResponse {
                            id: request_id.clone(),
                            body: kyomi_core::connect_protocol::ConnectResponseBody::Error {
                                error: "Connect token revoked".into(),
                            },
                        }).await;
                        continue;
                    }
                }

                // Get the local command sender
                let sender = match connections.get(&dsid) {
                    Some(entry) => entry.sender.clone(),
                    None => {
                        tracing::warn!(
                            datasource_config_id = %dsid,
                            request_id = %request_id,
                            "Received remote command but local connection is gone"
                        );
                        // Publish an error response so the caller doesn't hang
                        let error_response = ConnectResponse {
                            id: request_id.clone(),
                            body: kyomi_core::connect_protocol::ConnectResponseBody::Error {
                                error: "Connect instance disconnected during cross-replica routing".into(),
                            },
                        };
                        publish_response(&redis_pool, &request_id, &error_response).await;
                        continue;
                    }
                };

                if request.streaming {
                    // Streaming: use mpsc channel to receive multiple responses,
                    // publish each one to Redis until terminal.
                    let (stream_tx, mut stream_rx) = mpsc::channel::<ConnectResponse>(32);
                    if sender.send((request, ResponseChannel::Stream(stream_tx))).await.is_err() {
                        tracing::warn!(
                            datasource_config_id = %dsid,
                            request_id = %request_id,
                            "Local command channel closed during cross-replica streaming forwarding"
                        );
                        let error_response = ConnectResponse {
                            id: request_id.clone(),
                            body: kyomi_core::connect_protocol::ConnectResponseBody::Error {
                                error: "Connect instance disconnected during command forwarding".into(),
                            },
                        };
                        publish_response(&redis_pool, &request_id, &error_response).await;
                        continue;
                    }

                    let redis_pool_clone = redis_pool.clone();
                    let dsid_clone = dsid.clone();
                    tokio::spawn(async move {
                        while let Some(response) = stream_rx.recv().await {
                            let is_terminal = matches!(
                                &response.body,
                                kyomi_core::connect_protocol::ConnectResponseBody::Result { .. }
                                    | kyomi_core::connect_protocol::ConnectResponseBody::Error { .. }
                                    | kyomi_core::connect_protocol::ConnectResponseBody::ArrowComplete { .. }
                            );
                            publish_response(&redis_pool_clone, &request_id, &response).await;
                            if is_terminal {
                                break;
                            }
                        }
                        tracing::debug!(
                            datasource_config_id = %dsid_clone,
                            request_id = %request_id,
                            "Cross-replica streaming forwarding complete"
                        );
                    });
                } else {
                    // Non-streaming: use oneshot channel for single response
                    let (response_tx, response_rx) = oneshot::channel();
                    if sender.send((request, ResponseChannel::Once(response_tx))).await.is_err() {
                        tracing::warn!(
                            datasource_config_id = %dsid,
                            request_id = %request_id,
                            "Local command channel closed during cross-replica forwarding"
                        );
                        let error_response = ConnectResponse {
                            id: request_id.clone(),
                            body: kyomi_core::connect_protocol::ConnectResponseBody::Error {
                                error: "Connect instance disconnected during command forwarding".into(),
                            },
                        };
                        publish_response(&redis_pool, &request_id, &error_response).await;
                        continue;
                    }

                    let redis_pool_clone = redis_pool.clone();
                    let dsid_clone = dsid.clone();
                    tokio::spawn(async move {
                        match response_rx.await {
                            Ok(response) => {
                                publish_response(&redis_pool_clone, &request_id, &response).await;
                            }
                            Err(_) => {
                                tracing::warn!(
                                    datasource_config_id = %dsid_clone,
                                    request_id = %request_id,
                                    "Response channel dropped during cross-replica command"
                                );
                                let error_response = ConnectResponse {
                                    id: request_id.clone(),
                                    body: kyomi_core::connect_protocol::ConnectResponseBody::Error {
                                        error: "Connect handler dropped the response".into(),
                                    },
                                };
                                publish_response(&redis_pool_clone, &request_id, &error_response).await;
                            }
                        }
                    });
                }
            }

            tracing::debug!(
                datasource_config_id = %dsid,
                "Connect command subscriber ended"
            );
        });

        self.subscribers.insert(datasource_config_id.to_string(), (connection_id, handle));
    }

    /// Stop the Redis command subscriber for the given datasource.
    ///
    /// Only aborts the subscriber if it belongs to `connection_id`, preventing
    /// a stale disconnect from killing a newer connection's subscriber.
    fn stop_command_subscriber(&self, datasource_config_id: &str, connection_id: u64) {
        let removed = self
            .subscribers
            .remove_if(datasource_config_id, |_key, (id, _handle)| {
                *id == connection_id
            });

        if let Some((_, (_, handle))) = removed {
            handle.abort();
            tracing::debug!(
                datasource_config_id,
                connection_id,
                "Connect command subscriber stopped"
            );
        }
    }

    /// Unregister a Connect instance on disconnect.
    ///
    /// Only removes the entry if it still belongs to `connection_id`.  This
    /// prevents a stale disconnect (old WebSocket closing after a reconnect)
    /// from evicting the newer connection's entry.
    ///
    /// Clears this connection's Redis generation membership even when a newer
    /// local routing entry has replaced it. Presence deletion is owner-checked.
    pub async fn unregister(&self, datasource_config_id: &str, connection_id: u64) {
        let Some(lifecycle) = self.lifecycle.get(&connection_id).map(|entry| entry.clone()) else {
            return;
        };
        if lifecycle.datasource_config_id != datasource_config_id {
            return;
        }

        let removed_current = self
            .connections
            .remove_if(datasource_config_id, |_key, session| {
                session.connection_id == connection_id
            }).is_some();

        if !removed_current {
            tracing::debug!(
                datasource_config_id,
                connection_id,
                "Unregistering superseded Connect generation without removing replacement"
            );
        }

        // Stop the command subscriber for this datasource (only if it belongs to us)
        self.stop_command_subscriber(datasource_config_id, connection_id);

        if let Err(e) = self.clear_redis_lifecycle(&lifecycle).await {
            tracing::warn!(datasource_config_id, connection_id, error = %e, "Connect Redis cleanup deferred");
            let registry = self.clone();
            tokio::spawn(async move {
                loop {
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    if registry.clear_redis_lifecycle(&lifecycle).await.is_ok() {
                        registry.lifecycle.remove(&connection_id);
                        break;
                    }
                }
            });
        } else {
            self.lifecycle.remove(&connection_id);
        }

        tracing::info!(datasource_config_id, "Connect instance unregistered");
    }

    async fn clear_redis_lifecycle(&self, lifecycle: &ConnectionLifecycle) -> kyomi_core::Result<()> {
        let Some(redis) = &self.redis else { return Ok(()); };
        let mut conn = redis.clone();
        if !lifecycle.jti.is_empty() {
            redis::cmd("SREM")
                .arg(active_generation_key(&lifecycle.datasource_config_id, &lifecycle.jti))
                .arg(&lifecycle.owner)
                .query_async::<i64>(&mut conn).await
                .map_err(|e| kyomi_core::Error::ServiceUnavailable(format!("Connect active generation cleanup failed: {e}")))?;
        }
        // Compare-and-delete cannot remove a newer owner, including one on a
        // different replica whose process-local connection ID is identical.
        redis::Script::new(
            "if redis.call('GET', KEYS[1]) == ARGV[1] then return redis.call('DEL', KEYS[1]) else return 0 end"
        )
            .key(presence_key(&lifecycle.datasource_config_id))
            .arg(presence_value(&lifecycle.owner, &lifecycle.jti))
            .invoke_async::<i64>(&mut conn).await
            .map_err(|e| kyomi_core::Error::ServiceUnavailable(format!("Connect presence cleanup failed: {e}")))?;
        Ok(())
    }

    /// Send a command to the Connect instance for the given datasource and wait
    /// for the response.
    ///
    /// Tries the local `DashMap` first (fast path). If the connection is not
    /// local but exists on another pod (Redis presence key), forwards the
    /// command via Redis pub/sub.
    ///
    /// Returns an error if:
    /// - No Connect instance is registered for this datasource (offline)
    /// - The command channel is closed (Connect disconnected mid-flight)
    /// - The response times out
    pub async fn send_command(
        &self,
        datasource_config_id: &str,
        request: ConnectRequest,
        timeout: Duration,
    ) -> kyomi_core::Result<ConnectResponse> {
        self.ensure_current(datasource_config_id).await?;
        // Fast path: local connection
        if let Some(entry) = self.connections.get(datasource_config_id) {
            let sender = entry.sender.clone();
            drop(entry); // Release DashMap read lock before async work

            return self.send_command_local(datasource_config_id, sender, request, timeout).await;
        }

        // Slow path: check if connection exists on another pod via Redis.
        // In single-instance mode (no Redis), cross-replica routing is unavailable —
        // the connection must be local or it doesn't exist.
        let Some(ref redis_pool) = self.redis else {
            return Err(kyomi_core::Error::ServiceUnavailable(format!(
                "Connect instance for datasource '{datasource_config_id}' is offline"
            )));
        };
        let key = presence_key(datasource_config_id);
        let mut conn = redis_pool.clone();
        let exists: bool = match redis::cmd("EXISTS")
            .arg(&key)
            .query_async::<i64>(&mut conn)
            .await
        {
            Ok(1) => true,
            Ok(_) => false,
            Err(e) => {
                tracing::warn!(
                    datasource_config_id,
                    error = %e,
                    "Redis EXISTS check failed during send_command"
                );
                false
            }
        };

        if !exists {
            return Err(kyomi_core::Error::ServiceUnavailable(format!(
                "Connect instance for datasource '{datasource_config_id}' is offline"
            )));
        }

        // Connection is on another pod — route via Redis pub/sub
        self.send_command_remote(datasource_config_id, request, timeout).await
    }

    /// Send a command through a local mpsc channel and wait for the response.
    async fn send_command_local(
        &self,
        datasource_config_id: &str,
        sender: CommandSender,
        request: ConnectRequest,
        timeout: Duration,
    ) -> kyomi_core::Result<ConnectResponse> {
        let (response_tx, response_rx) = oneshot::channel();

        sender
            .send((request, ResponseChannel::Once(response_tx)))
            .await
            .map_err(|_| {
                kyomi_core::Error::ServiceUnavailable(format!(
                    "Connect instance for datasource '{datasource_config_id}' disconnected"
                ))
            })?;

        match tokio::time::timeout(timeout, response_rx).await {
            Ok(Ok(response)) => Ok(response),
            Ok(Err(_recv_error)) => Err(kyomi_core::Error::ServiceUnavailable(format!(
                "Connect instance for datasource '{datasource_config_id}' dropped the response channel"
            ))),
            Err(_timeout) => Err(kyomi_core::Error::ServiceUnavailable(format!(
                "Connect command timed out after {}s for datasource '{datasource_config_id}'",
                timeout.as_secs()
            ))),
        }
    }

    /// Send a streaming command and return a receiver for multiple responses.
    ///
    /// Used by `ConnectProvider::execute_query()` to receive streaming
    /// responses (ArrowHeader → ArrowBatch* → ArrowComplete) from the Connect agent.
    ///
    /// The local path creates an mpsc channel and sends it through the command
    /// channel. The handler routes each response message through the mpsc.
    /// The receiver is closed when the handler removes the pending entry
    /// (on ArrowComplete or Error) or the WebSocket disconnects.
    pub async fn send_command_streaming(
        &self,
        datasource_config_id: &str,
        request: ConnectRequest,
        timeout: Duration,
    ) -> kyomi_core::Result<mpsc::Receiver<ConnectResponse>> {
        self.ensure_current(datasource_config_id).await?;
        // Fast path: local connection
        if let Some(entry) = self.connections.get(datasource_config_id) {
            let sender = entry.sender.clone();
            drop(entry);

            let (stream_tx, stream_rx) = mpsc::channel(32);

            sender
                .send((request, ResponseChannel::Stream(stream_tx)))
                .await
                .map_err(|_| {
                    kyomi_core::Error::ServiceUnavailable(format!(
                        "Connect instance for datasource '{datasource_config_id}' disconnected"
                    ))
                })?;

            Ok(stream_rx)
        } else {
            // Check remote presence. In single-instance mode (no Redis), only local
            // connections are visible — if not found locally, it's offline.
            let Some(ref redis_pool) = self.redis else {
                return Err(kyomi_core::Error::ServiceUnavailable(format!(
                    "Connect instance for datasource '{datasource_config_id}' is offline"
                )));
            };
            let key = presence_key(datasource_config_id);
            let mut conn = redis_pool.clone();
            let exists: bool = matches!(redis::cmd("EXISTS")
                .arg(&key)
                .query_async::<i64>(&mut conn)
                .await, Ok(1));

            if !exists {
                return Err(kyomi_core::Error::ServiceUnavailable(format!(
                    "Connect instance for datasource '{datasource_config_id}' is offline"
                )));
            }

            // Connection is on another pod — route via Redis pub/sub streaming
            self.send_command_streaming_remote(datasource_config_id, request, timeout).await
        }
    }

    async fn ensure_current(&self, datasource_config_id: &str) -> kyomi_core::Result<()> {
        if let Some(session) = self.connections.get(datasource_config_id) {
            let jti = session.jti.clone();
            let revoked = *session.revoked.borrow();
            drop(session);
            if revoked || self.is_revoked(datasource_config_id, &jti).await? {
                self.revoke_local(datasource_config_id, &jti);
                return Err(kyomi_core::Error::ServiceUnavailable("Connect token revoked".into()));
            }
        }
        Ok(())
    }

    /// Send a streaming command to a Connect instance on another pod via Redis pub/sub.
    ///
    /// Similar to `send_command_remote`, but keeps the subscription open for multiple
    /// response messages (ArrowHeader → ArrowBatch* → ArrowComplete). Returns an
    /// mpsc receiver that the caller consumes as a stream. A background task reads
    /// from Redis pub/sub and forwards each message until a terminal response arrives.
    async fn send_command_streaming_remote(
        &self,
        datasource_config_id: &str,
        request: ConnectRequest,
        timeout: Duration,
    ) -> kyomi_core::Result<mpsc::Receiver<ConnectResponse>> {
        let request_id = request.id.clone();
        let res_channel = format!("{RES_CHANNEL_PREFIX}{request_id}");
        let cmd_channel = format!("{CMD_CHANNEL_PREFIX}{datasource_config_id}");

        // Cross-replica streaming requires Redis — should not be called in local-only mode.
        let redis_url = self.redis_url.as_deref().ok_or_else(|| {
            kyomi_core::Error::Internal(
                "send_command_streaming_remote called without Redis (single-instance mode)".into()
            )
        })?;
        let redis_pool = self.redis.as_ref().expect("redis_url implies redis pool");

        // Create a dedicated Redis connection for subscribing to responses
        let client = redis::Client::open(redis_url).map_err(|e| {
            kyomi_core::Error::Internal(format!(
                "Failed to create Redis client for cross-replica streaming: {e}"
            ))
        })?;

        let mut pubsub = client.get_async_pubsub().await.map_err(|e| {
            kyomi_core::Error::Internal(format!(
                "Failed to connect Redis pub/sub for cross-replica streaming: {e}"
            ))
        })?;

        // Subscribe to the response channel BEFORE publishing the command
        pubsub.subscribe(&res_channel).await.map_err(|e| {
            kyomi_core::Error::Internal(format!(
                "Failed to subscribe to Redis response channel for streaming: {e}"
            ))
        })?;

        // Publish the command to the pod holding the WebSocket
        let request_json = serde_json::to_string(&request).map_err(|e| {
            kyomi_core::Error::Internal(format!(
                "Failed to serialize ConnectRequest for cross-replica streaming: {e}"
            ))
        })?;

        let mut conn = redis_pool.clone();
        let listeners: i64 = redis::cmd("PUBLISH")
            .arg(&cmd_channel)
            .arg(&request_json)
            .query_async(&mut conn)
            .await
            .map_err(|e| {
                kyomi_core::Error::Internal(format!(
                    "Failed to publish streaming command to Redis: {e}"
                ))
            })?;

        if listeners == 0 {
            return Err(kyomi_core::Error::ServiceUnavailable(format!(
                "Connect instance for datasource '{datasource_config_id}' is offline (no listeners on command channel)"
            )));
        }

        // Create mpsc channel to forward Redis messages to the caller
        let (stream_tx, stream_rx) = mpsc::channel::<ConnectResponse>(32);
        let dsid = datasource_config_id.to_string();

        // Spawn a background task that reads from Redis pub/sub and forwards
        // each response through the mpsc channel until a terminal message
        tokio::spawn(async move {
            let mut redis_stream = pubsub.on_message();
            let deadline = tokio::time::Instant::now() + timeout;

            loop {
                match tokio::time::timeout_at(deadline, redis_stream.next()).await {
                    Ok(Some(msg)) => {
                        let payload: String = match msg.get_payload() {
                            Ok(p) => p,
                            Err(e) => {
                                tracing::warn!(
                                    datasource_config_id = %dsid,
                                    request_id = %request_id,
                                    error = %e,
                                    "Bad Redis payload in cross-replica streaming"
                                );
                                break;
                            }
                        };

                        let response: ConnectResponse = match serde_json::from_str(&payload) {
                            Ok(r) => r,
                            Err(e) => {
                                tracing::warn!(
                                    datasource_config_id = %dsid,
                                    request_id = %request_id,
                                    error = %e,
                                    "Failed to deserialize streaming response from Redis"
                                );
                                break;
                            }
                        };

                        let is_terminal = matches!(
                            &response.body,
                            kyomi_core::connect_protocol::ConnectResponseBody::Result { .. }
                                | kyomi_core::connect_protocol::ConnectResponseBody::Error { .. }
                                | kyomi_core::connect_protocol::ConnectResponseBody::ArrowComplete { .. }
                        );

                        if stream_tx.send(response).await.is_err() {
                            // Receiver dropped — caller no longer interested
                            tracing::debug!(
                                datasource_config_id = %dsid,
                                request_id = %request_id,
                                "Streaming receiver dropped during cross-replica forwarding"
                            );
                            break;
                        }

                        if is_terminal {
                            break;
                        }
                    }
                    Ok(None) => {
                        // Redis subscription ended unexpectedly
                        tracing::warn!(
                            datasource_config_id = %dsid,
                            request_id = %request_id,
                            "Redis subscription ended during cross-replica streaming"
                        );
                        break;
                    }
                    Err(_timeout) => {
                        tracing::warn!(
                            datasource_config_id = %dsid,
                            request_id = %request_id,
                            timeout_secs = timeout.as_secs(),
                            "Cross-replica streaming timed out"
                        );
                        // Send an error through the stream so the caller knows
                        let _ = stream_tx.send(ConnectResponse {
                            id: request_id.clone(),
                            body: kyomi_core::connect_protocol::ConnectResponseBody::Error {
                                error: format!(
                                    "Connect streaming timed out after {}s (cross-replica)",
                                    timeout.as_secs()
                                ),
                            },
                        }).await;
                        break;
                    }
                }
            }
        });

        Ok(stream_rx)
    }

    /// Send a command to a Connect instance on another pod via Redis pub/sub.
    ///
    /// 1. Subscribe to `connect:res:{request_id}` for the response.
    /// 2. Publish the request to `connect:cmd:{datasource_config_id}`.
    /// 3. Wait for the response (with timeout).
    async fn send_command_remote(
        &self,
        datasource_config_id: &str,
        request: ConnectRequest,
        timeout: Duration,
    ) -> kyomi_core::Result<ConnectResponse> {
        let request_id = request.id.clone();
        let res_channel = format!("{RES_CHANNEL_PREFIX}{request_id}");
        let cmd_channel = format!("{CMD_CHANNEL_PREFIX}{datasource_config_id}");

        // Cross-replica routing requires Redis — should not be called in local-only mode.
        let redis_url = self.redis_url.as_deref().ok_or_else(|| {
            kyomi_core::Error::Internal(
                "send_command_remote called without Redis (single-instance mode)".into()
            )
        })?;
        let redis_pool = self.redis.as_ref().expect("redis_url implies redis pool");

        // Create a dedicated Redis connection for subscribing to the response
        let client = redis::Client::open(redis_url).map_err(|e| {
            kyomi_core::Error::Internal(format!(
                "Failed to create Redis client for cross-replica routing: {e}"
            ))
        })?;

        let mut pubsub = client.get_async_pubsub().await.map_err(|e| {
            kyomi_core::Error::Internal(format!(
                "Failed to connect Redis pub/sub for cross-replica routing: {e}"
            ))
        })?;

        // Subscribe to the response channel BEFORE publishing the command
        // to avoid a race where the response arrives before we're listening
        pubsub.subscribe(&res_channel).await.map_err(|e| {
            kyomi_core::Error::Internal(format!(
                "Failed to subscribe to Redis response channel: {e}"
            ))
        })?;

        // Publish the command to the pod holding the WebSocket
        let request_json = serde_json::to_string(&request).map_err(|e| {
            kyomi_core::Error::Internal(format!(
                "Failed to serialize ConnectRequest for cross-replica routing: {e}"
            ))
        })?;

        let mut conn = redis_pool.clone();
        let listeners: i64 = redis::cmd("PUBLISH")
            .arg(&cmd_channel)
            .arg(&request_json)
            .query_async(&mut conn)
            .await
            .map_err(|e| {
                kyomi_core::Error::Internal(format!(
                    "Failed to publish command to Redis: {e}"
                ))
            })?;

        if listeners == 0 {
            // No one is listening on the command channel — the Connect instance
            // may have disconnected between our EXISTS check and PUBLISH
            return Err(kyomi_core::Error::ServiceUnavailable(format!(
                "Connect instance for datasource '{datasource_config_id}' is offline (no listeners on command channel)"
            )));
        }

        // Wait for the response
        let mut stream = pubsub.on_message();
        match tokio::time::timeout(timeout, stream.next()).await {
            Ok(Some(msg)) => {
                let payload: String = msg.get_payload().map_err(|e| {
                    kyomi_core::Error::Internal(format!(
                        "Failed to read Redis response payload: {e}"
                    ))
                })?;

                let response: ConnectResponse = serde_json::from_str(&payload).map_err(|e| {
                    kyomi_core::Error::Internal(format!(
                        "Failed to deserialize ConnectResponse from Redis: {e}"
                    ))
                })?;

                Ok(response)
            }
            Ok(None) => Err(kyomi_core::Error::ServiceUnavailable(format!(
                "Redis response subscription ended unexpectedly for datasource '{datasource_config_id}'"
            ))),
            Err(_timeout) => Err(kyomi_core::Error::ServiceUnavailable(format!(
                "Connect command timed out after {}s for datasource '{datasource_config_id}' (cross-replica)",
                timeout.as_secs()
            ))),
        }
    }

    /// Check whether a Connect instance is online for the given datasource.
    ///
    /// Checks the local `DashMap` first (fast path), then falls back to a Redis
    /// key check for cross-replica awareness.
    pub async fn is_connected(&self, datasource_config_id: &str) -> bool {
        // Fast path: check local connections
        if self.connections.contains_key(datasource_config_id) {
            return true;
        }

        // Slow path: check Redis presence key (may be on another replica).
        // In single-instance mode (no Redis), only local connections are visible.
        let Some(ref redis_pool) = self.redis else {
            return false;
        };
        let key = presence_key(datasource_config_id);
        let mut conn = redis_pool.clone();
        match redis::cmd("EXISTS")
            .arg(&key)
            .query_async::<i64>(&mut conn)
            .await
        {
            Ok(1) => true,
            Ok(_) => false,
            Err(e) => {
                tracing::warn!(
                    datasource_config_id,
                    error = %e,
                    "Redis EXISTS check failed for Connect presence"
                );
                false
            }
        }
    }

    /// Refresh the Redis presence key for a Connect instance (called on heartbeat pong).
    ///
    /// Refreshes only the key owned by this connection. A stale socket must
    /// never replace a newer connection's presence key.
    pub async fn refresh_heartbeat(&self, datasource_config_id: &str, connection_id: u64) {
        let Some(session) = self.connections.get(datasource_config_id) else { return; };
        if session.connection_id != connection_id { return; }
        let value = presence_value(&session.owner, &session.jti);
        drop(session);
        // In single-instance mode (no Redis), presence keys are not used.
        let Some(ref redis_pool) = self.redis else {
            return;
        };
        let key = presence_key(datasource_config_id);
        let mut conn = redis_pool.clone();
        let script = redis::Script::new(
            "if redis.call('GET', KEYS[1]) == ARGV[1] then return redis.call('EXPIRE', KEYS[1], 60) else return 0 end"
        );
        if let Err(e) = script.key(&key).arg(value).invoke_async::<i64>(&mut conn).await {
            tracing::warn!(
                datasource_config_id,
                error = %e,
                "Failed to refresh Redis heartbeat for Connect"
            );
        }
    }
}

/// Build the Redis key for a Connect presence entry.
fn presence_key(datasource_config_id: &str) -> String {
    format!("connect:{datasource_config_id}")
}

fn presence_value(owner: &str, jti: &str) -> String {
    format!("{owner}|{jti}")
}

fn revoked_key(datasource_config_id: &str, jti: &str) -> String {
    format!("connect:revoked:{datasource_config_id}:{jti}")
}

fn active_generation_key(datasource_config_id: &str, jti: &str) -> String {
    format!("connect:active:{datasource_config_id}:{jti}")
}

/// Publish a ConnectResponse to the Redis response channel for cross-replica routing.
pub async fn publish_response(redis: &RedisPool, request_id: &str, response: &ConnectResponse) {
    let channel = format!("{RES_CHANNEL_PREFIX}{request_id}");
    let json = match serde_json::to_string(response) {
        Ok(j) => j,
        Err(e) => {
            tracing::error!(
                request_id,
                error = %e,
                "Failed to serialize ConnectResponse for Redis publish"
            );
            return;
        }
    };

    let mut conn = redis.clone();
    if let Err(e) = redis::cmd("PUBLISH")
        .arg(&channel)
        .arg(&json)
        .query_async::<i64>(&mut conn)
        .await
    {
        tracing::error!(
            request_id,
            error = %e,
            "Failed to publish ConnectResponse to Redis"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper to create a test registry with Redis.
    async fn test_registry() -> ConnectRegistry {
        let config = kyomi_core::Config::test_config();
        let redis_url = config.redis_url.unwrap_or_else(|| "redis://localhost:6380".into());
        let redis = kyomi_core::redis::create_pool(&redis_url)
            .await
            .expect("test Redis");
        ConnectRegistry::new(redis, redis_url)
    }

    fn test_request(id: &str) -> ConnectRequest {
        ConnectRequest {
            id: id.into(),
            op: kyomi_core::connect_protocol::ConnectOp::TestConnection,
            params: None,
            streaming: false,
        }
    }

    #[tokio::test]
    async fn local_rotation_revokes_pending_and_preserves_replacement() {
        let registry = ConnectRegistry::new_local();
        let dsid = "ds-local-revocation";
        let (old_tx, mut old_rx) = mpsc::channel(4);
        let (old_id, mut revoked) = registry.register_authenticated(dsid, "old-jti", old_tx).await.unwrap();
        let pending = tokio::spawn({
            let registry = registry.clone();
            async move { registry.send_command(dsid, test_request("pending"), Duration::from_secs(30)).await }
        });
        let (_request, response) = old_rx.recv().await.unwrap();

        let cleanup = tokio::spawn({
            let registry = registry.clone();
            async move {
                revoked.changed().await.unwrap();
                assert!(*revoked.borrow());
                drop(response); // synthetic socket closes and drops its pending command
                registry.unregister(dsid, old_id).await;
            }
        });
        registry.revoke_generation(dsid, "old-jti").await.unwrap();
        cleanup.await.unwrap();
        assert!(tokio::time::timeout(Duration::from_secs(1), pending).await.unwrap().unwrap().is_err());
        assert!(registry.send_command(dsid, test_request("late"), Duration::from_secs(1)).await.is_err());

        let (new_tx, mut new_rx) = mpsc::channel(4);
        let (new_id, new_revoked) = registry.register_authenticated(dsid, "new-jti", new_tx).await.unwrap();
        registry.unregister(dsid, old_id).await;
        registry.revoke_generation(dsid, "old-jti").await.unwrap();
        assert!(!*new_revoked.borrow());
        let command = tokio::spawn({
            let registry = registry.clone();
            async move { registry.send_command(dsid, test_request("new"), Duration::from_secs(1)).await }
        });
        let (request, response) = new_rx.recv().await.unwrap();
        response.send(ConnectResponse {
            id: request.id,
            body: kyomi_core::connect_protocol::ConnectResponseBody::Result { result: serde_json::json!(true) },
        }).unwrap();
        assert!(command.await.unwrap().is_ok());
        registry.unregister(dsid, new_id).await;
    }

    #[tokio::test]
    async fn replacing_socket_closes_old_revocation_watch() {
        let registry = ConnectRegistry::new_local();
        let (old_tx, _) = mpsc::channel(1);
        let (old_id, mut old_watch) = registry.register_authenticated("ds-replacement", "same-jti", old_tx).await.unwrap();
        let (new_tx, _) = mpsc::channel(1);
        let (new_id, new_watch) = registry.register_authenticated("ds-replacement", "same-jti", new_tx).await.unwrap();
        assert!(tokio::time::timeout(Duration::from_secs(1), old_watch.changed()).await.unwrap().is_err());
        assert!(!*new_watch.borrow());
        registry.unregister("ds-replacement", old_id).await;
        assert!(registry.is_connected("ds-replacement").await);
        registry.unregister("ds-replacement", new_id).await;
    }

    #[tokio::test]
    async fn local_revocation_waits_for_superseded_old_jti_handler() {
        let registry = ConnectRegistry::new_local();
        let dsid = "ds-local-superseded-revocation";
        let (old_tx, mut old_rx) = mpsc::channel(1);
        let (old_id, mut old_watch) = registry.register_authenticated(dsid, "old-jti", old_tx).await.unwrap();
        let pending = tokio::spawn({
            let registry = registry.clone();
            async move { registry.send_command(dsid, test_request("old-pending"), Duration::from_secs(30)).await }
        });
        let (_request, pending_response) = old_rx.recv().await.unwrap();

        let (new_tx, _) = mpsc::channel(1);
        let (new_id, new_watch) = registry.register_authenticated(dsid, "new-jti", new_tx).await.unwrap();
        assert!(old_watch.changed().await.is_err(), "replacement closes the old handler's watch");
        assert!(registry.lifecycle.contains_key(&old_id));

        let revocation = tokio::spawn({
            let registry = registry.clone();
            async move { registry.revoke_generation(dsid, "old-jti").await }
        });
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(!revocation.is_finished(), "old blocked handler still owns its lifecycle");
        assert!(!*new_watch.borrow());

        drop(pending_response);
        registry.unregister(dsid, old_id).await;
        tokio::time::timeout(Duration::from_secs(1), revocation).await.unwrap().unwrap().unwrap();
        assert!(tokio::time::timeout(Duration::from_secs(1), pending).await.unwrap().unwrap().is_err());
        assert!(registry.is_connected(dsid).await, "old cleanup cannot evict new JTI");
        registry.unregister(dsid, new_id).await;
    }

    #[tokio::test]
    async fn concurrent_authenticated_handshakes_keep_one_owner_and_generation() {
        let registry = test_registry().await;
        let dsid = format!("ds-concurrent-{}", uuid::Uuid::new_v4());
        let (a_tx, _a_rx) = mpsc::channel(1);
        let (b_tx, _b_rx) = mpsc::channel(1);
        let (a, b) = tokio::join!(
            registry.register_authenticated(&dsid, "jti-a", a_tx),
            registry.register_authenticated(&dsid, "jti-b", b_tx),
        );
        let (a_id, mut a_watch) = a.unwrap();
        let (b_id, mut b_watch) = b.unwrap();
        let session = registry.connections.get(&dsid).unwrap();
        let current_id = session.connection_id;
        let current_owner = session.owner.clone();
        let current_jti = session.jti.clone();
        drop(session);
        assert!(
            (current_id == a_id && current_jti == "jti-a")
                || (current_id == b_id && current_jti == "jti-b")
        );
        let mut redis = registry.redis.clone().unwrap();
        let owner: String = redis::cmd("GET").arg(presence_key(&dsid)).query_async(&mut redis).await.unwrap();
        assert_eq!(owner, presence_value(&current_owner, &current_jti));
        if current_id == a_id {
            assert!(tokio::time::timeout(Duration::from_secs(1), b_watch.changed()).await.unwrap().is_err());
            assert!(!*a_watch.borrow());
        } else {
            assert!(tokio::time::timeout(Duration::from_secs(1), a_watch.changed()).await.unwrap().is_err());
            assert!(!*b_watch.borrow());
        }
        registry.unregister(&dsid, a_id).await;
        registry.unregister(&dsid, b_id).await;
    }

    #[tokio::test]
    async fn superseded_same_and_new_jti_sessions_clear_only_their_own_membership() {
        let registry = test_registry().await;
        let dsid = format!("ds-reconnect-{}", uuid::Uuid::new_v4());
        let (first_tx, _) = mpsc::channel(1);
        let (first_id, mut first_watch) = registry.register_authenticated(&dsid, "first-jti", first_tx).await.unwrap();
        let (same_tx, _) = mpsc::channel(1);
        let (same_id, mut same_watch) = registry.register_authenticated(&dsid, "first-jti", same_tx).await.unwrap();
        assert!(first_watch.changed().await.is_err());

        let mut redis = registry.redis.clone().unwrap();
        let first_key = active_generation_key(&dsid, "first-jti");
        let count: i64 = redis::cmd("SCARD").arg(&first_key).query_async(&mut redis).await.unwrap();
        assert_eq!(count, 2);
        registry.unregister(&dsid, first_id).await;
        let count: i64 = redis::cmd("SCARD").arg(&first_key).query_async(&mut redis).await.unwrap();
        assert_eq!(count, 1);
        assert!(registry.is_connected(&dsid).await);

        let (new_tx, _) = mpsc::channel(1);
        let (new_id, new_watch) = registry.register_authenticated(&dsid, "new-jti", new_tx).await.unwrap();
        assert!(same_watch.changed().await.is_err());
        registry.unregister(&dsid, same_id).await;
        let count: i64 = redis::cmd("SCARD").arg(&first_key).query_async(&mut redis).await.unwrap();
        assert_eq!(count, 0);
        let new_count: i64 = redis::cmd("SCARD").arg(active_generation_key(&dsid, "new-jti"))
            .query_async(&mut redis).await.unwrap();
        assert_eq!(new_count, 1);
        registry.revoke_generation(&dsid, "first-jti").await.unwrap();
        assert!(!*new_watch.borrow());
        assert!(registry.is_connected(&dsid).await);
        registry.unregister(&dsid, new_id).await;
    }

    #[tokio::test]
    async fn equal_replica_local_ids_cannot_remove_another_redis_owner() {
        let config = kyomi_core::Config::test_config();
        let redis_url = config.redis_url.unwrap_or_else(|| "redis://localhost:6380".into());
        let redis = kyomi_core::redis::create_pool(&redis_url).await.expect("test Redis");
        let dsid = format!("ds-id-collision-{}", uuid::Uuid::new_v4());
        let pod_a = ConnectRegistry::new(redis.clone(), redis_url.clone());
        let pod_b = ConnectRegistry::new(redis.clone(), redis_url.clone());
        let writer = ConnectRegistry::new(redis.clone(), redis_url);
        let (a_tx, _) = mpsc::channel(1);
        let (b_tx, _) = mpsc::channel(1);
        let (a_id, _) = pod_a.register_authenticated(&dsid, "same-jti", a_tx).await.unwrap();
        let (b_id, _) = pod_b.register_authenticated(&dsid, "same-jti", b_tx).await.unwrap();
        assert_eq!(a_id, b_id, "separate replicas may assign the same local ID");
        let a_owner = pod_a.connections.get(&dsid).unwrap().owner.clone();
        let b_owner = pod_b.connections.get(&dsid).unwrap().owner.clone();
        assert_ne!(a_owner, b_owner);
        let key = active_generation_key(&dsid, "same-jti");
        let mut conn = redis;
        let count: i64 = redis::cmd("SCARD").arg(&key).query_async(&mut conn).await.unwrap();
        assert_eq!(count, 2);

        pod_a.unregister(&dsid, a_id).await;
        let count: i64 = redis::cmd("SCARD").arg(&key).query_async(&mut conn).await.unwrap();
        assert_eq!(count, 1, "old replica must not remove the other replica's member");
        let owner: String = redis::cmd("GET").arg(presence_key(&dsid)).query_async(&mut conn).await.unwrap();
        assert_eq!(owner, presence_value(&b_owner, "same-jti"));
        let revocation = tokio::spawn({
            let dsid = dsid.clone();
            async move { writer.revoke_generation(&dsid, "same-jti").await }
        });
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(!revocation.is_finished());
        pod_b.unregister(&dsid, b_id).await;
        tokio::time::timeout(Duration::from_secs(2), revocation).await.unwrap().unwrap().unwrap();
    }

    #[tokio::test]
    async fn failed_redis_cleanup_retains_identity_and_retries() {
        let registry = test_registry().await;
        let dsid = format!("ds-cleanup-retry-{}", uuid::Uuid::new_v4());
        let (tx, _) = mpsc::channel(1);
        let (id, _) = registry.register_authenticated(&dsid, "old-jti", tx).await.unwrap();
        let key = active_generation_key(&dsid, "old-jti");
        let mut conn = registry.redis.clone().unwrap();
        // Wrong Redis type forces SREM to fail; restore it after unregister.
        redis::cmd("SET").arg(&key).arg("temporary-bad-type")
            .query_async::<()>(&mut conn).await.unwrap();
        registry.unregister(&dsid, id).await;
        assert!(registry.lifecycle.contains_key(&id), "failed cleanup must retain the UUID");
        redis::cmd("DEL").arg(&key).query_async::<i64>(&mut conn).await.unwrap();
        tokio::time::timeout(Duration::from_secs(3), async {
            while registry.lifecycle.contains_key(&id) {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }).await.unwrap();
        let present: i64 = redis::cmd("EXISTS").arg(presence_key(&dsid))
            .query_async(&mut conn).await.unwrap();
        assert_eq!(present, 0);
    }

    #[tokio::test]
    async fn redis_rotation_reaches_other_replica_and_blocks_commands() {
        let config = kyomi_core::Config::test_config();
        let redis_url = config.redis_url.unwrap_or_else(|| "redis://localhost:6380".into());
        let redis = kyomi_core::redis::create_pool(&redis_url).await.expect("test Redis");
        let dsid = format!("ds-redis-revocation-{}", uuid::Uuid::new_v4());
        let holder = ConnectRegistry::new(redis.clone(), redis_url.clone());
        let writer = ConnectRegistry::new(redis, redis_url);
        let (tx, _rx) = mpsc::channel(4);
        let (id, mut revoked) = holder.register_authenticated(&dsid, "old-jti", tx).await.unwrap();
        holder.start_command_subscriber(&dsid, id);
        tokio::time::sleep(Duration::from_millis(100)).await;

        let cleanup = tokio::spawn({
            let holder = holder.clone();
            let dsid = dsid.clone();
            async move {
                revoked.changed().await.unwrap();
                assert!(*revoked.borrow());
                holder.unregister(&dsid, id).await;
            }
        });
        writer.revoke_generation(&dsid, "old-jti").await.unwrap();
        cleanup.await.unwrap();
        assert!(holder.send_command(&dsid, test_request("late"), Duration::from_secs(1)).await.is_err());
        let remote_denied = tokio::time::timeout(Duration::from_secs(2),
            writer.send_command(&dsid, test_request("remote-late"), Duration::from_secs(30)))
            .await.expect("revoked remote command must fail promptly");
        let message = match remote_denied {
            Err(error) => error.to_string(),
            Ok(ConnectResponse { body: kyomi_core::connect_protocol::ConnectResponseBody::Error { error }, .. }) => error,
            Ok(response) => panic!("revoked command returned success: {response:?}"),
        };
        assert!(message.contains("offline") || message.contains("revoked"), "unexpected denial: {message}");
        let (old_attempt, _) = mpsc::channel(4);
        assert!(holder.register_authenticated(&dsid, "old-jti", old_attempt).await.is_err());

        let (replacement_tx, mut replacement_rx) = mpsc::channel(4);
        let (replacement_id, replacement_revoked) = holder.register_authenticated(&dsid, "new-jti", replacement_tx).await.unwrap();
        holder.start_command_subscriber(&dsid, replacement_id);
        holder.refresh_heartbeat(&dsid, id).await;
        let mut conn = holder.redis.clone().unwrap();
        let owner: String = redis::cmd("GET").arg(presence_key(&dsid)).query_async(&mut conn).await.unwrap();
        let replacement_owner = holder.connections.get(&dsid).unwrap().owner.clone();
        assert_eq!(owner, presence_value(&replacement_owner, "new-jti"));
        holder.unregister(&dsid, id).await;
        writer.revoke_generation(&dsid, "old-jti").await.unwrap();
        assert!(!*replacement_revoked.borrow());
        tokio::time::sleep(Duration::from_millis(100)).await;
        let remote = tokio::spawn({
            let writer = writer.clone();
            let dsid = dsid.clone();
            async move { writer.send_command(&dsid, test_request("replacement"), Duration::from_secs(2)).await }
        });
        let (request, response) = tokio::time::timeout(Duration::from_secs(2), replacement_rx.recv()).await.unwrap().unwrap();
        response.send(ConnectResponse {
            id: request.id,
            body: kyomi_core::connect_protocol::ConnectResponseBody::Result { result: serde_json::json!(true) },
        }).unwrap();
        assert!(remote.await.unwrap().is_ok());
        holder.unregister(&dsid, replacement_id).await;
    }

    #[tokio::test]
    async fn redis_revocation_waits_for_old_generation_when_replacement_owns_presence() {
        let config = kyomi_core::Config::test_config();
        let redis_url = config.redis_url.unwrap_or_else(|| "redis://localhost:6380".into());
        let redis = kyomi_core::redis::create_pool(&redis_url).await.expect("test Redis");
        let dsid = format!("ds-overlap-{}", uuid::Uuid::new_v4());
        let old_holder = ConnectRegistry::new(redis.clone(), redis_url.clone());
        let new_holder = ConnectRegistry::new(redis.clone(), redis_url.clone());
        let writer = ConnectRegistry::new(redis, redis_url);
        let (old_tx, _) = mpsc::channel(1);
        let (old_id, mut old_watch) = old_holder.register_authenticated(&dsid, "old-jti", old_tx).await.unwrap();
        old_holder.start_command_subscriber(&dsid, old_id);
        tokio::time::sleep(Duration::from_millis(100)).await;
        let (new_tx, _) = mpsc::channel(1);
        let (new_id, new_watch) = new_holder.register_authenticated(&dsid, "new-jti", new_tx).await.unwrap();
        new_holder.start_command_subscriber(&dsid, new_id);
        tokio::time::sleep(Duration::from_millis(100)).await;
        let (release, released) = oneshot::channel();
        let cleanup = tokio::spawn({
            let old_holder = old_holder.clone();
            let dsid = dsid.clone();
            async move {
                old_watch.changed().await.unwrap();
                released.await.unwrap();
                old_holder.unregister(&dsid, old_id).await;
            }
        });
        let revocation = tokio::spawn({
            let writer = writer.clone();
            let dsid = dsid.clone();
            async move { writer.revoke_generation(&dsid, "old-jti").await }
        });
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(!revocation.is_finished(), "replacement presence must not hide old session");
        release.send(()).unwrap();
        cleanup.await.unwrap();
        tokio::time::timeout(Duration::from_secs(2), revocation).await.unwrap().unwrap().unwrap();
        assert!(!*new_watch.borrow());
        assert!(new_holder.is_connected(&dsid).await);
        new_holder.unregister(&dsid, new_id).await;
    }

    #[tokio::test]
    async fn redis_revocation_does_not_treat_expired_presence_as_closed_session() {
        let config = kyomi_core::Config::test_config();
        let redis_url = config.redis_url.unwrap_or_else(|| "redis://localhost:6380".into());
        let redis = kyomi_core::redis::create_pool(&redis_url).await.expect("test Redis");
        let dsid = format!("ds-expired-presence-{}", uuid::Uuid::new_v4());
        let holder = ConnectRegistry::new(redis.clone(), redis_url.clone());
        let writer = ConnectRegistry::new(redis.clone(), redis_url);
        let (tx, _rx) = mpsc::channel(1);
        let (id, _watch) = holder.register_authenticated(&dsid, "old-jti", tx).await.unwrap();
        let active_key = active_generation_key(&dsid, "old-jti");
        let mut conn = redis;
        let ttl: i64 = redis::cmd("TTL").arg(&active_key).query_async(&mut conn).await.unwrap();
        assert_eq!(ttl, -1, "active generation must not expire while a send is blocked");
        redis::cmd("DEL").arg(presence_key(&dsid)).query_async::<i64>(&mut conn).await.unwrap();

        let revocation = tokio::spawn({
            let dsid = dsid.clone();
            async move { writer.revoke_generation(&dsid, "old-jti").await }
        });
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(!revocation.is_finished(), "missing presence cannot prove the socket closed");
        holder.unregister(&dsid, id).await;
        tokio::time::timeout(Duration::from_secs(2), revocation).await.unwrap().unwrap().unwrap();
    }

    #[test]
    fn redis_key_format() {
        assert_eq!(presence_key("ds-abc123"), "connect:ds-abc123");
    }

    #[tokio::test]
    async fn send_command_fails_when_not_registered() {
        let registry = test_registry().await;

        let request = ConnectRequest {
            id: "test-1".into(),
            op: kyomi_core::connect_protocol::ConnectOp::TestConnection,
            params: None,
            streaming: false,
        };

        let result = registry
            .send_command("ds-nonexistent", request, Duration::from_secs(5))
            .await;

        assert!(result.is_err());
        let err_msg = format!("{}", result.unwrap_err());
        assert!(
            err_msg.contains("offline"),
            "expected 'offline' in error, got: {err_msg}"
        );
    }

    #[tokio::test]
    async fn register_and_send_command_roundtrip() {
        let registry = test_registry().await;

        // Create the command channel
        let (cmd_tx, mut cmd_rx) = mpsc::channel::<CommandPayload>(16);
        let dsid = "ds-roundtrip-test";

        let conn_id = registry.register(dsid, cmd_tx).await;

        // Spawn a mock Connect handler that echoes back a success response
        let handler = tokio::spawn(async move {
            if let Some((request, response_tx)) = cmd_rx.recv().await {
                let response = ConnectResponse {
                    id: request.id,
                    body: kyomi_core::connect_protocol::ConnectResponseBody::Result {
                        result: serde_json::json!(true),
                    },
                };
                let _ = response_tx.send(response);
            }
        });

        let request = ConnectRequest {
            id: "roundtrip-1".into(),
            op: kyomi_core::connect_protocol::ConnectOp::TestConnection,
            params: None,
            streaming: false,
        };

        let response = registry
            .send_command(dsid, request, Duration::from_secs(5))
            .await
            .expect("command should succeed");

        assert_eq!(response.id, "roundtrip-1");
        match response.body {
            kyomi_core::connect_protocol::ConnectResponseBody::Result { result } => {
                assert_eq!(result, serde_json::json!(true));
            }
            other => panic!("expected Result, got {other:?}"),
        }

        handler.await.unwrap();

        // Cleanup
        registry.unregister(dsid, conn_id).await;
    }

    #[tokio::test]
    async fn is_connected_local_check() {
        let registry = test_registry().await;
        let dsid = "ds-connected-test";

        assert!(!registry.is_connected(dsid).await);

        let (cmd_tx, _cmd_rx) = mpsc::channel::<CommandPayload>(16);
        let conn_id = registry.register(dsid, cmd_tx).await;

        assert!(registry.is_connected(dsid).await);

        registry.unregister(dsid, conn_id).await;

        assert!(!registry.is_connected(dsid).await);
    }

    #[tokio::test]
    async fn stale_unregister_does_not_evict_newer_connection() {
        let registry = test_registry().await;
        let dsid = "ds-race-test";

        // First connection registers
        let (cmd_tx1, _cmd_rx1) = mpsc::channel::<CommandPayload>(16);
        let conn_id_1 = registry.register(dsid, cmd_tx1).await;

        // Second connection registers (supersedes first)
        let (cmd_tx2, _cmd_rx2) = mpsc::channel::<CommandPayload>(16);
        let conn_id_2 = registry.register(dsid, cmd_tx2).await;
        assert_ne!(conn_id_1, conn_id_2);

        // First connection tries to unregister (stale) — should be a no-op
        registry.unregister(dsid, conn_id_1).await;

        // Second connection should still be registered
        assert!(registry.is_connected(dsid).await);

        // Second connection unregisters — should work
        registry.unregister(dsid, conn_id_2).await;
        assert!(!registry.is_connected(dsid).await);
    }

    /// Test cross-replica routing: simulate a "remote" connection by registering
    /// a connection, starting the command subscriber, then sending a command
    /// from a second registry that only sees the Redis presence key.
    #[tokio::test]
    async fn cross_replica_command_routing() {
        let config = kyomi_core::Config::test_config();
        let redis_url = config.redis_url.unwrap_or_else(|| "redis://localhost:6380".into());
        let redis = kyomi_core::redis::create_pool(&redis_url)
            .await
            .expect("test Redis");

        let dsid = "ds-cross-replica-test";

        // "Pod A" — holds the WebSocket connection
        let pod_a = ConnectRegistry::new(redis.clone(), redis_url.clone());
        let (cmd_tx, mut cmd_rx) = mpsc::channel::<CommandPayload>(16);
        let conn_id = pod_a.register(dsid, cmd_tx).await;
        pod_a.start_command_subscriber(dsid, conn_id);

        // Give the subscriber a moment to connect to Redis
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Mock handler on Pod A: echoes back a success response
        let handler = tokio::spawn(async move {
            if let Some((request, response_tx)) = cmd_rx.recv().await {
                let response = ConnectResponse {
                    id: request.id,
                    body: kyomi_core::connect_protocol::ConnectResponseBody::Result {
                        result: serde_json::json!({"rows": 42}),
                    },
                };
                let _ = response_tx.send(response);
            }
        });

        // "Pod B" — does NOT hold the WebSocket, but can see Redis presence key
        let pod_b = ConnectRegistry::new(redis.clone(), redis_url.clone());

        // Pod B should see the connection via Redis
        assert!(pod_b.is_connected(dsid).await);

        // Pod B sends a command — should be routed via Redis to Pod A
        let request = ConnectRequest {
            id: "cross-1".into(),
            op: kyomi_core::connect_protocol::ConnectOp::TestConnection,
            params: None,
            streaming: false,
        };

        let response = pod_b
            .send_command(dsid, request, Duration::from_secs(5))
            .await
            .expect("cross-replica command should succeed");

        assert_eq!(response.id, "cross-1");
        match response.body {
            kyomi_core::connect_protocol::ConnectResponseBody::Result { result } => {
                assert_eq!(result, serde_json::json!({"rows": 42}));
            }
            other => panic!("expected Result, got {other:?}"),
        }

        handler.await.unwrap();

        // Cleanup
        pod_a.unregister(dsid, conn_id).await;
    }

    /// Test that Redis presence keys are properly set and cleaned up.
    #[tokio::test]
    async fn redis_presence_keys_lifecycle() {
        let config = kyomi_core::Config::test_config();
        let redis_url = config.redis_url.unwrap_or_else(|| "redis://localhost:6380".into());
        let redis = kyomi_core::redis::create_pool(&redis_url)
            .await
            .expect("test Redis");

        let registry = ConnectRegistry::new(redis.clone(), redis_url);
        let dsid = "ds-presence-lifecycle";

        // Before registration, key should not exist
        let key = presence_key(dsid);
        let mut conn = redis.clone();
        let exists: i64 = redis::cmd("EXISTS")
            .arg(&key)
            .query_async(&mut conn)
            .await
            .unwrap();
        assert_eq!(exists, 0);

        // Register — key should be set
        let (cmd_tx, _cmd_rx) = mpsc::channel::<CommandPayload>(16);
        let conn_id = registry.register(dsid, cmd_tx).await;

        let exists: i64 = redis::cmd("EXISTS")
            .arg(&key)
            .query_async(&mut conn)
            .await
            .unwrap();
        assert_eq!(exists, 1);

        // Check TTL is set (should be approximately 60)
        let ttl: i64 = redis::cmd("TTL")
            .arg(&key)
            .query_async(&mut conn)
            .await
            .unwrap();
        assert!(ttl > 0 && ttl <= 60, "TTL should be 1-60, got: {ttl}");

        // Unregister — key should be deleted
        registry.unregister(dsid, conn_id).await;

        let exists: i64 = redis::cmd("EXISTS")
            .arg(&key)
            .query_async(&mut conn)
            .await
            .unwrap();
        assert_eq!(exists, 0);
    }

    /// Test timeout behavior when the Connect handler doesn't respond.
    #[tokio::test]
    async fn send_command_timeout_on_no_response() {
        let registry = test_registry().await;

        // Register a connection but the handler never responds
        let (cmd_tx, mut cmd_rx) = mpsc::channel::<CommandPayload>(16);
        let dsid = "ds-timeout-test";
        let conn_id = registry.register(dsid, cmd_tx).await;

        // Spawn a handler that receives but never responds
        let handler = tokio::spawn(async move {
            // Receive the command but drop the response sender
            if let Some((_request, _response_tx)) = cmd_rx.recv().await {
                // Intentionally drop response_tx without sending
            }
        });

        let request = ConnectRequest {
            id: "timeout-1".into(),
            op: kyomi_core::connect_protocol::ConnectOp::TestConnection,
            params: None,
            streaming: false,
        };

        let result = registry
            .send_command(dsid, request, Duration::from_secs(1))
            .await;

        assert!(result.is_err());
        let err_msg = format!("{}", result.unwrap_err());
        assert!(
            err_msg.contains("dropped the response channel"),
            "expected 'dropped' error, got: {err_msg}"
        );

        handler.await.unwrap();
        registry.unregister(dsid, conn_id).await;
    }

    /// Test cross-replica timeout when the remote pod's handler is slow.
    #[tokio::test]
    async fn cross_replica_timeout_when_remote_slow() {
        let config = kyomi_core::Config::test_config();
        let redis_url = config.redis_url.unwrap_or_else(|| "redis://localhost:6380".into());
        let redis = kyomi_core::redis::create_pool(&redis_url)
            .await
            .expect("test Redis");

        let dsid = "ds-cross-timeout-test";

        // "Pod A" — holds the connection but the handler is very slow
        let pod_a = ConnectRegistry::new(redis.clone(), redis_url.clone());
        let (cmd_tx, mut cmd_rx) = mpsc::channel::<CommandPayload>(16);
        let conn_id = pod_a.register(dsid, cmd_tx).await;
        pod_a.start_command_subscriber(dsid, conn_id);

        tokio::time::sleep(Duration::from_millis(100)).await;

        // Slow handler — waits longer than the caller's timeout
        let handler = tokio::spawn(async move {
            if let Some((request, response_tx)) = cmd_rx.recv().await {
                tokio::time::sleep(Duration::from_secs(5)).await;
                let response = ConnectResponse {
                    id: request.id,
                    body: kyomi_core::connect_protocol::ConnectResponseBody::Result {
                        result: serde_json::json!(true),
                    },
                };
                let _ = response_tx.send(response);
            }
        });

        // "Pod B" — sends command with a short timeout
        let pod_b = ConnectRegistry::new(redis.clone(), redis_url.clone());

        let request = ConnectRequest {
            id: "cross-timeout-1".into(),
            op: kyomi_core::connect_protocol::ConnectOp::TestConnection,
            params: None,
            streaming: false,
        };

        let result = pod_b
            .send_command(dsid, request, Duration::from_secs(1))
            .await;

        assert!(result.is_err());
        let err_msg = format!("{}", result.unwrap_err());
        assert!(
            err_msg.contains("timed out"),
            "expected 'timed out' in error, got: {err_msg}"
        );

        // Cleanup
        handler.abort();
        pod_a.unregister(dsid, conn_id).await;
    }

    /// Test that the subscriber is cleaned up when the connection is unregistered.
    #[tokio::test]
    async fn subscriber_cleaned_up_on_unregister() {
        let registry = test_registry().await;
        let dsid = "ds-subscriber-cleanup";

        let (cmd_tx, _cmd_rx) = mpsc::channel::<CommandPayload>(16);
        let conn_id = registry.register(dsid, cmd_tx).await;
        registry.start_command_subscriber(dsid, conn_id);

        // Subscriber should be tracked
        assert!(registry.subscribers.contains_key(dsid));

        // Unregister removes both the connection and the subscriber
        registry.unregister(dsid, conn_id).await;

        assert!(!registry.subscribers.contains_key(dsid));
        assert!(!registry.connections.contains_key(dsid));
    }
}

#[cfg(all(test, target_os = "linux"))]
#[path = "registry_recovery_tests.rs"]
mod recovery_tests;
