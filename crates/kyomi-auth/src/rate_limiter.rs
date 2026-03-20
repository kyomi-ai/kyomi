// SPDX-License-Identifier: AGPL-3.0-or-later

//! KV-backed fixed-window rate limiter.
//!
//! Replaces the previous Lua token bucket (Redis-only) with INCR + EXPIRE,
//! which works with both Redis and InMemoryKVStore.

/// Sentinel value returned in `remaining` for endpoints that are not rate-limited.
const UNLIMITED_REMAINING: u32 = u32::MAX;

/// Result of a rate limit check.
#[derive(Debug, Clone)]
pub struct RateLimitResult {
    pub allowed: bool,
    pub remaining: u32,
    pub retry_after_secs: u64,
}

/// Rate limiter configuration for a single bucket.
#[derive(Debug, Clone)]
pub struct BucketConfig {
    pub capacity: u32,
    pub window_secs: u64,
}

/// Check rate limits for a given IP and optional user_id on an endpoint.
///
/// Returns `Ok(RateLimitResult)` with `allowed: false` if rate limited.
pub async fn check_rate_limit(
    kv: &kyomi_core::KVPool,
    ip: &str,
    endpoint: &str,
    user_id: Option<&str>,
) -> kyomi_core::Result<RateLimitResult> {
    let constants = kyomi_core::constants::get();
    let rate_config = match endpoint {
        "login" => &constants.rate_limits.login,
        "register" => &constants.rate_limits.register,
        // "signup" is the same operation as "register" — same config applies.
        "signup" => &constants.rate_limits.register,
        "refresh" => &constants.rate_limits.refresh,
        "api_call" => &constants.rate_limits.api_call,
        "trial_chat" => &constants.rate_limits.trial_chat,
        // Passkey recovery is a credential-recovery flow — treat like login.
        "passkey_recovery" => &constants.rate_limits.login,
        _ => {
            return Ok(RateLimitResult {
                allowed: true,
                remaining: UNLIMITED_REMAINING,
                retry_after_secs: 0,
            })
        }
    };

    // IP-based rate limit
    let ip_key = constants.redis.key_prefixes.rate_limit_ip
        .replace("{ip}", ip)
        .replace("{endpoint}", endpoint);

    let ip_result = check_bucket(
        kv,
        &ip_key,
        &BucketConfig {
            capacity: rate_config.ip_capacity,
            window_secs: rate_config.window_seconds,
        },
    ).await?;

    if !ip_result.allowed {
        return Ok(ip_result);
    }

    // User-based rate limit (if user_id provided)
    if let Some(uid) = user_id {
        let user_key = constants.redis.key_prefixes.rate_limit_user
            .replace("{user_id}", uid)
            .replace("{endpoint}", endpoint);

        let user_result = check_bucket(
            kv,
            &user_key,
            &BucketConfig {
                capacity: rate_config.user_capacity,
                window_secs: rate_config.window_seconds,
            },
        ).await?;

        if !user_result.allowed {
            return Ok(user_result);
        }

        // Both buckets passed — return the more-constrained remaining count.
        return Ok(RateLimitResult {
            allowed: true,
            remaining: ip_result.remaining.min(user_result.remaining),
            retry_after_secs: 0,
        });
    }

    Ok(ip_result)
}

/// Fixed-window counter using INCR + EXPIRE.
///
/// On the first increment the TTL is set to establish the window boundary.
/// Subsequent increments within the same window do not reset the TTL.
async fn check_bucket(
    kv: &kyomi_core::KVPool,
    key: &str,
    config: &BucketConfig,
) -> kyomi_core::Result<RateLimitResult> {
    let count = kv.incr(key).await?;
    // Only set TTL on first increment to avoid resetting the window on every call.
    if count == 1 {
        kv.expire(key, config.window_secs).await?;
    }

    let allowed = count <= config.capacity as i64;
    let remaining = (config.capacity as i64 - count).max(0) as u32;
    let retry_after_secs = if allowed { 0 } else { config.window_secs };

    Ok(RateLimitResult {
        allowed,
        remaining,
        retry_after_secs,
    })
}
