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

pub mod ai;
pub mod analytics;
pub mod auth;
pub mod billing;
pub mod chat;
pub mod collections;
pub mod connect;
pub mod copilot;
pub mod context;
pub mod dashboards;
pub mod datasource_oauth;
pub mod datasources;
pub mod feedback;
pub mod home;
pub mod knowledge;
pub mod onboarding;
pub mod ownership;
pub mod profile;
pub(crate) mod provider_cache;
pub mod security;
pub mod setup;
pub mod sidebar;
pub mod slack;
pub mod sql_editor;
pub mod team;
pub mod unsubscribe;
pub mod usage;
pub mod watches;
pub mod workspace;

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

    /// Raw Redis connection pool used for components that need direct Redis
    /// access from server functions (e.g. Connect agent presence checks).
    /// `None` when running without Redis (single-instance mode) — callers
    /// must treat this as "feature unavailable" and respond accordingly.
    pub redis: Option<kyomi_core::RedisPool>,

    /// WebAuthn instance for passkey registration/authentication.
    /// Built once at server startup from config.
    pub webauthn: Option<std::sync::Arc<webauthn_rs::Webauthn>>,

    /// Lazy-loaded embedding model for knowledge graph operations.
    /// Required by workspace admin populate-graph server function.
    pub embedding: kyomi_embed::LazyEmbedding,

    /// Connect registry for routing queries to Kyomi Connect instances.
    /// Required for query execution against Connect-type datasources.
    pub connect_registry: Option<kyomi_datasource_server::ConnectRegistry>,

    /// WebSocket manager for streaming AI responses, real-time events,
    /// streaming query results, and agent response delivery.
    pub ws_manager: Option<kyomi_auth::websocket::WebSocketManager>,

    /// Registry for cancelling in-flight agent tasks via WebSocket `cancel_request`.
    /// Uses the same DashMap<(user_id, session_id), CancellationToken> pattern as
    /// `apps/server/src/cancel_registry.rs`. Optional — agent execution is skipped
    /// when not provided.
    pub cancel_registry: Option<CancelRegistry>,

    /// Platform registry for messaging integrations (Slack, Teams, etc.).
    /// Required by agent execution for platform-aware tool dispatch.
    pub platforms: Option<std::sync::Arc<kyomi_core::platform::PlatformRegistry>>,

    /// Connect token service for generating Kyomi Connect JWT tokens.
    /// Required by Connect Setup server functions.
    pub connect_token: Option<std::sync::Arc<kyomi_auth::connect_token::ConnectTokenService>>,

    /// MCP Streamable HTTP session manager for billing-tier-driven
    /// tool capability invalidation. Required by the Leptos billing
    /// server_fn. `None` disables MCP invalidation on the Leptos path
    /// (acceptable only when MCP sessions aren't in use, e.g. tests).
    pub mcp_sessions: Option<kyomi_auth::mcp_session_manager::MCPSessionManager>,

    /// Slack HTTP client for Slack Web API calls (channel listing, etc.).
    /// Present only when the `slack` feature is enabled and Slack is configured.
    #[cfg(feature = "slack")]
    pub slack_client: Option<kyomi_slack::client::SlackClient>,
}

#[cfg(feature = "ssr")]
pub use kyomi_core::cancel_registry::CancelRegistry;

/// Extract the authenticated user from the Axum request.
///
/// Returns an `Err` when no auth is present AND sets the HTTP response
/// status to 401 Unauthorized via `ResponseOptions`. Without the status
/// override the default `ServerFnError::ServerError` serializes as a
/// 500 Internal Server Error, which triggers `tower_http::trace`'s
/// on-failure classification and spams both server logs and the browser
/// console with spurious 5xx entries on every unauthenticated page load
/// (e.g. anonymous visits to `/login`). Auth failure is a client error,
/// not a server error — 401 is the correct classification.
#[cfg(feature = "ssr")]
pub(crate) async fn extract_auth() -> Result<kyomi_auth::middleware::AuthUser, leptos::prelude::ServerFnError> {
    let ctx = extract_context()?;
    match leptos_axum::extract_with_state::<kyomi_auth::middleware::AuthUser, _>(&ctx.auth_state).await {
        Ok(auth) => Ok(auth),
        Err(e) => {
            // Flag the response as 401 so tower_http and the browser don't
            // classify this as a 5xx server error. Every server fn invocation
            // has a ResponseOptions in context; matches the pattern used in
            // auth.rs / security.rs / onboarding.rs.
            leptos::prelude::expect_context::<leptos_axum::ResponseOptions>()
                .set_status(axum::http::StatusCode::UNAUTHORIZED);
            Err(leptos::prelude::ServerFnError::new(format!("Authentication required: {e}")))
        }
    }
}

/// Extract the server context (db, config, auth_state) from Leptos context.
#[cfg(feature = "ssr")]
pub(crate) fn extract_context() -> Result<ServerContext, leptos::prelude::ServerFnError> {
    leptos::prelude::use_context::<ServerContext>().ok_or_else(|| {
        tracing::error!("Server context not available");
        leptos::prelude::ServerFnError::new("Server context not available")
    })
}

