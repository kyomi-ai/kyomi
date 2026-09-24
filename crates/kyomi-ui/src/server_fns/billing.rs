// SPDX-License-Identifier: AGPL-3.0-or-later

//! Server functions for Billing settings.
//!
//! These replace the REST API calls that BillingPanel.jsx makes:
//! - `GET /billing/subscription-info` -> `get_subscription_info()`
//! - `GET /billing/invoices` -> `get_invoices()`
//! - `POST /billing/create-checkout` -> `create_checkout()`
//! - `POST /billing/cancel-subscription` -> `cancel_subscription()`
//! - `POST /billing/reactivate-subscription` -> `reactivate_subscription()`
//! - `POST /billing/create-portal-session` -> `create_portal_session()`
//!
//! Calls the same service-layer code as `apps/server/src/routes/billing.rs`.

use leptos::prelude::*;
use serde::{Deserialize, Serialize};

use kyomi_types::BillingLapseReason;

#[cfg(feature = "ssr")]
use super::{
    extract_auth_allow_lapsed, extract_context, AuthenticatedContext, IntoServerFnErrorCore,
    IntoServerFnErrorSqlx,
};
#[cfg(feature = "ssr")]
use kyomi_types::Permission;

// ─────────────────────────────────────────────────────────────────────────────
// Types
// ─────────────────────────────────────────────────────────────────────────────

/// Subscription information for the current workspace.
///
/// Cloud plan — single tier at $5/user/month. The `billing_cycle` field is
/// retained for backward compatibility but is always "monthly" for new
/// subscriptions.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SubscriptionInfo {
    pub tier: String,
    pub status: String,
    pub billing_cycle: Option<String>,
    pub period_start: Option<String>,
    pub period_end: Option<String>,
    pub ai_reset_date: Option<String>,
    /// Trial expiration timestamp (ISO 8601). Present when status is "trialing".
    pub trial_ends_at: Option<String>,
    pub user_limit: Option<i32>,
    /// Number of analytics events consumed this month.
    pub analytics_events_used: Option<i64>,
    /// Remaining purchased analytics event bundle balance (non-expiring).
    pub analytics_bundle_balance: Option<i64>,
    /// Number of active members in the workspace (for seat billing display).
    pub active_members: i32,
}

/// A single invoice record.
///
/// Matches the JSON shape returned by `GET /billing/invoices`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct InvoiceRecord {
    pub invoice_id: String,
    pub amount_paid: f64,
    pub currency: String,
    pub status: Option<String>,
    pub hosted_invoice_url: Option<String>,
    pub invoice_pdf: Option<String>,
    pub created: Option<i64>,
    pub description: Option<String>,
}

/// Result of a checkout or portal session creation — a URL to redirect to.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RedirectUrl {
    pub url: String,
}

/// Result of creating an embedded checkout session.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EmbeddedCheckoutSession {
    pub client_secret: String,
    pub session_id: String,
}

/// Result of creating a subscription checkout — either embedded checkout
/// (for new subscriptions) or an immediate modification result.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum CheckoutOutcome {
    /// New subscription — mount embedded checkout with this client_secret.
    Embedded(EmbeddedCheckoutSession),
    /// Existing subscription modified — no checkout needed.
    Modified(String),
}

/// Status of a checkout session (for verifying completion from onComplete callback).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CheckoutStatus {
    pub status: String,
    pub payment_status: String,
}

/// Result of a mutation (cancel, reactivate, update team size).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BillingResult {
    pub message: String,
}

/// Which flow the paywall's primary call-to-action should trigger
/// (KYO-806 A2), computed server-side by `kyomi_auth::subscription_service::checkout_path`
/// (via `get_billing_paywall`) — the client never re-derives this from
/// `reason` or any other field.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PaywallAction {
    /// `past_due` with an existing Stripe subscription — pay the open
    /// invoice via `start_payment_recovery` / `complete_payment_recovery`.
    /// Must NEVER go through `create_checkout` (KYO-806 A6's guard).
    RecoverPayment,
    /// Every other lapsed case (`cancelled` with no live subscription, or
    /// the expired no-Stripe trial) — start a new subscription via
    /// `create_checkout`.
    Subscribe,
}

/// Details the full-screen billing paywall renders from (KYO-806 A2).
///
/// `reason` is the copy driver — `None` means the workspace isn't actually
/// lapsed (including: self-hosted/personal mode, which is never lapsed).
/// The paywall's decision to render at ALL is never based on this DTO: the
/// client's authority for that is `SidebarUser.billing_lapsed`
/// (KYO-805/KYO-811), computed once by the `AuthUser` extractor. This
/// endpoint only supplies the copy/action for a paywall the client has
/// already decided to show.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BillingPaywall {
    pub reason: Option<BillingLapseReason>,
    pub workspace_name: Option<String>,
    /// Same rule `require_workspace_owner` enforces server-side
    /// (`permissions_for(auth).contains(Permission::ManageBilling)`) — lets
    /// the client show/hide the "Add payment method" CTA vs. the
    /// "Ask {owner} to update billing" message for a non-owner member.
    pub can_manage_billing: bool,
    /// The workspace owner's contact — only what any workspace member may
    /// already see (name + email), for "Ask {owner} to update billing"
    /// copy when `can_manage_billing` is `false`.
    pub owner_name: Option<String>,
    pub owner_email: Option<String>,
    pub action: PaywallAction,
    /// Seat quantity the `Subscribe` action should pass to `create_checkout`
    /// — mirrors the active-member count the settings billing page
    /// (`crates/kyomi-ui/src/pages/settings/billing.rs`) and
    /// `get_subscription_info` both use. Always at least 1.
    pub seat_count: u64,
}

