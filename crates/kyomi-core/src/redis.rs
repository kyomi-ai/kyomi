// SPDX-License-Identifier: AGPL-3.0-or-later

//! Redis connection manager via the `redis` crate.

use std::time::Duration;

use redis::aio::ConnectionManager;

/// Type alias for the Redis connection manager.
///
/// `ConnectionManager` automatically reconnects on failure — no manual retry
/// logic needed.  It is cheaply cloneable (internally Arc'd).
pub type RedisPool = ConnectionManager;

/// Upper bound on the *initial* connection attempt made by [`create_pool`].
///
/// `ConnectionManager::new` builds its retry strategy from
/// `ConnectionManagerConfig`'s defaults — 6 retries with an exponential
/// backoff (`backon`'s default `min_delay: 1s`, `max_delay: 60s`, and a
/// `factor: 100` that saturates the delay at `max_delay` almost immediately)
/// — so against an unreachable host it does not return `Err` until roughly
/// 301s of pure sleeping have elapsed (measured end-to-end at 473s).
///
/// That retry strategy is not exclusive to the initial connect: the manager
/// stores it and reuses it for steady-state reconnection of an
/// already-established connection, so it must not be shortened here — a
/// shorter backoff there would hammer a struggling Redis on every drop.
/// `tokio::time::timeout` around the one-shot `ConnectionManager::new` call
/// bounds only how long *this function* waits for construction to resolve;
/// it never touches the strategy stored inside the manager for later
/// reconnects.
///
/// 5s leaves generous headroom under the 10s acceptance bound for a real but
/// slow-to-answer Redis (TLS handshake, a cold container still booting)
/// while still failing fast — a live TCP refusal is instant, so anything
/// past a couple of seconds here is not "slow," it is "not going to answer."
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// Create a Redis connection manager from the given URL.
pub async fn create_pool(redis_url: &str) -> crate::Result<RedisPool> {
    let client = redis::Client::open(redis_url)
        .map_err(|e| crate::Error::Internal(format!("invalid redis URL: {e}")))?;

    // `ConnectionAddr`'s `Display` is `host:port` (or a unix socket path) and
    // never includes credentials — unlike `redis_url` itself, which may
    // carry `redis://:password@host:port`. Use it in error messages instead
    // of the raw URL so a misconfigured/unreachable Redis never leaks a
    // password into logs or a propagated error.
    let addr = client.get_connection_info().addr.clone();

    let manager = tokio::time::timeout(CONNECT_TIMEOUT, ConnectionManager::new(client))
        .await
        .map_err(|_| {
            crate::Error::Internal(format!(
                "redis connection to {addr} timed out after {}s",
                CONNECT_TIMEOUT.as_secs()
            ))
        })?
        .map_err(|e| crate::Error::Internal(format!("redis connection to {addr} failed: {e}")))?;

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

    /// KYO-252: `create_pool()` against an unreachable Redis used to hang for
    /// ~473s — `ConnectionManager::new`'s default retry strategy sleeps
    /// through ~301s of exponential backoff before giving up, on top of
    /// per-attempt time. This asserts the fix (`CONNECT_TIMEOUT` wrapping the
    /// one-shot construction) actually bounds wall-clock time, not just that
    /// the call eventually errors — that's the whole bug.
    #[tokio::test]
    async fn test_create_pool_fails_fast_against_unreachable_redis() {
        // Bind port 0 to get an OS-assigned free port, then drop the
        // listener so nothing is listening there. This is a reliable way to
        // get a definitely-dead port without hardcoding one that might
        // collide with something else on the box — and without touching
        // 6380, which is the live dev Redis instance.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind to port 0");
        let port = listener.local_addr().expect("read local_addr").port();
        drop(listener);

        let url = format!("redis://127.0.0.1:{port}/0");
        let start = std::time::Instant::now();
        let result = create_pool(&url).await;
        let elapsed = start.elapsed();

        assert!(
            result.is_err(),
            "expected create_pool to fail against a port nothing listens on"
        );
        assert!(
            elapsed < Duration::from_secs(10),
            "create_pool took {elapsed:?} to fail against a dead port — \
             should be bounded by CONNECT_TIMEOUT ({CONNECT_TIMEOUT:?}), \
             well under the 10s acceptance bound (was ~473s before KYO-252)"
        );
    }
}