/// Get workspace_id from the auth user, or error.
#[cfg(feature = "ssr")]
pub(crate) fn workspace_id(auth: &kyomi_auth::middleware::AuthUser) -> Result<&str, leptos::prelude::ServerFnError> {
    auth.workspace
        .workspace_id
        .as_deref()
        .ok_or_else(|| {
            tracing::error!("Workspace context required");
            leptos::prelude::ServerFnError::new("Workspace context required")
        })
}

/// Whether `auth` holds `permission`, per the single role→capability mapping
/// in [`kyomi_auth::permissions::permissions_for`].
///
/// Prefer [`AuthenticatedContext::has`] when an `AuthenticatedContext` is
/// already in scope; this free function exists for the rare server fn that
/// only extracts a bare `AuthUser` (e.g. `ai::test_workspace_ai_config`).
#[cfg(feature = "ssr")]
pub(crate) fn has_permission(
    auth: &kyomi_auth::middleware::AuthUser,
    permission: kyomi_types::Permission,
) -> bool {
    kyomi_auth::permissions::permissions_for(auth).contains(&permission)
}

/// Reject the request with `message` unless `auth` holds `permission`.
///
/// The single shared authorization guard for Leptos server functions. This
/// replaces the six byte-identical `require_workspace_admin` copies that
/// used to live in `team.rs`, `analytics.rs`, `datasources.rs`,
/// `workspace.rs`, `ai.rs`, and `connect.rs`, plus the inline role checks in
/// `dashboards.rs`, `onboarding.rs`, and `sql_editor.rs`. Does not set an
/// HTTP status code — the default `ServerFnError` classification applies,
/// matching every one of those call sites. For the owner-only gate that
/// also sets HTTP 403 (billing), see `billing::require_workspace_owner`,
/// which wraps the same [`kyomi_auth::permissions::permissions_for`]
/// mapping rather than duplicating it.
#[cfg(feature = "ssr")]
pub(crate) fn require_permission(
    auth: &kyomi_auth::middleware::AuthUser,
    permission: kyomi_types::Permission,
    message: &str,
) -> Result<(), leptos::prelude::ServerFnError> {
    if has_permission(auth, permission) {
        Ok(())
    } else {
        Err(leptos::prelude::ServerFnError::new(message))
    }
}

/// Bundles the three values every authenticated server function needs:
/// the authenticated user, the server context, and the resolved workspace ID.
///
/// Call `AuthenticatedContext::extract().await?` at the top of any server
/// function that requires authentication to replace the three-line boilerplate.
#[cfg(feature = "ssr")]
pub(crate) struct AuthenticatedContext {
    pub auth: kyomi_auth::middleware::AuthUser,
    pub ctx: ServerContext,
    pub ws_id: String,
}

#[cfg(feature = "ssr")]
impl AuthenticatedContext {
    pub(crate) async fn extract() -> Result<Self, leptos::prelude::ServerFnError> {
        let auth = extract_auth().await?;
        let ctx = extract_context()?;
        let ws_id = workspace_id(&auth)?.to_string();
        Ok(Self { auth, ctx, ws_id })
    }

    pub(crate) fn db(&self) -> &kyomi_core::DbPool {
        &self.ctx.db
    }

    pub(crate) fn kv(&self) -> Result<kyomi_core::KVPool, leptos::prelude::ServerFnError> {
        self.ctx.kv.clone().ok_or_else(|| {
            tracing::error!("KV store requested but not configured in ServerContext");
            leptos::prelude::ServerFnError::new("KV store not available")
        })
    }

    pub(crate) fn encryption_key(
        &self,
    ) -> Result<std::sync::Arc<[u8; 32]>, leptos::prelude::ServerFnError> {
        self.ctx.encryption_key.clone().ok_or_else(|| {
            tracing::error!("Encryption key requested but not configured in ServerContext");
            leptos::prelude::ServerFnError::new("Encryption key not configured")
        })
    }

    /// Whether the authenticated user holds `permission` in their workspace.
    /// See [`has_permission`].
    pub(crate) fn has(&self, permission: kyomi_types::Permission) -> bool {
        has_permission(&self.auth, permission)
    }

    /// Reject the request with `message` unless the authenticated user holds
    /// `permission`. See [`require_permission`].
    pub(crate) fn require(
        &self,
        permission: kyomi_types::Permission,
        message: &str,
    ) -> Result<(), leptos::prelude::ServerFnError> {
        require_permission(&self.auth, permission, message)
    }
}

/// Extension trait that converts any `Result<T, E: Display>` into a server
/// function result, replacing the boilerplate
/// `.map_err(|e| ServerFnError::new(e.to_string()))`.
#[cfg(feature = "ssr")]
pub(crate) trait IntoServerFnError<T> {
    fn into_sfn(self) -> Result<T, leptos::prelude::ServerFnError>;
}

#[cfg(feature = "ssr")]
impl<T, E: std::fmt::Display> IntoServerFnError<T> for Result<T, E> {
    fn into_sfn(self) -> Result<T, leptos::prelude::ServerFnError> {
        self.map_err(|e| {
            tracing::error!(error = %e, "server function error");
            leptos::prelude::ServerFnError::new(kyomi_core::sanitize_error(&e.to_string()))
        })
    }
}
