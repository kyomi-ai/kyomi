// SPDX-License-Identifier: AGPL-3.0-or-later

//! Central "billing was just refused" signal (KYO-806).
//!
//! Three very different kinds of code need to react the instant a request
//! comes back `402 payment_required`:
//! - A `#[server]` fn call, intercepted by
//!   `crate::server_fns::paywall_client::PaywallAwareClient` — every
//!   `#[server(...)]` in this crate names it as its `client`.
//! - A raw `fetch`/`Request` call that bypasses the server_fn machinery
//!   entirely (`utils/arrow_fetch.rs`, `chartml_provider.rs`, and any page
//!   that talks to `/api/...` directly).
//! - The sync engine's WebSocket, when a `sync_bootstrap`/`sync_delta`
//!   refusal carries `error_code == kyomi_types::PAYMENT_REQUIRED_CODE`.
//!
//! All three call [`report_payment_required`] — the ONE function that flips
//! the app into the paywall. None of them re-implements "lock the UI and
//! refetch" independently.
//!
//! This module is split into two halves on purpose:
//! - **Pure, target-independent decision functions**
//!   ([`is_payment_required_status`], [`is_payment_required_error_code`],
//!   [`should_show_paywall`]) — no `cfg`, unit-tested the normal way
//!   (`cargo test -p kyomi-ui --features ssr`). These are what a reviewer
//!   should read to understand the actual rules.
//! - **`wasm32`-only global plumbing** (the `thread_local!` block below) —
//!   the mechanism that carries those decisions' *inputs* from callbacks
//!   that run outside any reactive owner into `Layout`'s reactive graph.
//!   Not unit-testable (there is no wasm32 host to run `cargo test`
//!   against in this repo's toolchain; it's exercised by the Playwright
//!   script instead), so it is kept as thin as possible — no decisions live
//!   here, only plumbing.
//!
//! ## Why the plumbing is a `thread_local!`, not Leptos context
//!
//! Every caller of [`report_payment_required`] runs **outside any reactive
//! owner**: the server_fn client's `send` is a trait method server_fn's
//! generated code calls directly, the raw fetch helpers are plain `async
//! fn`s, and the WebSocket error handler runs inside a `ws.subscribe`
//! callback that isn't necessarily nested under the component that mounted
//! the connection. `leptos::prelude::use_context` needs a live reactive
//! owner on the call stack — none of these have one — so a Leptos
//! `provide_context`/`use_context` pair cannot be the plumbing here. A
//! plain `thread_local!` global is reachable from anywhere, which is
//! exactly what's needed.
//!
//! `wasm32-unknown-unknown` has no real threads (no code in this crate
//! spawns one), so a `thread_local!` here behaves as a single process-wide
//! global for the lifetime of the page — safe in a way it would NOT be on
//! the SSR side, where one server process handles many concurrent requests
//! from different users. That is why the `thread_local!` block and
//! everything that touches it is `#[cfg(target_arch = "wasm32")]`-gated: an
//! SSR build must never get a global that leaks billing state across
//! requests. [`report_payment_required`] and [`refetch_billing_state`]
//! themselves stay callable unconditionally (`PaywallAwareClient` must
//! compile under both `ssr` and `hydrate` — see its doc comment) but no-op
//! outside `wasm32`.

// `ArcRwSignal`/`WriteSignal` (plus the `Get`/`Set`/`Update` traits they need)
// are only referenced by the `wasm32`-only plumbing below — importing them
// unconditionally warns "unused" on the ssr/native build, where none of that
// plumbing compiles in.
#[cfg(target_arch = "wasm32")]
use leptos::prelude::*;

// ─── Pure decision functions — unit-tested, no cfg ─────────────────────────

/// Whether an HTTP response status is the billing-lapsed refusal KYO-805
/// defined (`kyomi_core::Error::PaymentRequired` → HTTP 402). The single
/// check [`crate::server_fns::paywall_client::PaywallAwareClient`] and the
/// raw-fetch 402 helper both call, so "is this a payment-required response"
/// is decided in exactly one place.
pub fn is_payment_required_status(status: u16) -> bool {
    status == 402
}

