// SPDX-License-Identifier: AGPL-3.0-or-later

//! The billing-gate wire contract shared by every layer that can reject a
//! request because a SaaS workspace's billing has lapsed (KYO-805).
//!
//! `kyomi-ui` compiles to `wasm32` and can only depend unconditionally on
//! `kyomi-types` (`kyomi-core` and `kyomi-auth` are `ssr`-gated optional
//! dependencies, unavailable to the WASM client) — see
//! `docs/standards/string-text-processing/shared-types-belong-in-kyomi-types.md`.
//! [`PAYMENT_REQUIRED_CODE`] is the one definition of the machine-readable
//! `"error"` value a lapsed-workspace response carries, so the client-side
//! paywall (KYO-806) and every server-side producer of that value read the
//! same constant instead of independently-maintained copies.

use serde::{Deserialize, Serialize};

/// Machine-readable code for a request refused because the caller's
/// workspace billing is lapsed. Every producer of this value — the REST/
/// server-fn JSON body's `"error"` field (`kyomi_core::Error::PaymentRequired`'s
/// `IntoResponse`, re-exported as `kyomi_core::PAYMENT_REQUIRED_CODE`), the
/// server-fn error message contract (`kyomi_ui::server_fns::PAYMENT_REQUIRED_MARKER`),
/// and the WebSocket sync handlers' `send_error` `error_code`
/// (`apps/server/src/routes/websocket.rs`) — re-exports this constant rather
/// than redefining the literal, so they cannot drift apart (KYO-805).
pub const PAYMENT_REQUIRED_CODE: &str = "payment_required";

/// Why a SaaS workspace's billing is currently lapsed (KYO-806).
///
/// This is the typed, wire-shared reason behind `is_billing_lapsed` /
/// `billing_lapse_reason` in `kyomi_core::capability` — the paywall (client
/// side, built in a later phase of KYO-806) renders copy keyed on this enum
/// rather than string-matching `subscription_status`, which is why it lives
/// here rather than in `kyomi-core`: `kyomi-ui` compiles to `wasm32` and can
/// only depend unconditionally on `kyomi-types` (`kyomi-core` is an
/// `ssr`-gated optional dependency there) — see
/// `docs/standards/string-text-processing/shared-types-belong-in-kyomi-types.md`.
///
/// Variants mirror `kyomi_core::capability::is_billing_lapsed`'s three
/// lapsed cases exactly — see that function's doc comment for the full
/// reasoning behind each one:
/// - [`Self::PaymentFailed`] — `PastDue`: Stripe has already determined this
///   workspace isn't paid up.
/// - [`Self::SubscriptionEnded`] — `Cancelled` and not a scheduled
///   cancellation still inside its paid-up grace period.
/// - [`Self::TrialEnded`] — `Trialing` with no live Stripe subscription and
///   `trial_ends_at` in the past (the app-managed fallback trial).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BillingLapseReason {
    /// The app-managed fallback trial (no Stripe subscription) expired.
    TrialEnded,
    /// Stripe reports the subscription `past_due` — the last payment attempt failed.
    PaymentFailed,
    /// The subscription was cancelled and any scheduled-cancellation grace
    /// period (paid-up time remaining through `subscription_period_end`)
    /// has ended or never applied.
    SubscriptionEnded,
}
