// SPDX-License-Identifier: AGPL-3.0-or-later

//! Subscription modification service — shared by the REST route
//! (`apps/server/src/routes/billing.rs`) and the Leptos server_fn
//! (`crates/kyomi-ui/src/server_fns/billing.rs`).
//!
//! Owns the full "modify an existing subscription" flow so both entry
//! points invalidate MCP sessions immediately. Before consolidation, the
//! Leptos server_fn only updated Stripe + DB and relied on the Stripe
//! webhook to invalidate MCP sessions, leaving MCP clients with stale
//! tool capabilities for the duration of the webhook round-trip.

use kyomi_core::{DbPool, Error};

use crate::{mcp_session_manager::MCPSessionManager, stripe_config, stripe_service::StripeService};

/// Result of a successful subscription modification.
///
/// Callers format this into their response shape (Json for REST,
/// `CheckoutOutcome::Modified` for the Leptos server_fn). The underlying
/// Stripe + DB + MCP invalidation sequence is identical either way.
#[derive(Debug, Clone)]
pub struct ModifySubscriptionResult {
    pub tier: String,
    pub status: String,
    pub billing_cycle: Option<String>,
    pub user_limit: i32,
}

/// Modify an existing Stripe subscription to the current Cloud price,
/// persist the new state to the DB, and immediately invalidate MCP
/// sessions so connected clients pick up the new tool capabilities
/// without waiting for the Stripe webhook round-trip.
///
/// Steps:
/// 1. Call Stripe to update the subscription to the current Cloud price.
/// 2. Write the resulting Stripe state back to the workspaces DB row.
/// 3. Push `notifications/tools/list_changed` to SSE clients on this
///    replica, then invalidate workspace sessions in the KV store so
///    clients connected to other replicas re-initialize on their next
///    request.
pub async fn modify_existing_subscription(
    db: &DbPool,
    stripe: &StripeService,
    mcp_sessions: &MCPSessionManager,
    workspace_id: &str,
    subscription_id: &str,
) -> Result<ModifySubscriptionResult, Error> {
    let new_price_id = stripe_config::get_cloud_price_id().ok_or_else(|| {
        Error::BadRequest("STRIPE_CLOUD_MONTHLY not configured".to_string())
    })?;

    // Modify the subscription to the Cloud price
    let sub_data = stripe
        .update_subscription(subscription_id, new_price_id, "cloud", "monthly")
        .await
        .map_err(|e| {
            tracing::error!("Failed to modify subscription: {e}");
            Error::Internal(format!("Failed to modify subscription: {e}"))
        })?;

    // Update workspace from Stripe data (source of truth)
    let period_start_str = sub_data.period_start.map(|dt| dt.to_rfc3339());
    let period_end_str = sub_data.period_end.map(|dt| dt.to_rfc3339());
    kyomi_core::db_execute!(
        db,
        "UPDATE workspaces SET \
             subscription_tier = $1, \
             subscription_status = $2, \
             billing_cycle = $3, \
             subscription_period_start = $4, \
             subscription_period_end = $5, \
             user_limit = $6 \
         WHERE workspace_id = $7",
        &sub_data.tier,
        &sub_data.status,
        sub_data.billing_cycle.as_deref(),
        period_start_str.as_deref(),
        period_end_str.as_deref(),
        sub_data.user_limit,
        workspace_id
    )?;

    tracing::info!(
        workspace_id = %workspace_id,
        tier = %sub_data.tier,
        "Modified existing subscription"
    );

    // Notify connected SSE clients that tools have changed, then invalidate
    // all sessions so disconnected clients re-initialize on next request.
    mcp_sessions.notify_tools_changed(workspace_id).await;
    mcp_sessions
        .invalidate_workspace_sessions(workspace_id)
        .await;

    Ok(ModifySubscriptionResult {
        tier: sub_data.tier,
        status: sub_data.status,
        billing_cycle: sub_data.billing_cycle,
        user_limit: sub_data.user_limit,
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Verify that MCP invalidation runs against an in-memory KV store.
    ///
    /// The Stripe call itself is not mocked (there's no Stripe mocking
    /// infrastructure in the crate). This test exercises the invalidation
    /// path directly — the service's other behaviour (Stripe update + DB
    /// write) is covered by the REST route's existing contract tests.
    #[tokio::test]
    async fn mcp_invalidation_clears_workspace_sessions() {
        let kv = kyomi_core::kv_store::create_kv_store(None)
            .await
            .expect("in-memory KV store should initialize");
        let mcp = MCPSessionManager::new(kv);

        // Seed two sessions for the target workspace and one for a different workspace.
        let s1 = mcp.create_session("ws-subscription-test-1").await;
        let s2 = mcp.create_session("ws-subscription-test-1").await;
        let other = mcp.create_session("ws-subscription-test-other").await;

        // Perform the invalidation portion of the service flow directly
        // against the same manager. This matches what
        // `modify_existing_subscription` does after the Stripe + DB steps.
        mcp.notify_tools_changed("ws-subscription-test-1").await;
        mcp.invalidate_workspace_sessions("ws-subscription-test-1")
            .await;

        assert!(mcp.validate_session(&s1).await.is_none());
        assert!(mcp.validate_session(&s2).await.is_none());
        // Untargeted workspace must survive.
        assert_eq!(
            mcp.validate_session(&other).await,
            Some("ws-subscription-test-other".to_string())
        );
    }
}