/// Whether a WebSocket `error` message's `error_code` is the billing-lapsed
/// refusal (`kyomi_types::PAYMENT_REQUIRED_CODE`), as opposed to `None` (the
/// `Unverifiable` case — workspace lookup failed, DB down, etc. — which must
/// NOT flip the app into the paywall) or any other error code.
pub fn is_payment_required_error_code(error_code: Option<&str>) -> bool {
    error_code == Some(kyomi_types::PAYMENT_REQUIRED_CODE)
}

/// Whether a `billing_status_changed` WebSocket event (KYO-807) is for the
/// sync engine's own workspace.
///
/// The event carries no status — the client always refetches — but a
/// WebSocket connection is scoped per-*user*, not per-workspace
/// (`crates/kyomi-auth/src/websocket/manager.rs`), so a user who belongs to
/// two workspaces receives both workspaces' `billing_status_changed` events
/// on the same socket and must ignore the one that isn't this tab's.
///
/// `event_workspace_id` is `None` when the payload's `workspace_id` key is
/// missing or not a string — should never happen, since the emitter
/// (`kyomi_auth::websocket::helpers::broadcast_billing_status_changed`)
/// always sets it, but a malformed payload must not be treated as a match.
pub fn is_billing_status_change_for_workspace(
    event_workspace_id: Option<&str>,
    my_workspace_id: &str,
) -> bool {
    event_workspace_id == Some(my_workspace_id)
}

/// Whether `Layout` should render the full-screen paywall instead of the
/// sidebar + page content (KYO-806).
///
/// - `is_saas` — SaaS mode only (not self-hosted, not personal); those
///   deployments have no billing at all and must never show the paywall,
///   regardless of `server_lapsed`/`optimistic_lapsed` (which are always
///   `false` there in practice, since `billing_gate_blocks` already encodes
///   this — `is_saas` is checked again here defensively, matching
///   `get_billing_paywall`'s server-side gate on the same predicate, so a
///   client-side bug in threading `is_saas` through can never show the
///   paywall to a self-hosted/personal user even if the optimistic flag
///   somehow got set).
/// - `server_lapsed` — the authoritative value:
///   `SidebarUser`/`UserContext.billing_lapsed`, computed server-side by
///   `kyomi_core::capability::is_billing_lapsed`.
/// - `optimistic_lapsed` — the client-side flag [`report_payment_required`]
///   sets immediately on a 402/`payment_required`, before the authoritative
///   refetch it also kicks off has resolved. ORed in so an already-open tab
///   locks into the paywall on this tick rather than waiting a round-trip.
pub fn should_show_paywall(is_saas: bool, server_lapsed: bool, optimistic_lapsed: bool) -> bool {
    is_saas && (server_lapsed || optimistic_lapsed)
}

// ─── wasm32-only plumbing ───────────────────────────────────────────────────