/// Outcome of a `complete_payment_recovery` call (KYO-806 A3) — the wire
/// mirror of `kyomi_auth::payment_recovery::RecoveryOutcome`, which lives
/// ssr-only in kyomi-auth and can't be referenced from WASM client code
/// directly.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum PaymentRecoveryOutcome {
    /// Every open invoice on the subscription is now paid; the workspace is
    /// no longer lapsed.
    Recovered,
    /// The payment needs further customer action (e.g. 3-D Secure) before
    /// it can succeed.
    NeedsAction { hosted_invoice_url: Option<String> },
    /// The payment attempt failed outright (e.g. the card was declined).
    Declined { message: String },
}

#[cfg(feature = "ssr")]
impl From<kyomi_auth::payment_recovery::RecoveryOutcome> for PaymentRecoveryOutcome {
    fn from(outcome: kyomi_auth::payment_recovery::RecoveryOutcome) -> Self {
        match outcome {
            kyomi_auth::payment_recovery::RecoveryOutcome::Recovered => Self::Recovered,
            kyomi_auth::payment_recovery::RecoveryOutcome::NeedsAction { hosted_invoice_url } => {
                Self::NeedsAction { hosted_invoice_url }
            }
            kyomi_auth::payment_recovery::RecoveryOutcome::Declined { message } => {
                Self::Declined { message }
            }
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// SSR-only helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Reject anyone who isn't the workspace owner.
///
/// Billing is owner-only because the owner is the single spending authority
/// for the workspace. Admins can invite users (consuming seats), but only the
/// owner can change subscription plan, buy bundles, or adjust the seat cap.
///
/// Sets the HTTP response status to 403 Forbidden via `ResponseOptions` on
/// the reject path so tower_http and the browser don't classify owner-only
/// rejection as a 5xx server error. Mirrors the 401 pattern in
/// `extract_auth` — permission failures are client errors, not server
/// errors.
#[cfg(feature = "ssr")]
fn require_workspace_owner(
    auth: &kyomi_auth::middleware::AuthUser,
) -> Result<(), ServerFnError> {
    if kyomi_auth::permissions::permissions_for(auth).contains(&Permission::ManageBilling) {
        Ok(())
    } else {
        leptos::prelude::expect_context::<leptos_axum::ResponseOptions>()
            .set_status(axum::http::StatusCode::FORBIDDEN);
        Err(ServerFnError::new("Workspace owner access required"))
    }
}

/// Minimal workspace row for billing operations.
#[cfg(feature = "ssr")]
#[derive(Debug, sqlx::FromRow)]
struct WorkspaceRow {
    name: Option<String>,
    subscription_tier: String,
    subscription_status: String,
    billing_cycle: Option<String>,
    subscription_period_start: Option<String>,
    subscription_period_end: Option<String>,
    trial_ends_at: Option<String>,
    user_limit: Option<i32>,
    stripe_customer_id: Option<String>,
    stripe_subscription_id: Option<String>,
    #[sqlx(default)]
    analytics_bundle_events: Option<i64>,
}

/// Parse a timestamp produced by `CAST(timestamptz AS TEXT)` in Postgres
/// (`YYYY-MM-DD HH:MM:SS[.fff]+00`) or a standard RFC3339 string. Returns
/// `None` on failure.
///
/// Postgres's `timestamptz::text` cast uses a space instead of `T` between
/// the date and time, and a short `+00` offset rather than `+00:00`, which
/// `chrono::DateTime::parse_from_rfc3339` rejects. We try RFC3339 first and
/// fall back to the Postgres text format.
#[cfg(feature = "ssr")]
fn parse_pg_or_rfc3339(s: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(s) {
        return Some(dt.with_timezone(&chrono::Utc));
    }
    // Postgres text cast: 2026-05-12 07:39:04.698546+00
    for fmt in ["%Y-%m-%d %H:%M:%S%.f%#z", "%Y-%m-%d %H:%M:%S%#z"] {
        if let Ok(dt) = chrono::DateTime::parse_from_str(s, fmt) {
            return Some(dt.with_timezone(&chrono::Utc));
        }
    }
    None
}

#[cfg(feature = "ssr")]
impl WorkspaceRow {
    fn period_start_dt(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        self.subscription_period_start
            .as_deref()
            .and_then(parse_pg_or_rfc3339)
    }
    fn period_end_dt(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        self.subscription_period_end
            .as_deref()
            .and_then(parse_pg_or_rfc3339)
    }
    fn trial_ends_at_dt(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        self.trial_ends_at.as_deref().and_then(parse_pg_or_rfc3339)
    }
}

/// Load a workspace by workspace_id.
#[cfg(feature = "ssr")]
async fn load_workspace(
    db: &kyomi_core::DbPool,
    ws_id: &str,
) -> Result<WorkspaceRow, ServerFnError> {
    kyomi_core::db_fetch_optional!(
        db, WorkspaceRow,
        "SELECT name, subscription_tier, subscription_status, \
         billing_cycle, \
         CAST(subscription_period_start AS TEXT) AS subscription_period_start, \
         CAST(subscription_period_end AS TEXT) AS subscription_period_end, \
         CAST(trial_ends_at AS TEXT) AS trial_ends_at, \
         user_limit, \
         stripe_customer_id, stripe_subscription_id, \
         COALESCE(analytics_bundle_events, 0) AS analytics_bundle_events \
         FROM workspaces WHERE workspace_id = $1",
        ws_id
    )
    .into_sfn_sqlx()?
    .ok_or_else(|| ServerFnError::new("Workspace not found"))
}

/// Get the StripeService from config, or error.
#[cfg(feature = "ssr")]
fn require_stripe(
    config: &kyomi_core::Config,
) -> Result<kyomi_auth::stripe_service::StripeService, ServerFnError> {
    let secret_key = config
        .stripe_secret_key
        .as_deref()
        .ok_or_else(|| ServerFnError::new("Billing features are not available"))?;
    let webhook_secret = config
        .stripe_webhook_secret
        .as_deref()
        .unwrap_or_default();
    Ok(kyomi_auth::stripe_service::StripeService::new(
        secret_key,
        webhook_secret,
    ))
}

// ─────────────────────────────────────────────────────────────────────────────
// Server functions
// ─────────────────────────────────────────────────────────────────────────────

/// Fetch subscription info for the current workspace.
///
/// Mirrors `GET /api/v1/billing/subscription-info`.
///
/// Note: subscription data (tier, status, period) comes from the database and
/// is always available regardless of Stripe configuration. Only checkout and
/// portal operations require Stripe.
///
/// Allowlisted while billing is lapsed (KYO-805, via `extract_allow_lapsed`)
/// — this is the endpoint that tells the owner their subscription IS lapsed.
#[server(prefix = "/leptos-api", client = crate::server_fns::paywall_client::PaywallAwareClient)]
pub async fn get_subscription_info() -> Result<SubscriptionInfo, ServerFnError> {
    let ac = AuthenticatedContext::extract_allow_lapsed().await?;

    require_workspace_owner(&ac.auth)?;

    let workspace = load_workspace(ac.db(), &ac.ws_id).await?;

    // Calculate AI reset date
    let ai_reset_date = {
        let billing_cycle = workspace.billing_cycle.as_deref();
        let period_start = workspace.period_start_dt();
        let period_end = workspace.period_end_dt();

        if let Some(start) = period_start {
            if billing_cycle == Some("monthly") {
                period_end.map(|dt| dt.to_rfc3339())
            } else {
                let now = chrono::Utc::now();
                let (_, monthly_end) =
                    kyomi_auth::billing_service::BillingService::calculate_monthly_period(
                        start, now,
                    );
                Some(monthly_end.to_rfc3339())
            }
        } else {
            None
        }
    };

    // Analytics events this month from Redis (same pattern as usage.rs).
    // Falls back to 0 if Redis is unavailable.
    let analytics_events_used: i64 = if let Some(ref redis_url) = ac.ctx.config.redis_url {
        match kyomi_core::redis::create_pool(redis_url).await {
            Ok(mut conn) => {
                kyomi_auth::analytics_quota::get_usage_count(&mut conn, &ac.ws_id)
                    .await
                    .unwrap_or(0) as i64
            }
            Err(_) => 0,
        }
    } else {
        0
    };

    // Analytics bundle balance from the workspace row (already loaded)
    let analytics_bundle_balance = workspace.analytics_bundle_events.unwrap_or(0);

    // Count active workspace members for seat billing display
    let bt = kyomi_core::sql_compat::bool_true(ac.db().is_postgres());
    let count_sql = format!(
        "SELECT COUNT(*) FROM workspace_users WHERE workspace_id = $1 AND active = {bt}"
    );
    let active_members: i32 = kyomi_core::db_fetch_scalar!(
        ac.db(), i64, &count_sql, &ac.ws_id
    ).into_sfn_sqlx()? as i32;

    // Normalize all timestamps to RFC3339 so the frontend's date formatter
    // (which expects RFC3339) renders them correctly. Postgres's
    // `timestamptz::text` cast uses a space separator that trips the parser.
    let period_start_rfc = workspace.period_start_dt().map(|dt| dt.to_rfc3339());
    let period_end_rfc = workspace.period_end_dt().map(|dt| dt.to_rfc3339());
    let trial_ends_at_rfc = workspace.trial_ends_at_dt().map(|dt| dt.to_rfc3339());

    Ok(SubscriptionInfo {
        tier: workspace.subscription_tier,
        status: workspace.subscription_status,
        billing_cycle: workspace.billing_cycle,
        period_start: period_start_rfc,
        period_end: period_end_rfc,
        ai_reset_date,
        trial_ends_at: trial_ends_at_rfc,
        user_limit: workspace.user_limit,
        analytics_events_used: Some(analytics_events_used),
        analytics_bundle_balance: Some(analytics_bundle_balance),
        active_members,
    })
}

/// Fetch recent invoices for the current workspace.
///
/// Mirrors `GET /api/v1/billing/invoices`.
///
/// Allowlisted while billing is lapsed (KYO-805) — the owner needs to see
/// past invoices while sorting out a lapsed subscription.
#[server(prefix = "/leptos-api", client = crate::server_fns::paywall_client::PaywallAwareClient)]
pub async fn get_invoices() -> Result<Vec<InvoiceRecord>, ServerFnError> {
    let ac = AuthenticatedContext::extract_allow_lapsed().await?;

    // Stripe not configured — no invoices to show.
    if ac.ctx.config.stripe_secret_key.is_none() {
        return Ok(vec![]);
    }

    require_workspace_owner(&ac.auth)?;

    let workspace = load_workspace(ac.db(), &ac.ws_id).await?;

    // If no Stripe customer, return empty list
    let customer_id = match workspace.stripe_customer_id {
        Some(ref id) if !id.is_empty() => id.clone(),
        _ => return Ok(vec![]),
    };

    let stripe_service = require_stripe(&ac.ctx.config)?;
    let invoices = stripe_service
        .list_invoices(&customer_id, 10)
        .await
        .map_err(|e| ServerFnError::new(format!("Failed to fetch invoices: {e}")))?;

    Ok(invoices
        .into_iter()
        .map(|inv| InvoiceRecord {
            invoice_id: inv.invoice_id,
            amount_paid: inv.amount_paid as f64 / 100.0,
            currency: inv.currency.to_uppercase(),
            status: inv.status,
            hosted_invoice_url: inv.hosted_invoice_url,
            invoice_pdf: inv.invoice_pdf,
            created: inv.created,
            description: None,
        })
        .collect())
}

/// Create a Stripe checkout session for subscription.
///
/// Returns `CheckoutOutcome::Embedded` with a client_secret for new
/// subscriptions (mount via Stripe.js embedded checkout), or
/// `CheckoutOutcome::Modified` when an existing subscription was
/// reactivated directly (no checkout needed).
///
/// Allowlisted while billing is lapsed (KYO-805) — this is how the owner
/// pays to un-lapse the workspace for the `cancelled` / expired-trial cases.
/// `past_due` is refused below (KYO-806 A6) — see [`start_payment_recovery`]
/// for that case instead.
#[server(prefix = "/leptos-api", client = crate::server_fns::paywall_client::PaywallAwareClient)]
pub async fn create_checkout(
    quantity: u64,
) -> Result<CheckoutOutcome, ServerFnError> {
    // lint-allow: server-fn-callouts=this fn has no REST counterpart to drift from — the only REST billing route is the Stripe webhook (see apps/server/tests/contract_billing.rs module doc), not a duplicate of this checkout-creation flow; the 4th callout, checkout_path(, is a pure routing decision (no I/O) that must run before require_stripe below to enforce the KYO-806 past_due guard
    let ac = AuthenticatedContext::extract_allow_lapsed().await?;
    require_workspace_owner(&ac.auth)?;

    let workspace = load_workspace(ac.db(), &ac.ws_id).await?;

    // KYO-806 A6: decide the routing BEFORE any Stripe call (`require_stripe`
    // included) — a past_due workspace with a live subscription must never
    // reach the new-subscription flow below, which would create a second,
    // competing subscription. `checkout_path` is the one place this
    // decision is made; see its doc comment for why NewSubscription is
    // still correct for a past_due row with no subscription id.
    let path = kyomi_auth::subscription_service::checkout_path(
        &workspace.subscription_status,
        workspace.stripe_subscription_id.as_deref(),
    );

    if path == kyomi_auth::subscription_service::CheckoutPath::RecoverPastDue {
        leptos::prelude::expect_context::<leptos_axum::ResponseOptions>()
            .set_status(axum::http::StatusCode::CONFLICT);
        return Err(ServerFnError::new(
            "This subscription is past due. Add a payment method via payment recovery to pay \
             the open invoice and reactivate it — starting a new subscription here would create \
             a second, duplicate one.",
        ));
    }

    let stripe_service = require_stripe(&ac.ctx.config)?;

    // If user already has an active subscription, modify it directly.
    //
    // Delegates to the shared service so this path performs the same
    // Stripe + DB + MCP invalidation sequence as the REST route. Without
    // this, MCP clients would only see updated tool capabilities after
    // the Stripe webhook round-trip.
    if path == kyomi_auth::subscription_service::CheckoutPath::ModifyExisting {
        let sub_id = workspace.stripe_subscription_id.as_deref().ok_or_else(|| {
            ServerFnError::new("Workspace subscription state is inconsistent — missing subscription id")
        })?;

        kyomi_auth::subscription_service::modify_existing_subscription(
            ac.db(),
            &stripe_service,
            ac.ctx.mcp_sessions
                .as_ref()
                .ok_or_else(|| ServerFnError::new("MCP session manager unavailable"))?,
            &ac.ws_id,
            sub_id,
        )
        .await
        .into_sfn_core()?;

        return Ok(CheckoutOutcome::Modified(
            "Subscription reactivated successfully".to_string(),
        ));
    }

    // New subscription flow — single Cloud price from env
    let price_id = kyomi_auth::stripe_config::get_cloud_price_id()
        .ok_or_else(|| {
            ServerFnError::new("STRIPE_CLOUD_MONTHLY not configured")
        })?;

    // Create Stripe customer if workspace doesn't have one
    let customer_id = match workspace.stripe_customer_id {
        Some(ref id) if !id.is_empty() => id.clone(),
        _ => {
            let email = &ac.auth.email;
            let ws_name = workspace.name.as_deref().unwrap_or("Unnamed");
            let new_customer_id = stripe_service
                .create_customer(email, &ac.ws_id, ws_name)
                .await
                .map_err(|e| ServerFnError::new(format!("Failed to create Stripe customer: {e}")))?;

            kyomi_core::db_execute!(
                ac.db(),
                "UPDATE workspaces SET stripe_customer_id = $1 WHERE workspace_id = $2",
                &new_customer_id,
                &ac.ws_id
            )
            .into_sfn_sqlx()?;

            new_customer_id
        }
    };

    let params = kyomi_auth::stripe_service::EmbeddedCheckoutParams {
        customer_id,
        price_id: price_id.to_string(),
        workspace_id: ac.ws_id.clone(),
        quantity,
        trial_days: 30,
    };

    let result = stripe_service
        .create_embedded_checkout_session(&params)
        .await
        .map_err(|e| ServerFnError::new(format!("Failed to create checkout session: {e}")))?;

    Ok(CheckoutOutcome::Embedded(EmbeddedCheckoutSession {
        client_secret: result.client_secret,
        session_id: result.session_id,
    }))
}

/// Fetch the details the full-screen billing paywall renders from
/// (KYO-806 A2) — copy, the workspace owner's contact, and which action
/// (`RecoverPayment` vs. `Subscribe`) the primary CTA should trigger.
///
/// Allowlisted while billing is lapsed (KYO-805) — this is the entire point:
/// it's read WHILE lapsed to render the paywall that un-lapses the
/// workspace. The paywall's decision to show at all is `SidebarUser.billing_lapsed`
/// (already allow-lapsed); this endpoint only supplies what to render once
/// that decision is made.
#[server(prefix = "/leptos-api", client = crate::server_fns::paywall_client::PaywallAwareClient)]
pub async fn get_billing_paywall() -> Result<BillingPaywall, ServerFnError> {
    let ac = AuthenticatedContext::extract_allow_lapsed().await?;

    let can_manage_billing = ac.has(Permission::ManageBilling);

    let workspace = kyomi_auth::workspace_service::get_workspace_full(ac.db(), &ac.ws_id)
        .await
        .into_sfn_core()?
        .ok_or_else(|| ServerFnError::new("Workspace not found"))?;

    let now = chrono::Utc::now();

    // The one gate `AuthUser`/`AuthUserAllowLapsed` also compute
    // (`kyomi_core::capability::billing_gate_blocks`) — covers self-hosted
    // AND personal mode identically, since personal mode always carries
    // `self_hosted = true` (see `KyomiMode::self_hosted`). No separate
    // hand-rolled personal-mode check here: a second, independent gate is
    // exactly what would let this endpoint and the server-side enforcement
    // drift apart.
    let reason = kyomi_core::capability::billing_gate_blocks(&workspace, ac.ctx.config.self_hosted, now)
        .then(|| kyomi_core::capability::billing_lapse_reason(&workspace, now))
        .flatten();

    let (owner_name, owner_email) =
        match kyomi_auth::user_service::get_user_by_id(ac.db(), &workspace.owner_user_id)
            .await
            .into_sfn_core()?
        {
            Some(owner) => (owner.name, Some(owner.email)),
            None => (None, None),
        };

    // Derive the CTA from `checkout_path` — the same decision
    // `create_checkout`'s guard (KYO-806 A6) makes — rather than a second,
    // independent match on `subscription_status` that could silently
    // disagree with it.
    let action = match kyomi_auth::subscription_service::checkout_path(
        workspace.subscription_status.as_ref(),
        workspace.stripe_subscription_id.as_deref(),
    ) {
        kyomi_auth::subscription_service::CheckoutPath::RecoverPastDue => {
            PaywallAction::RecoverPayment
        }
        kyomi_auth::subscription_service::CheckoutPath::ModifyExisting
        | kyomi_auth::subscription_service::CheckoutPath::NewSubscription => {
            PaywallAction::Subscribe
        }
    };

    // Seat quantity for the Subscribe path — same active-member count
    // get_subscription_info reports and the settings billing page
    // (crates/kyomi-ui/src/pages/settings/billing.rs) uses to seed its
    // seat-count control.
    let bt = kyomi_core::sql_compat::bool_true(ac.db().is_postgres());
    let count_sql =
        format!("SELECT COUNT(*) FROM workspace_users WHERE workspace_id = $1 AND active = {bt}");
    let active_members: i64 =
        kyomi_core::db_fetch_scalar!(ac.db(), i64, &count_sql, &ac.ws_id).into_sfn_sqlx()?;

    Ok(BillingPaywall {
        reason,
        workspace_name: workspace.name.clone(),
        can_manage_billing,
        owner_name,
        owner_email,
        action,
        seat_count: active_members.max(1) as u64,
    })
}

/// Start past-due payment recovery: create an embedded Setup-mode Checkout
/// Session for the workspace's existing Stripe customer (KYO-806 A3).
///
/// Requires `kyomi_auth::subscription_service::checkout_path` to resolve to
/// `CheckoutPath::RecoverPastDue` for this workspace (`past_due` status with
/// a live `stripe_subscription_id`) plus a `stripe_customer_id` on file —
/// every other lapsed state uses [`create_checkout`] instead. Owner-only,
/// same as every other billing mutation. The returned session mounts via
/// the same embedded-checkout flow as [`create_checkout`]'s
/// `CheckoutOutcome::Embedded` — this is a Setup-mode session, not a
/// Subscription-mode one, so it never creates a second subscription.
///
/// Allowlisted while billing is lapsed (KYO-805) — this is how a past_due
/// owner pays without waiting for Stripe's automatic invoice retry (which
/// can take days).
#[server(prefix = "/leptos-api", client = crate::server_fns::paywall_client::PaywallAwareClient)]
pub async fn start_payment_recovery() -> Result<EmbeddedCheckoutSession, ServerFnError> {
    let ac = AuthenticatedContext::extract_allow_lapsed().await?;
    require_workspace_owner(&ac.auth)?;

    let workspace = load_workspace(ac.db(), &ac.ws_id).await?;

    // The single routing decision (KYO-806 A6) — the same one
    // `create_checkout`'s guard and `get_billing_paywall`'s CTA both defer
    // to — rather than a third, independent hand-rolled check on
    // `subscription_status`/`stripe_subscription_id` that could silently
    // disagree with it.
    let path = kyomi_auth::subscription_service::checkout_path(
        &workspace.subscription_status,
        workspace.stripe_subscription_id.as_deref(),
    );
    if path != kyomi_auth::subscription_service::CheckoutPath::RecoverPastDue {
        return Err(ServerFnError::new(
            "Payment recovery is only available for a past-due subscription with an existing \
             Stripe subscription on file — use Subscribe instead",
        ));
    }

    let customer_id = workspace.stripe_customer_id.as_deref().ok_or_else(|| {
        ServerFnError::new("No Stripe customer on file for this workspace")
    })?;

    let stripe_service = require_stripe(&ac.ctx.config)?;

    let result = stripe_service
        .create_recovery_setup_session(&kyomi_auth::stripe_service::RecoverySetupSessionParams {
            customer_id: customer_id.to_string(),
            workspace_id: ac.ws_id.clone(),
        })
        .await
        .map_err(|e| {
            ServerFnError::new(format!("Failed to create payment recovery session: {e}"))
        })?;

    Ok(EmbeddedCheckoutSession {
        client_secret: result.client_secret,
        session_id: result.session_id,
    })
}

/// Complete past-due payment recovery once the embedded Setup Checkout
/// Session's `onComplete` callback fires (KYO-806 A3).
///
/// Applies the collected payment method to the customer and subscription,
/// pays every open invoice on the subscription, and — only once every
/// invoice is confirmed paid — refreshes the workspace's subscription state
/// so an immediate client refetch of `SidebarUser`/`UserContext` no longer
/// reports lapsed. See `kyomi_auth::payment_recovery::recover_past_due_payment`
/// for the full flow and its idempotency guarantees (this call and the
/// webhook backstop in `apps/server/src/routes/billing.rs` can race safely).
///
/// Allowlisted while billing is lapsed (KYO-805) — this fires at the tail
/// of the exact checkout flow that un-lapses the workspace, so the
/// workspace is very possibly still lapsed at the moment this runs.
#[server(prefix = "/leptos-api", client = crate::server_fns::paywall_client::PaywallAwareClient)]
pub async fn complete_payment_recovery(
    session_id: String,
) -> Result<PaymentRecoveryOutcome, ServerFnError> {
    let ac = AuthenticatedContext::extract_allow_lapsed().await?;
    require_workspace_owner(&ac.auth)?;

    let workspace = load_workspace(ac.db(), &ac.ws_id).await?;
    let subscription_id = workspace.stripe_subscription_id.as_deref().ok_or_else(|| {
        ServerFnError::new("No subscription on file to recover")
    })?;
    let customer_id = workspace.stripe_customer_id.as_deref().ok_or_else(|| {
        ServerFnError::new("No Stripe customer on file for this workspace")
    })?;

    let stripe_service = require_stripe(&ac.ctx.config)?;
    let mcp_sessions = ac.ctx.mcp_sessions.as_ref().ok_or_else(|| {
        ServerFnError::new("MCP session manager unavailable")
    })?;

    let outcome = kyomi_auth::payment_recovery::recover_past_due_payment(
        ac.db(),
        &stripe_service,
        mcp_sessions,
        kyomi_auth::payment_recovery::RecoveryIds {
            workspace_id: &ac.ws_id,
            stripe_customer_id: customer_id,
            stripe_subscription_id: subscription_id,
            session_id: &session_id,
        },
    )
    .await
    .into_sfn_core()?;

    Ok(outcome.into())
}

/// Sync a completed new-subscription Checkout Session's resulting
/// subscription into the workspace row immediately (KYO-806 A4).
///
/// Without this, the DB only learns of the new subscription from the
/// `customer.subscription.created` webhook, so a client refetch right after
/// the embedded checkout's `onComplete` would still see the workspace as
/// lapsed. Used for the `Subscribe` paywall action (`create_checkout`'s
/// `CheckoutOutcome::Embedded` path) — idempotent with that webhook; see
/// `kyomi_auth::payment_recovery::sync_new_subscription_checkout`.
///
/// Allowlisted while billing is lapsed (KYO-805) — fires at the tail of the
/// exact checkout flow that un-lapses the workspace.
#[server(prefix = "/leptos-api", client = crate::server_fns::paywall_client::PaywallAwareClient)]
pub async fn sync_checkout_subscription(session_id: String) -> Result<(), ServerFnError> {
    let ac = AuthenticatedContext::extract_allow_lapsed().await?;
    require_workspace_owner(&ac.auth)?;

    let workspace = load_workspace(ac.db(), &ac.ws_id).await?;
    let customer_id = workspace.stripe_customer_id.as_deref().ok_or_else(|| {
        ServerFnError::new("No Stripe customer on file for this workspace")
    })?;

    let stripe_service = require_stripe(&ac.ctx.config)?;
    let mcp_sessions = ac.ctx.mcp_sessions.as_ref().ok_or_else(|| {
        ServerFnError::new("MCP session manager unavailable")
    })?;

    kyomi_auth::payment_recovery::sync_new_subscription_checkout(
        ac.db(),
        &stripe_service,
        mcp_sessions,
        &ac.ws_id,
        customer_id,
        &session_id,
    )
    .await
    .into_sfn_core()?;

    Ok(())
}

/// Cancel the current subscription at period end.
///
/// Mirrors `POST /api/v1/billing/cancel-subscription`.
#[server(prefix = "/leptos-api", client = crate::server_fns::paywall_client::PaywallAwareClient)]
pub async fn cancel_subscription() -> Result<BillingResult, ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;
    require_workspace_owner(&ac.auth)?;

    let stripe_service = require_stripe(&ac.ctx.config)?;
    let workspace = load_workspace(ac.db(), &ac.ws_id).await?;

    let sub_id = workspace
        .stripe_subscription_id
        .as_deref()
        .ok_or_else(|| ServerFnError::new("No active subscription"))?;

    stripe_service
        .cancel_subscription(sub_id, true)
        .await
        .map_err(|e| ServerFnError::new(format!("Failed to cancel subscription: {e}")))?;

    kyomi_core::db_execute!(
        ac.db(),
        "UPDATE workspaces SET subscription_status = 'cancelled' WHERE workspace_id = $1",
        &ac.ws_id
    )
    .into_sfn_sqlx()?;

    Ok(BillingResult {
        message: "Subscription will be cancelled at the end of your billing period".to_string(),
    })
}

/// Reactivate a cancelled subscription.
///
/// Mirrors `POST /api/v1/billing/reactivate-subscription`.
#[server(prefix = "/leptos-api", client = crate::server_fns::paywall_client::PaywallAwareClient)]
pub async fn reactivate_subscription() -> Result<BillingResult, ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;
    require_workspace_owner(&ac.auth)?;

    let stripe_service = require_stripe(&ac.ctx.config)?;
    let workspace = load_workspace(ac.db(), &ac.ws_id).await?;

    let sub_id = workspace
        .stripe_subscription_id
        .as_deref()
        .ok_or_else(|| ServerFnError::new("No subscription to reactivate"))?;

    if workspace.subscription_status != "cancelled" {
        return Err(ServerFnError::new("Subscription is not cancelled"));
    }

    stripe_service
        .reactivate_subscription(sub_id)
        .await
        .map_err(|e| ServerFnError::new(format!("Failed to reactivate subscription: {e}")))?;

    kyomi_core::db_execute!(
        ac.db(),
        "UPDATE workspaces SET subscription_status = 'active' WHERE workspace_id = $1",
        &ac.ws_id
    )
    .into_sfn_sqlx()?;

    Ok(BillingResult {
        message: "Subscription has been reactivated".to_string(),
    })
}

/// The DB sentinel for "no seat cap". Treated as unlimited by the invite flow.
pub const UNLIMITED_SEAT_CAP: i32 = 999_999;

/// Update the workspace seat cap — the owner's spending ceiling.
///
/// Workspace admins can invite users up to this cap. Owner-only because
/// raising the cap increases monthly Stripe charges.
///
/// Validates that the new cap is at least as high as current active members —
/// lowering below that would require removing users first.
#[server(prefix = "/leptos-api", client = crate::server_fns::paywall_client::PaywallAwareClient)]
pub async fn update_user_limit(limit: i32) -> Result<i32, ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;
    require_workspace_owner(&ac.auth)?;

