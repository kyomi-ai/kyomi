// SPDX-License-Identifier: AGPL-3.0-or-later

//! Analytics event quota management.
//!
//! Defines per-tier event limits and provides Redis sync utilities.

use std::collections::HashMap;

/// Analytics quota configuration for a subscription tier.
#[derive(Debug, Clone)]
pub struct AnalyticsTierConfig {
    /// Maximum events per month.
    pub monthly_event_limit: u64,
    /// Grace percentage above the limit (e.g. 20 = allow up to 120%).
    pub grace_percent: u8,
    /// Data retention in days.
    pub retention_days: u32,
}

impl AnalyticsTierConfig {
    /// Compute the grace limit (quota_limit * (100 + grace_percent) / 100).
    pub fn grace_limit(&self) -> u64 {
        self.monthly_event_limit * (100 + self.grace_percent as u64) / 100
    }
}

/// Cloud analytics configuration. All tiers map to the same config.
///
/// - 100K events/month included
/// - 20% grace (120K hard limit)
/// - 180 days (6 months) retention
/// - Additional events via purchased bundles (tracked on workspace)
pub fn default_tier_configs() -> HashMap<String, AnalyticsTierConfig> {
    let cloud = AnalyticsTierConfig {
        monthly_event_limit: 100_000,
        grace_percent: 20,
        retention_days: 180,
    };
    let mut m = HashMap::new();
    // All tier names map to the same Cloud config for backward compatibility.
    // The DB still stores old tier strings (free, starter, pro, etc.) for
    // existing workspaces — they all get Cloud-level quotas now.
    for tier in &["cloud", "free", "basic", "starter", "pro", "team", "enterprise"] {
        m.insert((*tier).to_string(), cloud.clone());
    }
    m
}

/// Sync a workspace's analytics quota to Redis.
///
/// Called when a site is created, a tier changes, or during periodic reconciliation.
pub async fn sync_quota_to_redis(
    redis: &mut redis::aio::ConnectionManager,
    workspace_id: &str,
    config: &AnalyticsTierConfig,
) -> Result<(), redis::RedisError> {
    let key = format!("analytics:quota:{workspace_id}");
    redis::pipe()
        .hset(&key, "quota_limit", config.monthly_event_limit)
        .hset(&key, "grace_limit", config.grace_limit())
        .query_async(redis)
        .await
}

/// Get the current month's event count for a workspace from Redis.
pub async fn get_usage_count(
    redis: &mut redis::aio::ConnectionManager,
    workspace_id: &str,
) -> Result<u64, redis::RedisError> {
    let month_key = format!(
        "analytics:usage:{}:{}",
        workspace_id,
        chrono::Utc::now().format("%Y-%m")
    );
    let count: Option<u64> = redis::cmd("GET").arg(&month_key).query_async(redis).await?;
    Ok(count.unwrap_or(0))
}

/// Get per-site event counts for the current month from ClickHouse.
///
/// Accepts `(site_id, clickhouse_database)` pairs and queries each per-site database
/// concurrently to avoid N sequential round-trips.
///
/// Uses `toStartOfMonth(now())` server-side to avoid timezone mismatch — computing
/// the month start in Rust and sending it as a bare string would be interpreted in
/// the ClickHouse server's timezone, potentially counting events from the wrong range.
pub async fn get_per_site_counts_from_clickhouse(
    ch_host: &str,
    ch_port: u16,
    ch_password: &str,
    sites: &[(String, String)],  // Vec of (site_id, clickhouse_database)
    ch_secure: bool,
) -> Result<HashMap<String, u64>, kyomi_core::Error> {
    if sites.is_empty() {
        return Ok(HashMap::new());
    }

    let scheme = if ch_secure { "https" } else { "http" };
    let url = format!("{scheme}://{}:{}/", ch_host, ch_port);
    let ch_password = ch_password.to_string();
    // reqwest::Client is cheap to clone — all clones share the same connection pool.
    let client = crate::http_client()?;

    // Spawn concurrent requests — avoids N sequential round-trips for workspaces with many sites.
    let handles: Vec<tokio::task::JoinHandle<(String, Option<u64>)>> = sites
        .iter()
        .map(|(site_id, database)| {
            let client = client.clone();
            let url = url.clone();
            let ch_password = ch_password.clone();
            let site_id = site_id.clone();
            let database = database.clone();
            // toStartOfMonth(now()) is evaluated server-side and is timezone-aware,
            // avoiding the bare-datetime timezone pitfall of the ClickHouse HTTP API.
            let sql = format!(
                "SELECT count() as cnt FROM {database}.events \
                 WHERE timestamp >= toStartOfMonth(now()) FORMAT JSONEachRow"
            );
            tokio::task::spawn(async move {
                let resp = match client.post(&url)
                    .header("X-ClickHouse-User", "default")
                    .header("X-ClickHouse-Key", &ch_password)
                    .body(sql)
                    .send()
                    .await {
                    Ok(r) => r,
                    Err(e) => {
                        tracing::warn!(site_id = %site_id, error = %e, "ClickHouse count query failed");
                        return (site_id, None);
                    }
                };

                if !resp.status().is_success() {
                    // Skip databases that don't exist yet (site not yet migrated or just created)
                    tracing::warn!(site_id = %site_id, database = %database, "ClickHouse count query returned error — skipping site");
                    return (site_id, None);
                }

                let body = match resp.text().await {
                    Ok(b) => b,
                    Err(e) => {
                        tracing::warn!(site_id = %site_id, error = %e, "ClickHouse response read failed");
                        return (site_id, None);
                    }
                };

                for line in body.lines() {
                    if line.trim().is_empty() {
                        continue;
                    }
                    if let Ok(row) = serde_json::from_str::<serde_json::Value>(line)
                        && let Some(cnt) = row.get("cnt").and_then(|v| v.as_u64())
                    {
                        return (site_id, Some(cnt));
                    }
                }
                (site_id, None)
            })
        })
        .collect();

    let mut counts = HashMap::new();
    for handle in handles {
        match handle.await {
            Ok((site_id, Some(cnt))) => {
                counts.insert(site_id, cnt);
            }
            Ok((_, None)) => {} // Warning already logged inside the task
            Err(e) => {
                tracing::warn!(error = %e, "Task join error in get_per_site_counts_from_clickhouse");
            }
        }
    }

    Ok(counts)
}