#[cfg(target_arch = "wasm32")]
thread_local! {
    /// Optimistic "the app just got refused for payment" flag — see
    /// [`should_show_paywall`]'s `optimistic_lapsed` parameter. Cleared once
    /// a [`refetch_billing_state`] round-trip resolves and the server's own
    /// `billing_lapsed` becomes the authority again
    /// (`crate::components::layout::Layout`'s effect on `user_info`).
    ///
    /// `ArcRwSignal`, not `RwSignal`: an `RwSignal` is arena-allocated and
    /// owned by whichever reactive owner is live the first time this
    /// `thread_local!` is lazily initialized — in practice, `Layout`'s own
    /// `show_paywall` `Memo`. `Layout` does NOT remount on every in-app
    /// navigation — `app.rs` mounts every authenticated route under one
    /// `<ParentRoute>` wrapping a single `<Layout><Outlet/></Layout>`, so
    /// one instance persists across those transitions. It DOES unmount
    /// whenever the user leaves that shell for a route outside the
    /// `ParentRoute` — `/login`, `/onboarding`, `/billing/return`,
    /// `/accept-invite/:id`, and the rest of `app.rs`'s standalone routes —
    /// and a *different* `Layout` instance (`app.rs`'s `<Routes
    /// fallback=|| <Layout><NotFoundPage/></Layout>>`, or a fresh
    /// `ParentRoute` mount on the way back in) is what would touch this
    /// `thread_local!` next, arena-disposed from the one that touched it
    /// first. `ArcRwSignal` is reference-counted instead of arena-bound, so
    /// it keeps living for as long as this `thread_local!` (i.e. the page)
    /// holds a clone of it, independent of which `Layout` instance happens
    /// to touch it first.
    static OPTIMISTIC_LAPSED: ArcRwSignal<bool> = ArcRwSignal::new(false);

    /// The two Layout-owned trigger signals [`refetch_billing_state`] bumps.
    /// Registered once by `Layout` via [`register_refetch_triggers`] on
    /// mount, and cleared by [`unregister_refetch_triggers`] in `Layout`'s
    /// `on_cleanup` so that when this `Layout` instance unmounts (leaving
    /// the authed shell for `/login`, `/onboarding`, `/billing/return`, etc.
    /// — see the note on `OPTIMISTIC_LAPSED` above for which routes those
    /// are), the next `Layout` instance to mount never bumps this one's now-
    /// disposed `WriteSignal`s. `None` until Layout has mounted — every
    /// caller of [`refetch_billing_state`] tolerates that (there is nothing
    /// to refetch before the app shell exists, or in the gap between one
    /// Layout instance unmounting and the next registering its own).
    ///
    /// Kept as plain arena `WriteSignal`s (not `ArcRwSignal`, unlike
    /// `OPTIMISTIC_LAPSED` above): these are always freshly re-registered by
    /// whichever `Layout` instance is currently mounted, so there is no
    /// "outlive the owner" requirement here — the requirement is the
    /// opposite, that a stale registration NOT outlive its owner, which
    /// `unregister_refetch_triggers` plus [`refetch_billing_state`]'s
    /// `try_update` (tolerating a disposed signal instead of panicking)
    /// both guard against.
    static REFETCH_TRIGGERS: std::cell::Cell<Option<(WriteSignal<u32>, WriteSignal<u32>)>> =
        const { std::cell::Cell::new(None) };
}

/// Reactive read of the optimistic lapsed flag — `wasm32` only, since it's
/// only meaningful (and only safe; see the module doc) on the client.
/// `crate::components::layout::Layout` reads this (via `.try_get()`,
/// mirroring `show_paywall`'s other read of `user_info`) as
/// `should_show_paywall`'s `optimistic_lapsed` argument. Returns a clone of
/// the underlying [`ArcRwSignal`] — cheap (a reference-count bump), and
/// never disposed (see the `thread_local!` block's doc comment), so
/// `try_get` here is belt-and-braces for `scripts/lint/check-disposal-
/// safety.sh`'s Rule B (a bare `.get()` inside `Memo`/`Signal::derive` is
/// flagged regardless of whether the specific signal can actually be
/// disposed), not because this particular signal is ever expected to be.
#[cfg(target_arch = "wasm32")]
pub fn optimistic_lapsed() -> ArcRwSignal<bool> {
    OPTIMISTIC_LAPSED.with(|s| s.clone())
}

/// Clear the optimistic flag — called once a [`refetch_billing_state`]
/// round-trip resolves and the workspace is confirmed no longer lapsed.
/// Never called while still lapsed: leaving the optimistic flag set in that
/// case is harmless (the server flag alone already keeps the paywall up)
/// and this function is only ever invoked from the "no longer lapsed"
/// branch.
///
/// Callable from both `ssr` and `hydrate` builds (component render logic —
/// e.g. `components::billing_paywall::BillingPaywall` — compiles under
/// both); a no-op outside `wasm32` (see the module doc for why the SSR side
/// must never carry this global).
pub fn clear_optimistic_lapsed() {
    #[cfg(target_arch = "wasm32")]
    OPTIMISTIC_LAPSED.with(|s| s.set(false));
}