    if limit < 1 {
        return Err(ServerFnError::new("Seat cap must be at least 1"));
    }

    // Count active workspace members
    let bt = kyomi_core::sql_compat::bool_true(ac.db().is_postgres());
    let count_sql = format!(
        "SELECT COUNT(*) FROM workspace_users WHERE workspace_id = $1 AND active = {bt}"
    );
    let active: i64 = kyomi_core::db_fetch_scalar!(ac.db(), i64, &count_sql, &ac.ws_id)
        .into_sfn_sqlx()?;

    if (limit as i64) < active {
        return Err(ServerFnError::new(format!(
            "Cannot set seat cap below current active members ({active}). Remove users first."
        )));
    }

    kyomi_core::db_execute!(
        ac.db(),
        "UPDATE workspaces SET user_limit = $1 WHERE workspace_id = $2",
        limit,
        &ac.ws_id
    )
    .into_sfn_sqlx()?;

    tracing::info!(workspace_id = %ac.ws_id, limit, "Updated workspace seat cap");

    Ok(limit)
}

/// Create a Stripe billing portal session and return the redirect URL.
///
/// Mirrors `POST /api/v1/billing/create-portal-session`.
///
/// Allowlisted while billing is lapsed (KYO-805) — the Stripe portal is
/// where the owner updates a failed payment method.
#[server(prefix = "/leptos-api", client = crate::server_fns::paywall_client::PaywallAwareClient)]
pub async fn create_portal_session() -> Result<RedirectUrl, ServerFnError> {
    let ac = AuthenticatedContext::extract_allow_lapsed().await?;
    require_workspace_owner(&ac.auth)?;

    let stripe_service = require_stripe(&ac.ctx.config)?;
    let workspace = load_workspace(ac.db(), &ac.ws_id).await?;

    let customer_id = workspace.stripe_customer_id.as_deref().ok_or_else(|| {
        ServerFnError::new("No Stripe customer found. Please subscribe to a plan first.")
    })?;

    // Portal returns via cross-site redirect from Stripe — use the
    // intermediate bounce page so SameSite=Strict cookies work.
    let return_url = format!("{}/billing/return", ac.ctx.config.frontend_url);

    let (portal_url, _session_id) = stripe_service
        .create_portal_session(customer_id, &return_url)
        .await
        .map_err(|e| ServerFnError::new(format!("Failed to create portal session: {e}")))?;

    Ok(RedirectUrl { url: portal_url })
}

