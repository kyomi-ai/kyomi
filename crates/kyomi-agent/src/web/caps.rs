// SPDX-License-Identifier: AGPL-3.0-or-later

//! Usage caps for Kyomi-executed web calls (owner-confirmed defaults in
//! [`kyomi_core::web_tools::WebCaps`]): a per-turn limit, a per-workspace
//! monthly limit and a global monthly budget. Monthly counters live in the KV
//! store (Redis when configured), so they are shared across server instances
//! and survive restarts when Redis is configured (the in-memory fallback is per instance and resets on restart). A call is counted when it is *attempted*: the cap
//! bounds spend, and a failed call may still have been billed upstream.
//!
//! At a cap the tool returns a clear "unavailable" result and chat continues.

use std::fmt;

use chrono::{Datelike, Utc};
use kyomi_core::KVPool;
use kyomi_core::web_tools::{WebCaps, WebToolKind};

/// Why a call was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapRefusal {
    PerTurn,
    WorkspaceMonthly,
    GlobalMonthly,
}

impl fmt::Display for CapRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::PerTurn => "the per-conversation-turn limit for this tool has been reached",
            Self::WorkspaceMonthly => "this workspace's monthly web access limit has been reached",
            Self::GlobalMonthly => "the monthly web access budget has been reached",
        })
    }
}

const MONTH_TTL_SECS: u64 = 35 * 24 * 3600;

fn month_key(scope: &str, tool: WebToolKind) -> String {
    let now = Utc::now();
    format!("web_tools:{}:{}:{:04}-{:02}", tool.tool_name(), scope, now.year(), now.month())
}

/// Reserve one call against the monthly caps: refuse when either counter is at
/// its cap (nothing is counted for a refusal), otherwise count the call against
/// both. The check and the increment are separate KV operations, so the caps are
/// soft under concurrency: simultaneous calls can overshoot by the number of
/// concurrent runs. They bound spend, they are not a transaction.
pub async fn reserve_monthly(
    kv: &KVPool,
    caps: &WebCaps,
    workspace_id: &str,
    tool: WebToolKind,
) -> kyomi_core::Result<Result<(), CapRefusal>> {
    if let Err(refusal) = check_monthly(kv, caps, workspace_id, tool).await? {
        return Ok(Err(refusal));
    }
    add_usage(kv, workspace_id, tool, 1).await?;
    Ok(Ok(()))
}

/// Whether the monthly budget still has room, without spending any. Used for
/// provider-executed backends, which Kyomi cannot meter per call: the counters
/// are fed afterwards from the provider's reported usage ([`add_usage`]).
pub async fn check_monthly(
    kv: &KVPool,
    caps: &WebCaps,
    workspace_id: &str,
    tool: WebToolKind,
) -> kyomi_core::Result<Result<(), CapRefusal>> {
    let read = |key: String| async move {
        Ok::<i64, kyomi_core::Error>(kv.get(&key).await?.and_then(|v| v.parse().ok()).unwrap_or(0))
    };
    if read(month_key(&format!("ws:{workspace_id}"), tool)).await? >= i64::from(caps.workspace_monthly) {
        return Ok(Err(CapRefusal::WorkspaceMonthly));
    }
    if read(month_key("global", tool)).await? >= i64::from(caps.global_monthly) {
        return Ok(Err(CapRefusal::GlobalMonthly));
    }
    Ok(Ok(()))
}

/// Calls left this month for the workspace and for the whole instance (saturating at
/// zero), for observability: logged whenever a call is metered.
pub async fn remaining_monthly(
    kv: &KVPool,
    caps: &WebCaps,
    workspace_id: &str,
    tool: WebToolKind,
) -> kyomi_core::Result<(u32, u32)> {
    let read = |key: String| async move {
        Ok::<u32, kyomi_core::Error>(kv.get(&key).await?.and_then(|v| v.parse::<u32>().ok()).unwrap_or(0))
    };
    let ws = read(month_key(&format!("ws:{workspace_id}"), tool)).await?;
    let global = read(month_key("global", tool)).await?;
    Ok((caps.workspace_monthly.saturating_sub(ws), caps.global_monthly.saturating_sub(global)))
}

/// Add `calls` provider-reported billable calls to the monthly counters.
pub async fn add_usage(kv: &KVPool, workspace_id: &str, tool: WebToolKind, calls: u32) -> kyomi_core::Result<()> {
    for scope in [format!("ws:{workspace_id}"), "global".to_string()] {
        let key = month_key(&scope, tool);
        for _ in 0..calls {
            if kv.incr(&key).await? == 1 {
                kv.expire(&key, MONTH_TTL_SECS).await?;
            }
        }
    }
    Ok(())
}