/// Reconcile Redis event counters with ClickHouse actuals.
///
/// Queries ClickHouse for the current month's event count per workspace.
/// If any per-site query fails, the workspace is skipped entirely (Redis
/// left unchanged) to avoid clobbering counters with an incomplete total.
/// Otherwise, overwrites Redis if drift >1%, or seeds Redis from ClickHouse
/// when Redis has no count (e.g. after a Redis restart).
pub async fn reconcile_counters(
    redis: &mut redis::aio::ConnectionManager,
    db: &sqlx::PgPool,
    ch_host: &str,
    ch_port: u16,
    ch_password: &str,
    ch_secure: bool,
) -> Result<(), kyomi_core::Error> {
    // Get all workspaces with analytics sites (include subscription tier for quota sync)
    // Only include sites that have been migrated to per-site databases
    let rows: Vec<(String, String, String, String)> = sqlx::query_as(
        "SELECT DISTINCT a.workspace_id, a.site_id, a.clickhouse_database, w.subscription_tier \
         FROM analytics_sites a \
         JOIN workspaces w ON w.workspace_id = a.workspace_id \
         WHERE a.clickhouse_database IS NOT NULL \
         ORDER BY a.workspace_id"
    )
    .fetch_all(db)
    .await
    .map_err(|e| kyomi_core::Error::Internal(format!("DB query failed: {e}")))?;

    // Group (site_id, database) pairs by workspace_id, keep subscription tier
    let mut map: HashMap<String, (String, Vec<(String, String)>)> = HashMap::new();
    for (ws_id, site_id, database, tier) in rows {
        map.entry(ws_id).or_insert_with(|| (tier, Vec::new())).1.push((site_id, database));
    }
    type WorkspaceWithSites = (String, String, Vec<(String, String)>);
    let workspaces: Vec<WorkspaceWithSites> = map.into_iter()
        .map(|(ws_id, (tier, sites))| (ws_id, tier, sites))
        .collect();

    if workspaces.is_empty() {
        return Ok(());
    }

    // Get all (site_id, database) pairs for ClickHouse queries
    let all_sites: Vec<(String, String)> = workspaces.iter().flat_map(|(_, _, sites)| sites.clone()).collect();
    let ch_counts = get_per_site_counts_from_clickhouse(ch_host, ch_port, ch_password, &all_sites, ch_secure).await?;

    let month = chrono::Utc::now().format("%Y-%m").to_string();

    let tier_configs = default_tier_configs();

    for (workspace_id, tier, site_pairs) in &workspaces {
        // Ensure quota hash exists in Redis (idempotent — fixes missing quota from
        // sites created before spawn_analytics_post_create was added, or after Redis restart)
        if let Some(config) = tier_configs.get(tier.as_str())
            && let Err(e) = sync_quota_to_redis(redis, workspace_id, config).await
        {
            tracing::warn!(error = %e, workspace_id = %workspace_id, "Failed to sync quota during reconciliation");
        }

        // If any site's ClickHouse query failed it will be absent from ch_counts.
        // Summing missing sites as 0 would produce a falsely low total and overwrite
        // Redis with an incorrect value — skip reconciliation for this workspace instead.
        if site_pairs.iter().any(|(sid, _)| !ch_counts.contains_key(sid.as_str())) {
            tracing::warn!(
                workspace_id = %workspace_id,
                "Skipping reconciliation — one or more ClickHouse queries failed; Redis counter unchanged"
            );
            continue;
        }
        // The contains_key guard above guarantees every sid is present.
        let ch_total: u64 = site_pairs.iter().map(|(sid, _)| ch_counts.get(sid).copied().expect("guard ensures all sites present")).sum();

        let redis_key = format!("analytics:usage:{}:{}", workspace_id, month);
        let redis_count: u64 = redis::cmd("GET")
            .arg(&redis_key)
            .query_async(redis)
            .await
            .unwrap_or(0);

        // Only overwrite if drift > 1%
        if redis_count > 0 {
            let drift = (ch_total as f64 - redis_count as f64).abs() / redis_count as f64;
            if drift > 0.01 {
                tracing::info!(
                    workspace_id = %workspace_id,
                    redis = redis_count,
                    clickhouse = ch_total,
                    drift_pct = format!("{:.1}", drift * 100.0),
                    "Reconciling analytics counter"
                );
                // Preserve TTL: use KEEPTTL so we don't lose the key's existing expiry
                let _: () = redis::cmd("SET")
                    .arg(&redis_key)
                    .arg(ch_total)
                    .arg("KEEPTTL")
                    .query_async(redis)
                    .await
                    .unwrap_or(());
            }
        } else if ch_total > 0 {
            // Redis has no count but ClickHouse does (e.g. after Redis restart)
            let _: () = redis::cmd("SET")
                .arg(&redis_key)
                .arg(ch_total)
                .arg("EX")
                .arg(45 * 86400)
                .query_async(redis)
                .await
                .unwrap_or(());
        }
    }

    Ok(())
}