/// Purchase an AI token bundle via embedded Stripe checkout.
///
/// Returns an `EmbeddedCheckoutSession` with the client_secret to mount
/// the Stripe form inline. The webhook handler credits the workspace's
/// `ai_bundle_balance_usd` upon successful payment.
#[server(prefix = "/leptos-api", client = crate::server_fns::paywall_client::PaywallAwareClient)]
pub async fn purchase_ai_bundle(quantity: u32) -> Result<EmbeddedCheckoutSession, ServerFnError> {
    if quantity < 1 {
        return Err(ServerFnError::new("Quantity must be at least 1"));
    }

    let ac = AuthenticatedContext::extract().await?;

    // AI bundle purchases are not available in managed (SaaS) mode — AI is included.
    if !ac.ctx.config.self_hosted {
        return Err(ServerFnError::new("AI bundle purchases are not available. AI is included in your plan."));
    }

    require_workspace_owner(&ac.auth)?;

    let stripe_service = require_stripe(&ac.ctx.config)?;
    let workspace = load_workspace(ac.db(), &ac.ws_id).await?;

    let customer_id = workspace.stripe_customer_id.as_deref().ok_or_else(|| {
        ServerFnError::new("No Stripe customer found. Please subscribe to a plan first.")
    })?;

    let price_id =
        kyomi_auth::stripe_config::get_ai_bundle_price_id().ok_or_else(|| {
            ServerFnError::new("STRIPE_AI_BUNDLE not configured")
        })?;

    let params = kyomi_auth::stripe_service::EmbeddedPaymentCheckoutParams {
        customer_id: customer_id.to_string(),
        price_id: price_id.to_string(),
        workspace_id: ac.ws_id.clone(),
        purchase_type: "ai_bundle".to_string(),
        quantity: u64::from(quantity),
    };

    let result = stripe_service
        .create_embedded_payment_checkout_session(&params)
        .await
        .map_err(|e| ServerFnError::new(format!("Failed to create AI bundle checkout: {e}")))?;

    Ok(EmbeddedCheckoutSession {
        client_secret: result.client_secret,
        session_id: result.session_id,
    })
}

