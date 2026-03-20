// SPDX-License-Identifier: AGPL-3.0-or-later

//! KVStore trait — unified key-value store abstraction.
//!
//! Implementations:
//! - [`crate::kv_store_redis::RedisKVStore`] — backed by Redis (production)
//! - [`crate::kv_store_memory::InMemoryKVStore`] — in-process HashMap (single-instance / no-Redis)

use std::sync::Arc;

/// Async key-value store with TTL support.
///
/// All implementations must be `Send + Sync` so the store can be held in
/// Axum's shared state (which requires `Clone + Send + Sync + 'static`).
#[async_trait::async_trait]
pub trait KVStore: Send + Sync {
    /// SET key to value with an optional TTL in seconds.
    async fn set(&self, key: &str, value: &str, ttl_secs: Option<u64>) -> crate::Result<()>;

    /// GET key — returns `None` if the key is missing or expired.
    async fn get(&self, key: &str) -> crate::Result<Option<String>>;

    /// DEL key.
    async fn del(&self, key: &str) -> crate::Result<()>;

    /// GETDEL — atomically get and delete.
    ///
    /// Returns `None` if the key is missing or expired.
    async fn getdel(&self, key: &str) -> crate::Result<Option<String>>;

    /// INCR — atomically increment the integer value stored at key.
    ///
    /// Creates the key with value `1` if it is absent or expired.
    async fn incr(&self, key: &str) -> crate::Result<i64>;

    /// EXPIRE — set a TTL on an existing key.
    ///
    /// No-op if the key is missing or already expired.
    async fn expire(&self, key: &str, ttl_secs: u64) -> crate::Result<()>;

    /// SADD — add a member to a set. Creates the set if it doesn't exist.
    async fn sadd(&self, key: &str, member: &str) -> crate::Result<()>;

    /// SREM — remove a member from a set. No-op if member or key doesn't exist.
    async fn srem(&self, key: &str, member: &str) -> crate::Result<()>;

    /// SMEMBERS — return all members of a set. Empty vec if key doesn't exist.
    async fn smembers(&self, key: &str) -> crate::Result<Vec<String>>;

    /// SDEL — delete an entire set key.
    async fn sdel(&self, key: &str) -> crate::Result<()>;

    /// PING — health check.
    ///
    /// Default implementation performs a set / get / del round-trip so that
    /// all implementations get a working health check for free.
    async fn ping(&self) -> crate::Result<()> {
        self.set("__ping__", "1", Some(5)).await?;
        self.get("__ping__").await?;
        self.del("__ping__").await?;
        Ok(())
    }
}

/// Cheaply-cloneable, type-erased KV store handle.
///
/// Use this everywhere instead of a concrete type so that the Redis and
/// in-memory implementations are interchangeable at runtime.
pub type KVPool = Arc<dyn KVStore>;

/// Create the appropriate [`KVPool`] based on the optional Redis URL.
///
/// - `Some(url)` → [`crate::kv_store_redis::RedisKVStore`] (connects to Redis)
/// - `None` → [`crate::kv_store_memory::InMemoryKVStore`] with a 30-second
///   background expiry sweep
pub async fn create_kv_store(redis_url: Option<&str>) -> crate::Result<KVPool> {
    match redis_url {
        Some(url) => {
            tracing::info!("KVStore: using Redis backend");
            let store = crate::kv_store_redis::RedisKVStore::new(url).await?;
            Ok(Arc::new(store))
        }
        None => {
            tracing::info!("KVStore: using in-memory backend (no Redis URL configured)");
            let pool = crate::kv_store_memory::InMemoryKVStore::new_pool();
            Ok(pool)
        }
    }
}
