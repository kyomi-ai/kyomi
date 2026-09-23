// SPDX-License-Identifier: AGPL-3.0-or-later

//! The single "load a workspace, apply the KYO-805 billing gate, fail
//! closed" implementation shared by every enforcement point that isn't the
//! `AuthUser` axum extractor itself (`crate::middleware`, which has its own
//! copy because it already has the `Workspace` row in hand from the
//! membership lookup it must do anyway).
//!
//! Before this module existed, three call sites each carried their own copy
//! of "load the workspace, call `kyomi_core::capability::billing_gate_blocks`,
//! and fail closed on any DB error": the WebSocket sync handlers
//! (`apps/server/src/routes/websocket.rs`), the catalog refresh scheduler
//! (`kyomi_agent::catalog_scheduler::CatalogRefreshScheduler`), and the watch
//! scheduler's due-watch query (`kyomi_agent::scheduler::due_watches_to_execute`).
//! Consolidated here (KYO-805 follow-up) so none of the three can drift from
//! each other's definition of "lapsed" or "fail closed".

use chrono::{DateTime, Utc};

use kyomi_core::DbPool;

/// The outcome of checking whether a workspace's billing gate blocks a piece
/// of work right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceBillingGate {
    /// Not lapsed (or this deployment mode has no billing at all) — proceed.
    Open,
    /// A SaaS workspace whose billing is confirmed lapsed, per
    /// `kyomi_core::capability::billing_gate_blocks`.
    Lapsed,
    /// The gate could not be verified: the workspace row doesn't exist, or a
    /// DB/decode error occurred while loading it. [`Self::blocks`] treats
    /// this the same as [`Self::Lapsed`] (fail closed — never proceed on an
    /// unverifiable workspace), but it is a **distinct** variant so a caller
    /// that reports the refusal reason to a client never claims billing is
    /// lapsed on a workspace it never actually confirmed that about. A
    /// caller in that position must match this variant explicitly rather
    /// than collapsing it into "lapsed" (see
    /// `apps/server/src/routes/websocket.rs`'s use of this type for the
    /// concrete case: a `payment_required` error code must only ever be
    /// sent for a *confirmed* lapse).
    Unverifiable,
}

impl WorkspaceBillingGate {
    /// Whether this outcome should block the caller from proceeding.
    /// `Open` only — both `Lapsed` and `Unverifiable` block, per each
    /// variant's own doc comment.
    pub fn blocks(self) -> bool {
        !matches!(self, Self::Open)
    }
}