/// Purchase an analytics event bundle via embedded Stripe checkout.
///
/// Returns an `EmbeddedCheckoutSession` with the client_secret to mount
/// the Stripe form inline. The webhook handler credits the workspace's
/// `analytics_bundle_events` upon successful payment.
#[server(prefix = "/leptos-api", client = crate::server_fns::paywall_client::PaywallAwareClient)]
pub async fn purchase_analytics_bundle(quantity: u32) -> Result<EmbeddedCheckoutSession, ServerFnError> {
    if quantity < 1 {
        return Err(ServerFnError::new("Quantity must be at least 1"));
    }

    let ac = AuthenticatedContext::extract().await?;
    require_workspace_owner(&ac.auth)?;

    let stripe_service = require_stripe(&ac.ctx.config)?;
    let workspace = load_workspace(ac.db(), &ac.ws_id).await?;

    let customer_id = workspace.stripe_customer_id.as_deref().ok_or_else(|| {
        ServerFnError::new("No Stripe customer found. Please subscribe to a plan first.")
    })?;

    let price_id = kyomi_auth::stripe_config::get_analytics_bundle_price_id()
        .ok_or_else(|| {
            ServerFnError::new("STRIPE_ANALYTICS_BUNDLE not configured")
        })?;

    let params = kyomi_auth::stripe_service::EmbeddedPaymentCheckoutParams {
        customer_id: customer_id.to_string(),
        price_id: price_id.to_string(),
        workspace_id: ac.ws_id.clone(),
        purchase_type: "analytics_bundle".to_string(),
        quantity: u64::from(quantity),
    };

    let result = stripe_service
        .create_embedded_payment_checkout_session(&params)
        .await
        .map_err(|e| {
            ServerFnError::new(format!(
                "Failed to create analytics bundle checkout: {e}"
            ))
        })?;

    Ok(EmbeddedCheckoutSession {
        client_secret: result.client_secret,
        session_id: result.session_id,
    })
}