/// Per-turn counter shared by every tool call of one tool in one agent turn.
#[derive(Debug, Default)]
pub struct TurnCounter(std::sync::atomic::AtomicU32);

impl TurnCounter {
    /// Count one call; refuse when it would exceed `limit`.
    pub fn reserve(&self, limit: u32) -> Result<(), CapRefusal> {
        let prev = self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if prev >= limit { Err(CapRefusal::PerTurn) } else { Ok(()) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn caps(ws: u32, global: u32) -> WebCaps {
        WebCaps { per_turn_search: 5, per_turn_fetch: 5, workspace_monthly: ws, global_monthly: global }
    }

    #[tokio::test]
    async fn workspace_cap_then_global_cap() {
        let kv = kyomi_core::kv_store_memory::InMemoryKVStore::new_pool();
        let c = caps(2, 3);
        let t = WebToolKind::Search;
        assert_eq!(reserve_monthly(&kv, &c, "a", t).await.unwrap(), Ok(()));
        assert_eq!(reserve_monthly(&kv, &c, "a", t).await.unwrap(), Ok(()));
        assert_eq!(reserve_monthly(&kv, &c, "a", t).await.unwrap(), Err(CapRefusal::WorkspaceMonthly));
        // Other workspace still has room; the refused call above spent nothing, so
        // the global budget (3) has one left.
        assert_eq!(reserve_monthly(&kv, &c, "b", t).await.unwrap(), Ok(()));
        assert_eq!(reserve_monthly(&kv, &c, "b", t).await.unwrap(), Err(CapRefusal::GlobalMonthly));
        // A global refusal does not inflate the workspace counter either.
        assert_eq!(check_monthly(&kv, &caps(3, 100), "b", t).await.unwrap(), Ok(()), "b used 1, not 2");
    }

    #[tokio::test]
    async fn search_and_fetch_have_separate_counters() {
        let kv = kyomi_core::kv_store_memory::InMemoryKVStore::new_pool();
        let c = caps(1, 10);
        assert_eq!(reserve_monthly(&kv, &c, "a", WebToolKind::Search).await.unwrap(), Ok(()));
        assert_eq!(reserve_monthly(&kv, &c, "a", WebToolKind::Fetch).await.unwrap(), Ok(()));
    }

    #[tokio::test]
    async fn usage_reported_by_the_provider_feeds_the_budget_check() {
        let kv = kyomi_core::kv_store_memory::InMemoryKVStore::new_pool();
        let c = caps(2, 10);
        let t = WebToolKind::Search;
        assert_eq!(check_monthly(&kv, &c, "a", t).await.unwrap(), Ok(()));
        add_usage(&kv, "a", t, 2).await.unwrap();
        assert_eq!(check_monthly(&kv, &c, "a", t).await.unwrap(), Err(CapRefusal::WorkspaceMonthly));
        assert_eq!(check_monthly(&kv, &c, "b", t).await.unwrap(), Ok(()), "other workspaces unaffected");
        add_usage(&kv, "b", t, 8).await.unwrap();
        assert_eq!(check_monthly(&kv, &c, "c", t).await.unwrap(), Err(CapRefusal::GlobalMonthly));
    }

    #[tokio::test]
    async fn remaining_quota_reflects_usage_and_saturates_at_zero() {
        let kv = kyomi_core::kv_store_memory::InMemoryKVStore::new_pool();
        let c = caps(5, 8);
        let t = WebToolKind::Fetch;
        assert_eq!(remaining_monthly(&kv, &c, "a", t).await.unwrap(), (5, 8));
        add_usage(&kv, "a", t, 3).await.unwrap();
        assert_eq!(remaining_monthly(&kv, &c, "a", t).await.unwrap(), (2, 5));
        add_usage(&kv, "b", t, 9).await.unwrap();
        assert_eq!(remaining_monthly(&kv, &c, "a", t).await.unwrap(), (2, 0), "global saturates at zero");
    }

    #[test]
    fn per_turn_counter_refuses_over_limit() {
        let c = TurnCounter::default();
        for _ in 0..2 {
            assert!(c.reserve(2).is_ok());
        }
        assert_eq!(c.reserve(2), Err(CapRefusal::PerTurn));
    }
}
