// SPDX-License-Identifier: AGPL-3.0-or-later

//! Redis-backed KVStore implementation.
//!
//! Wraps `redis::aio::ConnectionManager` (aliased as [`crate::RedisPool`])
//! which automatically reconnects on failures and is cheaply cloneable.

use redis::AsyncCommands;

use crate::kv_store::KVStore;
use crate::RedisPool;

/// Redis-backed key-value store.
pub struct RedisKVStore {
    conn: RedisPool,
}

impl RedisKVStore {
    /// Connect to Redis at `redis_url` and return a ready store.
    pub async fn new(redis_url: &str) -> crate::Result<Self> {
        let conn = crate::redis::create_pool(redis_url).await?;
        Ok(Self { conn })
    }
}

// ---------------------------------------------------------------------------
// KVStore implementation
// ---------------------------------------------------------------------------

#[async_trait::async_trait]
impl KVStore for RedisKVStore {
    async fn set(&self, key: &str, value: &str, ttl_secs: Option<u64>) -> crate::Result<()> {
        let mut conn = self.conn.clone();
        match ttl_secs {
            Some(ttl) => conn
                .set_ex(key, value, ttl)
                .await
                .map_err(|e| crate::Error::Internal(format!("redis error: {e}")))?,
            None => conn
                .set(key, value)
                .await
                .map_err(|e| crate::Error::Internal(format!("redis error: {e}")))?,
        }
        Ok(())
    }

    async fn get(&self, key: &str) -> crate::Result<Option<String>> {
        let mut conn = self.conn.clone();
        conn.get(key)
            .await
            .map_err(|e| crate::Error::Internal(format!("redis error: {e}")))
    }

    async fn del(&self, key: &str) -> crate::Result<()> {
        let mut conn = self.conn.clone();
        conn.del::<_, ()>(key)
            .await
            .map_err(|e| crate::Error::Internal(format!("redis error: {e}")))?;
        Ok(())
    }

    async fn getdel(&self, key: &str) -> crate::Result<Option<String>> {
        let mut conn = self.conn.clone();
        redis::cmd("GETDEL")
            .arg(key)
            .query_async(&mut conn)
            .await
            .map_err(|e| crate::Error::Internal(format!("redis error: {e}")))
    }

    async fn incr(&self, key: &str) -> crate::Result<i64> {
        let mut conn = self.conn.clone();
        conn.incr(key, 1_i64)
            .await
            .map_err(|e| crate::Error::Internal(format!("redis error: {e}")))
    }

    async fn expire(&self, key: &str, ttl_secs: u64) -> crate::Result<()> {
        let mut conn = self.conn.clone();
        conn.expire::<_, ()>(key, ttl_secs as i64)
            .await
            .map_err(|e| crate::Error::Internal(format!("redis error: {e}")))?;
        Ok(())
    }

    async fn sadd(&self, key: &str, member: &str) -> crate::Result<()> {
        let mut conn = self.conn.clone();
        redis::cmd("SADD")
            .arg(key)
            .arg(member)
            .query_async::<i64>(&mut conn)
            .await
            .map_err(|e| crate::Error::Internal(format!("redis error: {e}")))?;
        Ok(())
    }

    async fn srem(&self, key: &str, member: &str) -> crate::Result<()> {
        let mut conn = self.conn.clone();
        redis::cmd("SREM")
            .arg(key)
            .arg(member)
            .query_async::<i64>(&mut conn)
            .await
            .map_err(|e| crate::Error::Internal(format!("redis error: {e}")))?;
        Ok(())
    }

    async fn smembers(&self, key: &str) -> crate::Result<Vec<String>> {
        let mut conn = self.conn.clone();
        redis::cmd("SMEMBERS")
            .arg(key)
            .query_async::<Vec<String>>(&mut conn)
            .await
            .map_err(|e| crate::Error::Internal(format!("redis error: {e}")))
    }

    async fn sdel(&self, key: &str) -> crate::Result<()> {
        self.del(key).await
    }
}