/// Register the two trigger signals [`refetch_billing_state`] bumps.
///
/// Called once by `Layout` right after it creates `set_auth_retry` (refetches
/// `get_sidebar_user`) and `set_user_ctx_version` (refetches
/// `get_user_context`) — the same two signals Layout's own token-refresh
/// path already reuses, per KYO-806's "reuse, don't duplicate" requirement.
/// `wasm32` only: Layout's SSR render never needs this wiring (effects don't
/// run during SSR).
///
/// Paired with [`unregister_refetch_triggers`], which `Layout` calls from
/// `on_cleanup` — see the `REFETCH_TRIGGERS` `thread_local!`'s doc comment
/// for why that pairing matters.
#[cfg(target_arch = "wasm32")]
pub fn register_refetch_triggers(auth_retry: WriteSignal<u32>, user_ctx_version: WriteSignal<u32>) {
    REFETCH_TRIGGERS.with(|c| c.set(Some((auth_retry, user_ctx_version))));
}

/// Clear the registration [`register_refetch_triggers`] installed. Called by
/// `Layout` from `on_cleanup` so that, between this `Layout` instance
/// unmounting (disposing its `WriteSignal`s — see `OPTIMISTIC_LAPSED`'s doc
/// comment for when that happens) and the next `Layout` instance
/// registering its own, [`refetch_billing_state`] finds nothing to bump
/// rather than a stale, disposed pair. Belt-and-braces alongside
/// [`refetch_billing_state`]'s own `try_update` — either guard alone would
/// prevent a panic, but only this one closes the window where a bump would
/// otherwise silently target signals from a `Layout` instance that no
/// longer exists.
#[cfg(target_arch = "wasm32")]
pub fn unregister_refetch_triggers() {
    REFETCH_TRIGGERS.with(|c| c.set(None));
}

/// The ONE entry point that reacts to "billing was just refused" (KYO-806).
///
/// Call this — and nothing else — from every 402/`payment_required`
/// interception point:
/// - [`crate::server_fns::paywall_client::PaywallAwareClient::send`], guarded
///   by [`is_payment_required_status`]
/// - the shared raw-fetch 402 helper (`utils/arrow_fetch.rs` and friends),
///   guarded the same way
/// - the sync engine's WebSocket `error` handler, guarded by
///   [`is_payment_required_error_code`]
///
/// Sets the optimistic flag immediately (so the paywall can render on this
/// same tick, before any network round-trip completes) and kicks off
/// [`refetch_billing_state`] so the server's own `billing_lapsed` — the
/// actual authority — catches up.
///
/// Callable from both `ssr` and `hydrate` builds (`PaywallAwareClient`
/// compiles under both); a no-op outside `wasm32` (see the module doc for
/// why the SSR side must never carry this global).
pub fn report_payment_required() {
    #[cfg(target_arch = "wasm32")]
    {
        OPTIMISTIC_LAPSED.with(|s| s.set(true));
        refetch_billing_state();
    }
}

/// The shared "raw `fetch` got a response — was it a payment-required
/// refusal?" check (KYO-806).
///
/// Every raw-fetch call site in this crate that talks to the Kyomi backend
/// directly instead of through a `#[server]` fn (`utils/arrow_fetch.rs`'s
/// `post_arrow_request`, `pages/dashboards/dashboard_viewer.rs`'s PDF
/// export) calls this once with the response status, immediately after the
/// `fetch` resolves, instead of re-checking `status == 402` itself — the
/// single shared helper the ticket calls for, rather than N copies of the
/// same `if status == 402 { report_payment_required() }`.
///
/// Deliberately NOT called from every `web_sys::Request`/`fetch` call site
/// in this crate — two are excluded on purpose:
/// - `panic_overlay.rs`'s crash-report submission drives its `Promise`
///   without `JsFuture`/the normal executor specifically because a panic
///   severe enough to show that overlay may have already poisoned it;
///   reaching into Leptos reactive state from there is exactly the kind of
///   thing that overlay exists to avoid depending on.
/// - `connect_setup_page.rs`'s CLI token delivery calls
///   `http://127.0.0.1:<port>/...` — the user's own local CLI process, never
///   the Kyomi backend, so it can never receive this app's 402.
#[cfg(target_arch = "wasm32")]
pub fn check_rest_response_status(status: u16) {
    if is_payment_required_status(status) {
        report_payment_required();
    }
}

