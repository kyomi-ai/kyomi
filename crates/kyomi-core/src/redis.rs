// SPDX-License-Identifier: AGPL-3.0-or-later

//! Redis connection manager via the `redis` crate.

use redis::aio::ConnectionManager;

/// Type alias for the Redis connection manager.
///
/// `ConnectionManager` automatically reconnects on failure — no manual retry
/// logic needed.  It is cheaply cloneable (internally Arc'd).
pub type RedisPool = ConnectionManager;

/// Create a Redis connection manager from the given URL.
pub async fn create_pool(redis_url: &str) -> crate::Result<RedisPool> {
    let client = redis::Client::open(redis_url)
        .map_err(|e| crate::Error::Internal(format!("invalid redis URL: {e}")))?;

    let manager = ConnectionManager::new(client)
        .await
        .map_err(|e| crate::Error::Internal(format!("redis connection failed: {e}")))?;

    tracing::info!("Redis connection manager ready");
    Ok(manager)
}

/// Run a PING command — useful for health endpoints.
pub async fn ping(conn: &mut RedisPool) -> crate::Result<()> {
    redis::cmd("PING")
        .query_async::<String>(conn)
        .await
        .map_err(|e| crate::Error::Internal(format!("redis ping failed: {e}")))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    #[tokio::test]
    async fn test_redis_connects() {
        let cfg = Config::test_config();
        let url = cfg.redis_url.as_deref().unwrap_or("redis://localhost:6381");
        let pool = create_pool(url).await;
        if pool.is_err() {
            eprintln!("skipping redis test — no Redis at {url}");
            return;
        }
        let mut pool = pool.unwrap();
        ping(&mut pool).await.expect("ping should succeed");
    }
}