/// Delete old analytics events based on each workspace's tier retention period.
///
/// Runs daily. Uses ClickHouse async mutations (non-blocking).
pub async fn cleanup_retention(
    db: &sqlx::PgPool,
    ch_host: &str,
    ch_port: u16,
    ch_password: &str,
    ch_secure: bool,
) -> Result<(), kyomi_core::Error> {
    let configs = default_tier_configs();

    // Get all sites that have been migrated to per-site databases
    let rows: Vec<(String, String, String, String)> = sqlx::query_as(
        "SELECT a.site_id, a.workspace_id, a.clickhouse_database, \
                COALESCE(w.subscription_tier, 'free') as tier \
         FROM analytics_sites a \
         JOIN workspaces w ON w.workspace_id = a.workspace_id \
         WHERE a.clickhouse_database IS NOT NULL"
    )
    .fetch_all(db)
    .await
    .map_err(|e| kyomi_core::Error::Internal(format!("DB query failed: {e}")))?;

    let client = crate::http_client()?;
    let scheme = if ch_secure { "https" } else { "http" };
    let url = format!("{scheme}://{}:{}/", ch_host, ch_port);

    for (site_id, _workspace_id, ch_database, tier) in &rows {
        let retention_days = configs
            .get(tier.as_str())
            .map(|c| c.retention_days)
            .unwrap_or(730); // Default to 2 years if tier unknown

        let sql = format!(
            "ALTER TABLE {}.events DELETE WHERE timestamp < now() - INTERVAL {} DAY",
            ch_database,
            retention_days
        );

        match client.post(&url)
            .header("X-ClickHouse-User", "default")
            .header("X-ClickHouse-Key", ch_password)
            .body(sql)
            .send()
            .await {
            Ok(resp) if resp.status().is_success() => {
                tracing::debug!(site_id = %site_id, database = %ch_database, retention_days, "Retention cleanup submitted");
            }
            Ok(resp) => {
                let body = resp.text().await.unwrap_or_default();
                tracing::warn!(site_id = %site_id, database = %ch_database, error = %body, "Retention cleanup failed");
            }
            Err(e) => {
                tracing::warn!(site_id = %site_id, database = %ch_database, error = %e, "Retention cleanup request failed");
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_grace_limit_calculation() {
        let config = AnalyticsTierConfig {
            monthly_event_limit: 500_000,
            grace_percent: 20,
            retention_days: 180,
        };
        assert_eq!(config.grace_limit(), 600_000);
    }

    #[test]
    fn test_grace_limit_zero_percent() {
        let config = AnalyticsTierConfig {
            monthly_event_limit: 10_000,
            grace_percent: 0,
            retention_days: 30,
        };
        assert_eq!(config.grace_limit(), 10_000);
    }

    #[test]
    fn test_default_tier_configs_has_all_tiers() {
        let configs = default_tier_configs();
        assert!(configs.contains_key("free"));
        assert!(configs.contains_key("basic"));
        assert!(configs.contains_key("starter"));
        assert!(configs.contains_key("pro"));
        assert!(configs.contains_key("team"));
        assert!(configs.contains_key("enterprise"));
    }

    #[test]
    fn test_all_tiers_have_same_config() {
        let configs = default_tier_configs();
        for (_, config) in &configs {
            assert_eq!(config.monthly_event_limit, 100_000);
            assert_eq!(config.grace_percent, 20);
            assert_eq!(config.retention_days, 180);
        }
    }
}
