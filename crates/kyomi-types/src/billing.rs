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

/// Machine-readable code for a request refused because the caller's
/// workspace billing is lapsed. Every producer of this value — the REST/
/// server-fn JSON body's `"error"` field (`kyomi_core::Error::PaymentRequired`'s
/// `IntoResponse`, re-exported as `kyomi_core::PAYMENT_REQUIRED_CODE`), the
/// server-fn error message contract (`kyomi_ui::server_fns::PAYMENT_REQUIRED_MARKER`),
/// and the WebSocket sync handlers' `send_error` `error_code`
/// (`apps/server/src/routes/websocket.rs`) — re-exports this constant rather
/// than redefining the literal, so they cannot drift apart (KYO-805).
pub const PAYMENT_REQUIRED_CODE: &str = "payment_required";