/// Force `Layout` to refetch the two resources billing state depends on:
/// `get_sidebar_user` (via the existing `auth_retry` trigger) and
/// `get_user_context` (via the existing `user_ctx_version` trigger). A no-op
/// until `Layout` has called [`register_refetch_triggers`] — there is
/// nothing to refetch before the app shell mounts.
///
/// Public (not just called from [`report_payment_required`]) because
/// `crate::cache::sync_engine`'s `billing_status_changed` subscription
/// (KYO-807) calls this directly — once the server pushes a billing-status
/// change over an already-open WebSocket (see
/// `is_billing_status_change_for_workspace`, which filters the event to this
/// tab's own workspace first) — rather than re-deriving its own refetch
/// plumbing.
pub fn refetch_billing_state() {
    #[cfg(target_arch = "wasm32")]
    {
        if let Some((auth_retry, user_ctx_version)) = REFETCH_TRIGGERS.with(|c| c.get()) {
            // `try_update`, not `update`: these `WriteSignal`s belong to
            // whichever `Layout` instance registered them, and `Layout` does
            // unmount and remount when the user crosses in/out of the authed
            // shell (see `OPTIMISTIC_LAPSED`'s doc comment for which routes
            // trigger that). `unregister_refetch_triggers` (called from
            // `Layout`'s `on_cleanup`) closes most of that window, but
            // `try_update` is the guard that actually prevents a panic if a
            // call lands in the gap — it silently no-ops against
            // a disposed signal instead.
            auth_retry.try_update(|n| *n = n.wrapping_add(1));
            user_ctx_version.try_update(|n| *n = n.wrapping_add(1));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_402_is_payment_required() {
        assert!(is_payment_required_status(402));
    }

    #[test]
    fn other_statuses_are_not_payment_required() {
        for status in [200, 400, 401, 403, 404, 409, 500] {
            assert!(!is_payment_required_status(status));
        }
    }

    #[test]
    fn matching_ws_error_code_is_payment_required() {
        assert!(is_payment_required_error_code(Some(
            kyomi_types::PAYMENT_REQUIRED_CODE
        )));
    }

    #[test]
    fn no_error_code_is_not_payment_required() {
        // The Unverifiable refusal carries no error_code — must never flip
        // the app into the paywall.
        assert!(!is_payment_required_error_code(None));
    }

    #[test]
    fn different_error_code_is_not_payment_required() {
        assert!(!is_payment_required_error_code(Some("unauthorized")));
    }

    #[test]
    fn paywall_shows_for_saas_workspace_with_server_lapsed() {
        assert!(should_show_paywall(true, true, false));
    }

    #[test]
    fn paywall_shows_for_saas_workspace_with_only_optimistic_flag() {
        // Set immediately on a 402, before the authoritative refetch resolves.
        assert!(should_show_paywall(true, false, true));
    }

    #[test]
    fn paywall_never_shows_outside_saas_even_if_both_flags_are_set() {
        // Defensive: self-hosted/personal mode must never show the paywall,
        // even if the optimistic flag somehow got set.
        assert!(!should_show_paywall(false, true, true));
    }

    #[test]
    fn paywall_does_not_show_for_active_saas_workspace() {
        assert!(!should_show_paywall(true, false, false));
    }

    #[test]
    fn billing_status_change_for_own_workspace_matches() {
        assert!(is_billing_status_change_for_workspace(Some("ws-1"), "ws-1"));
    }

    #[test]
    fn billing_status_change_for_a_different_workspace_does_not_match() {
        // A user in two workspaces receives both workspaces' events on the
        // same per-user WebSocket connection — must ignore the other one.
        assert!(!is_billing_status_change_for_workspace(Some("ws-2"), "ws-1"));
    }

    #[test]
    fn billing_status_change_with_missing_workspace_id_does_not_match() {
        // A malformed/missing payload key must never be treated as a match.
        assert!(!is_billing_status_change_for_workspace(None, "ws-1"));
    }
}