/// Check whether `workspace_id`'s billing gate blocks it right now.
///
/// - `self_hosted == true` short-circuits to [`WorkspaceBillingGate::Open`]
///   without touching the database — self-hosted/personal deployments have
///   no billing at all (see `kyomi_core::Config::self_hosted`'s doc
///   comment for why this flag covers both modes).
/// - A workspace that loads successfully is [`WorkspaceBillingGate::Lapsed`]
///   iff `kyomi_core::capability::billing_gate_blocks` says so (the single
///   definition of "lapsed" — never re-derived here), else
///   [`WorkspaceBillingGate::Open`].
/// - A workspace that doesn't exist, or a DB/decode error loading it, is
///   [`WorkspaceBillingGate::Unverifiable`] — logged here (`warn!`/`error!`)
///   so every call site gets that diagnostic for free rather than
///   duplicating it, matching `crate::middleware::load_auth_user`'s handling
///   of the same class of error.
///
/// Callers that only need a yes/no answer should call
/// [`WorkspaceBillingGate::blocks`] on the result; callers that report the
/// refusal reason to an end user must match on the enum so an
/// [`WorkspaceBillingGate::Unverifiable`] outcome is never described as a
/// confirmed billing lapse.
pub async fn check_workspace_billing_gate(
    db: &DbPool,
    workspace_id: &str,
    self_hosted: bool,
    now: DateTime<Utc>,
) -> WorkspaceBillingGate {
    if self_hosted {
        return WorkspaceBillingGate::Open;
    }

    match crate::user_service::get_workspace(db, workspace_id).await {
        Ok(Some(ws)) => {
            if kyomi_core::capability::billing_gate_blocks(&ws, self_hosted, now) {
                WorkspaceBillingGate::Lapsed
            } else {
                WorkspaceBillingGate::Open
            }
        }
        Ok(None) => {
            tracing::warn!(workspace_id, "billing gate: workspace not found — failing closed");
            WorkspaceBillingGate::Unverifiable
        }
        Err(e) => {
            tracing::error!(
                workspace_id,
                error = %e,
                "billing gate: failed to load workspace — failing closed"
            );
            WorkspaceBillingGate::Unverifiable
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{seed_user, seed_workspace, sqlite_pool, test_pool};

    async fn set_subscription_status(sq: &sqlx::SqlitePool, workspace_id: &str, status: &str) {
        sqlx::query("UPDATE workspaces SET subscription_status = $1 WHERE workspace_id = $2")
            .bind(status)
            .bind(workspace_id)
            .execute(sq)
            .await
            .expect("update subscription_status");
    }

    #[tokio::test]
    async fn active_workspace_is_open() {
        let db = test_pool().await;
        seed_user(sqlite_pool(&db), "user-1", "user-1@test.local").await;
        seed_workspace(sqlite_pool(&db), "ws-1", "user-1").await;

        let gate = check_workspace_billing_gate(&db, "ws-1", false, Utc::now()).await;
        assert_eq!(gate, WorkspaceBillingGate::Open);
        assert!(!gate.blocks());
    }

    #[tokio::test]
    async fn lapsed_workspace_is_lapsed() {
        let db = test_pool().await;
        seed_user(sqlite_pool(&db), "user-1", "user-1@test.local").await;
        seed_workspace(sqlite_pool(&db), "ws-1", "user-1").await;
        set_subscription_status(sqlite_pool(&db), "ws-1", "past_due").await;

        let gate = check_workspace_billing_gate(&db, "ws-1", false, Utc::now()).await;
        assert_eq!(gate, WorkspaceBillingGate::Lapsed);
        assert!(gate.blocks());
    }

    #[tokio::test]
    async fn self_hosted_past_due_is_open() {
        let db = test_pool().await;
        seed_user(sqlite_pool(&db), "user-1", "user-1@test.local").await;
        seed_workspace(sqlite_pool(&db), "ws-1", "user-1").await;
        set_subscription_status(sqlite_pool(&db), "ws-1", "past_due").await;

        let gate = check_workspace_billing_gate(&db, "ws-1", true, Utc::now()).await;
        assert_eq!(
            gate,
            WorkspaceBillingGate::Open,
            "self-hosted/personal mode must never block on billing status"
        );
    }

    #[tokio::test]
    async fn missing_workspace_is_unverifiable() {
        let db = test_pool().await;
        let gate = check_workspace_billing_gate(&db, "does-not-exist", false, Utc::now()).await;
        assert_eq!(gate, WorkspaceBillingGate::Unverifiable);
        assert!(
            gate.blocks(),
            "an unverifiable workspace must still fail closed (block)"
        );
    }

    #[tokio::test]
    async fn decode_error_is_unverifiable() {
        let db = test_pool().await;
        seed_user(sqlite_pool(&db), "user-1", "user-1@test.local").await;
        seed_workspace(sqlite_pool(&db), "ws-1", "user-1").await;
        // Corrupt subscription_status so decoding the workspace row fails —
        // same technique as `crate::middleware::tests::db_error_loading_workspace_fails_closed`.
        set_subscription_status(sqlite_pool(&db), "ws-1", "not-a-real-status").await;

        let gate = check_workspace_billing_gate(&db, "ws-1", false, Utc::now()).await;
        assert_eq!(gate, WorkspaceBillingGate::Unverifiable);
        assert!(
            gate.blocks(),
            "a workspace load/decode error must fail closed (block)"
        );
    }
}