/// Get the Stripe publishable key (needed for embedded checkout on the frontend).
///
/// Publishable keys are designed to be public — this is not a secret.
///
/// Allowlisted while billing is lapsed (KYO-805) — needed to mount the
/// embedded checkout that pays to un-lapse the workspace.
#[server(prefix = "/leptos-api", client = crate::server_fns::paywall_client::PaywallAwareClient)]
pub async fn get_stripe_publishable_key() -> Result<Option<String>, ServerFnError> {
    let _auth = extract_auth_allow_lapsed().await?;
    let ctx = extract_context()?;
    Ok(ctx.config.stripe_publishable_key.clone())
}

/// Check the status of a checkout session (for verifying completion).
///
/// Called by the embedded checkout `onComplete` callback to confirm
/// the session actually completed before showing success UI.
///
/// Allowlisted while billing is lapsed (KYO-805) — this fires at the tail
/// end of the exact checkout flow that un-lapses the workspace, so the
/// workspace is very possibly still lapsed (webhook not yet processed) at
/// the moment this is called.
#[server(prefix = "/leptos-api", client = crate::server_fns::paywall_client::PaywallAwareClient)]
pub async fn get_checkout_session_status(
    session_id: String,
) -> Result<CheckoutStatus, ServerFnError> {
    let _auth = extract_auth_allow_lapsed().await?;
    let ctx = extract_context()?;
    let stripe_service = require_stripe(&ctx.config)?;

    let status = stripe_service
        .retrieve_checkout_session_status(&session_id)
        .await
        .map_err(|e| ServerFnError::new(format!("Failed to retrieve session status: {e}")))?;

    Ok(CheckoutStatus {
        status: status.status,
        payment_status: status.payment_status,
    })
}

