// SPDX-License-Identifier: AGPL-3.0-or-later

//! Server functions — typed RPC that replaces REST API calls.
//!
//! Each function marked with `#[server]` runs on the server and is callable
//! from WASM client code. The Leptos runtime handles serialization,
//! transport, and error mapping automatically.
//!
//! ## Context Pattern
//!
//! Server functions get `DbPool`, `Config`, and `AuthState` from Leptos context
//! (provided at router setup). This avoids a circular dependency with the
//! server crate's `AppState`.

pub mod profile;

/// State provided to server functions via Leptos context.
///
/// Set up in the server's router configuration. This breaks the circular
/// dependency: kyomi-ui doesn't know about AppState, but gets the pieces
/// it needs via context.
#[cfg(feature = "ssr")]
#[derive(Clone)]
pub struct ServerContext {
    pub db: kyomi_core::DbPool,
    pub config: std::sync::Arc<kyomi_core::Config>,
    pub auth_state: kyomi_auth::middleware::AuthState,
}
