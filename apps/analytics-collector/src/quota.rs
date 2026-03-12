use chrono::Utc;
use tracing::warn;

/// Result of a quota check.
#[derive(Debug, PartialEq)]
pub enum QuotaResult {
    /// Under quota — proceed with event insertion.
    Allowed,
    /// Over quota but within grace — accept event, flag for notification.
    GracePeriod,
    /// Over grace limit — reject event with 429.
    Blocked,
}

/// Check and increment the event quota for a workspace.
///
/// Returns `Allowed` or `GracePeriod` if the event should be accepted,
/// `Blocked` if it should be rejected with 429.
///
/// On any Redis error, returns `Allowed` (fail-open).
pub async fn check_quota(
    redis: &mut redis::aio::ConnectionManager,
    workspace_id: &str,
) -> QuotaResult {
    let result = check_quota_inner(redis, workspace_id).await;
    match result {
        Ok(decision) => decision,
        Err(e) => {
            warn!(error = %e, workspace_id = %workspace_id, "Redis quota check failed — fail-open");
            QuotaResult::Allowed
        }
    }
}

/// Lua script that atomically checks quota before incrementing.
///
/// Returns: new_count (>0) if incremented, -1 if blocked (over grace limit).
/// This prevents blocked events from inflating the counter.
///
/// KEYS[1] = quota hash key, KEYS[2] = usage counter key
/// ARGV[1] = TTL in seconds (45 days)
const QUOTA_CHECK_LUA: &str = r#"
local quota = redis.call('HGET', KEYS[1], 'quota_limit')
if not quota then
    return 0
end
local grace = redis.call('HGET', KEYS[1], 'grace_limit')
if not grace then grace = quota end
quota = tonumber(quota)
grace = tonumber(grace)
local count = tonumber(redis.call('GET', KEYS[2]) or '0')
if count >= grace then
    return -1
end
local new_count = redis.call('INCR', KEYS[2])
if new_count == 1 then
    redis.call('EXPIRE', KEYS[2], ARGV[1])
end
return new_count
"#;

async fn check_quota_inner(
    redis: &mut redis::aio::ConnectionManager,
    workspace_id: &str,
) -> Result<QuotaResult, redis::RedisError> {
    let quota_key = format!("analytics:quota:{workspace_id}");
    let month_key = format!(
        "analytics:usage:{}:{}",
        workspace_id,
        Utc::now().format("%Y-%m")
    );

    // Atomic check-then-increment via Lua script
    let result: i64 = redis::cmd("EVAL")
        .arg(QUOTA_CHECK_LUA)
        .arg(2) // number of keys
        .arg(&quota_key)
        .arg(&month_key)
        .arg(45 * 86400) // TTL
        .query_async(redis)
        .await?;

    // 0 = no quota configured (allow freely)
    if result == 0 {
        return Ok(QuotaResult::Allowed);
    }

    // -1 = blocked (over grace limit, counter NOT incremented)
    if result < 0 {
        set_notification_flag(redis, workspace_id, "grace").await;
        return Ok(QuotaResult::Blocked);
    }

    let new_count = result as u64;

    // Read limits for threshold checks
    let (quota_limit, grace_limit): (Option<u64>, Option<u64>) = redis::pipe()
        .hget(&quota_key, "quota_limit")
        .hget(&quota_key, "grace_limit")
        .query_async(redis)
        .await?;

    let quota_limit = quota_limit.unwrap_or(0);
    let grace_limit = grace_limit.unwrap_or(quota_limit);

    // Check thresholds and set notification flags
    if new_count > grace_limit {
        // Edge case: crossed grace between Lua check and here (very unlikely)
        set_notification_flag(redis, workspace_id, "grace").await;
        return Ok(QuotaResult::Blocked);
    }

    if new_count > quota_limit {
        set_notification_flag(redis, workspace_id, "100").await;
        return Ok(QuotaResult::GracePeriod);
    }

    // 80% threshold — fire only on the exact crossing event
    if new_count > quota_limit * 80 / 100 && (new_count - 1) <= quota_limit * 80 / 100 {
        set_notification_flag(redis, workspace_id, "80").await;
    }

    Ok(QuotaResult::Allowed)
}

/// Set a notification flag in Redis (deduplication key).
/// Best-effort — failures are logged but don't affect the quota decision.
async fn set_notification_flag(
    redis: &mut redis::aio::ConnectionManager,
    workspace_id: &str,
    threshold: &str,
) {
    let key = format!(
        "analytics:notify:{}:{}:{}",
        workspace_id,
        Utc::now().format("%Y-%m"),
        threshold
    );
    // SETNX — only set if not already set (dedup across replicas)
    let result: Result<bool, _> = redis::cmd("SET")
        .arg(&key)
        .arg("1")
        .arg("NX")
        .arg("EX")
        .arg(45 * 86400)
        .query_async(redis)
        .await;
    if let Err(e) = result {
        warn!(error = %e, "Failed to set notification flag");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quota_result_equality() {
        assert_eq!(QuotaResult::Allowed, QuotaResult::Allowed);
        assert_eq!(QuotaResult::GracePeriod, QuotaResult::GracePeriod);
        assert_eq!(QuotaResult::Blocked, QuotaResult::Blocked);
        assert_ne!(QuotaResult::Allowed, QuotaResult::Blocked);
    }
}