#[cfg(all(test, feature = "ssr"))]
mod tests {
    use super::*;

    #[test]
    fn parse_pg_or_rfc3339_accepts_postgres_text_cast() {
        let parsed = parse_pg_or_rfc3339("2026-05-12 07:39:06+00").expect("parse");
        assert_eq!(parsed.to_rfc3339(), "2026-05-12T07:39:06+00:00");
    }

    #[test]
    fn parse_pg_or_rfc3339_accepts_postgres_text_with_fractional_seconds() {
        let parsed = parse_pg_or_rfc3339("2026-05-12 07:39:04.698546+00").expect("parse");
        assert_eq!(parsed.to_rfc3339(), "2026-05-12T07:39:04.698546+00:00");
    }

    #[test]
    fn parse_pg_or_rfc3339_accepts_rfc3339() {
        let parsed = parse_pg_or_rfc3339("2026-05-12T07:39:06+00:00").expect("parse");
        assert_eq!(parsed.to_rfc3339(), "2026-05-12T07:39:06+00:00");
    }

    #[test]
    fn parse_pg_or_rfc3339_accepts_postgres_text_with_colon_offset() {
        // Some Postgres client configurations emit `+00:00` instead of `+00`;
        // `%#z` accepts both variants.
        let parsed =
            parse_pg_or_rfc3339("2026-05-12 07:39:04.698546+00:00").expect("parse");
        assert_eq!(parsed.to_rfc3339(), "2026-05-12T07:39:04.698546+00:00");
    }

    #[test]
    fn parse_pg_or_rfc3339_rejects_garbage() {
        assert!(parse_pg_or_rfc3339("not a date").is_none());
    }
}
