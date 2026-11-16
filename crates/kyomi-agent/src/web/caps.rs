// SPDX-License-Identifier: AGPL-3.0-or-later

//! Usage caps for Kyomi-executed web calls (owner-confirmed defaults in
//! [`kyomi_core::web_tools::WebCaps`]): a per-turn limit, a per-workspace
//! monthly limit and a global monthly budget. Monthly counters live in the KV
//! store (Redis when configured), so they are shared across server instances
//! and survive restarts. A call is counted when it is *attempted*: the cap
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

/// Reserve one call against the monthly caps. Checks the workspace cap
/// first so a workspace over its own cap never spends the shared global budget.
pub async fn reserve_monthly(
    kv: &KVPool,
    caps: &WebCaps,
    workspace_id: &str,
    tool: WebToolKind,
) -> kyomi_core::Result<Result<(), CapRefusal>> {
    let global_key = month_key("global", tool);
    let ws_key = month_key(&format!("ws:{workspace_id}"), tool);
    // Workspace first: a workspace over its own cap must not spend the shared
    // global budget.
    let ws = kv.incr(&ws_key).await?;
    if ws == 1 {
        kv.expire(&ws_key, MONTH_TTL_SECS).await?;
    }
    if ws > i64::from(caps.workspace_monthly) {
        return Ok(Err(CapRefusal::WorkspaceMonthly));
    }
    let global = kv.incr(&global_key).await?;
    if global == 1 {
        kv.expire(&global_key, MONTH_TTL_SECS).await?;
    }
    if global > i64::from(caps.global_monthly) {
        return Ok(Err(CapRefusal::GlobalMonthly));
    }
    Ok(Ok(()))
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
        // Other workspace still has room, but the global budget (3) is now spent.
        assert_eq!(reserve_monthly(&kv, &c, "b", t).await.unwrap(), Ok(()));
        assert_eq!(reserve_monthly(&kv, &c, "b", t).await.unwrap(), Err(CapRefusal::GlobalMonthly));
    }

    #[tokio::test]
    async fn search_and_fetch_have_separate_counters() {
        let kv = kyomi_core::kv_store_memory::InMemoryKVStore::new_pool();
        let c = caps(1, 10);
        assert_eq!(reserve_monthly(&kv, &c, "a", WebToolKind::Search).await.unwrap(), Ok(()));
        assert_eq!(reserve_monthly(&kv, &c, "a", WebToolKind::Fetch).await.unwrap(), Ok(()));
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
