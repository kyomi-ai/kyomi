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

pub mod context;
pub mod profile;
pub mod sidebar;
#[cfg(feature = "slack")]
pub mod slack;

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

    /// Encryption key for decrypting stored tokens (e.g. Slack bot tokens).
    /// Required by Slack server functions; `None` disables those code paths.
    pub encryption_key: Option<std::sync::Arc<[u8; 32]>>,

    /// Key-value store for OAuth state tokens and ephemeral data.
    /// Required by Slack connect flow; `None` disables OAuth URL generation.
    pub kv: Option<kyomi_core::KVPool>,

    /// Slack HTTP client for Slack Web API calls (channel listing, etc.).
    /// Present only when the `slack` feature is enabled and Slack is configured.
    #[cfg(feature = "slack")]
    pub slack_client: Option<kyomi_slack::client::SlackClient>,
}

/// Extract the authenticated user from the Axum request.
#[cfg(feature = "ssr")]
pub(crate) async fn extract_auth() -> Result<kyomi_auth::middleware::AuthUser, leptos::prelude::ServerFnError> {
    let ctx = extract_context()?;
    leptos_axum::extract_with_state::<kyomi_auth::middleware::AuthUser, _>(&ctx.auth_state)
        .await
        .map_err(|e| leptos::prelude::ServerFnError::new(format!("Authentication required: {e}")))
}

/// Extract the server context (db, config, auth_state) from Leptos context.
#[cfg(feature = "ssr")]
pub(crate) fn extract_context() -> Result<ServerContext, leptos::prelude::ServerFnError> {
    leptos::prelude::use_context::<ServerContext>()
        .ok_or_else(|| leptos::prelude::ServerFnError::new("Server context not available"))
}

/// Get workspace_id from the auth user, or error.
#[cfg(feature = "ssr")]
pub(crate) fn workspace_id(auth: &kyomi_auth::middleware::AuthUser) -> Result<&str, leptos::prelude::ServerFnError> {
    auth.workspace
        .workspace_id
        .as_deref()
        .ok_or_else(|| leptos::prelude::ServerFnError::new("Workspace context required"))
}
