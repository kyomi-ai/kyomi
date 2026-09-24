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
pub mod paywall_client;
pub mod profile;
pub(crate) mod provider_cache;
pub mod security;
pub mod setup;
pub mod sidebar;
pub mod slack;
pub mod sql_editor;
pub mod team;
/// Shared test-only support for `#[server]` fns whose input must be decoded
/// with `server_fn::codec::Json` — see the module doc comment in
/// `test_support.rs` (KYO-476). Gated identically to the `mod
/// json_input_codec_tests` / `mod chart_json_codec_tests` blocks that consume
/// it, so it neither breaks the non-`ssr` build nor silently vanishes from a
/// test run that omits `--features ssr`.
#[cfg(all(test, feature = "ssr"))]
pub(crate) mod test_support;
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

/// KYO-805/KYO-806 contract: when a server fn's auth extraction fails
/// because the caller's workspace billing is lapsed, [`extract_auth`]'s
/// `ServerFnError` carries a message of the exact form
/// `"{PAYMENT_REQUIRED_MARKER}: <reason>"` — e.g.
/// `"payment_required: This workspace's billing is past due."` — so the
/// client-side paywall (KYO-806) can detect it via
/// `ServerFnError::to_string().contains(PAYMENT_REQUIRED_MARKER)` without
/// parsing prose. The HTTP status is *also* set to 402 via `ResponseOptions`
/// in the same branch, for any caller that reads the raw HTTP status instead
/// (e.g. code with access to the underlying `fetch()` `Response`) — the two
/// signals must never disagree; see [`extract_auth`]'s implementation, the
/// only place either is set.
///
/// A `pub use` re-export of `kyomi_types::PAYMENT_REQUIRED_CODE` under the
/// name this module's callers already use, rather than a second definition
/// of the literal. `kyomi-types` is a non-optional dependency of both this
/// crate and `kyomi-core` (unlike `kyomi-core` itself, which is an
/// `ssr`-only dependency here and therefore can't be re-exported from on the
/// WASM client side of the boundary) — see
/// `docs/standards/string-text-processing/shared-types-belong-in-kyomi-types.md`
/// and `kyomi_types::PAYMENT_REQUIRED_CODE`'s own doc comment. One
/// definition, no `#[cfg]` split, nothing that can drift.
pub const PAYMENT_REQUIRED_MARKER: &str = kyomi_types::PAYMENT_REQUIRED_CODE;

/// Pull the raw Axum [`Parts`](axum::http::request::Parts) out of Leptos
/// context — the same context leptos_axum itself populates before running a
/// server fn body. [`extract_auth`] and [`extract_auth_allow_lapsed`] use
/// this (rather than `leptos_axum::extract_with_state`) so they can inspect
/// the real `kyomi_core::Error` variant an extractor rejected with —
/// `extract_with_state` immediately collapses any rejection to a
/// `Debug`-formatted string, which would make distinguishing "payment
/// required" (402) from "not authenticated" (401) a matter of string-parsing
/// a debug format instead of matching an enum.
#[cfg(feature = "ssr")]
async fn extract_parts() -> Result<axum::http::request::Parts, leptos::prelude::ServerFnError> {
    leptos::prelude::use_context::<axum::http::request::Parts>().ok_or_else(|| {
        leptos::prelude::ServerFnError::new(
            "should have had Parts provided by the leptos_axum integration",
        )
    })
}

/// Extract the authenticated user from the Axum request, enforcing the
/// billing gate (KYO-805): a lapsed SaaS workspace's request is rejected
/// here, same as it would be for any other REST route taking a bare
/// `AuthUser` — see `kyomi_auth::middleware::AuthUser`'s `FromRequestParts`
/// impl, which this calls directly.
///
/// Sets the HTTP response status via `ResponseOptions` on every `Err` path:
/// 402 Payment Required for a lapsed workspace (see
/// [`PAYMENT_REQUIRED_MARKER`] for the message contract), 401 Unauthorized
/// for every other auth failure. Without the status override the default
/// `ServerFnError::ServerError` serializes as a 500 Internal Server Error,
/// which triggers `tower_http::trace`'s on-failure classification and spams
/// both server logs and the browser console with spurious 5xx entries on
/// every unauthenticated page load (e.g. anonymous visits to `/login`).
/// Neither auth failure nor payment-required is a server error — both are
/// client errors with distinct, correct status codes.
///
/// For the small allowlisted set of endpoints that must keep working while
/// billing is lapsed (login/logout, billing settings, workspace switching,
/// user/sidebar context — see the KYO-805 ticket for the full list), use
/// [`extract_auth_allow_lapsed`] instead.
#[cfg(feature = "ssr")]
pub(crate) async fn extract_auth() -> Result<kyomi_auth::middleware::AuthUser, leptos::prelude::ServerFnError> {
    use axum::extract::FromRequestParts;

    let ctx = extract_context()?;
    let mut parts = extract_parts().await?;
    match kyomi_auth::middleware::AuthUser::from_request_parts(&mut parts, &ctx.auth_state).await {
        Ok(auth) => Ok(auth),
        Err(kyomi_core::Error::PaymentRequired(msg)) => {
            leptos::prelude::expect_context::<leptos_axum::ResponseOptions>()
                .set_status(axum::http::StatusCode::PAYMENT_REQUIRED);
            Err(leptos::prelude::ServerFnError::new(format!(
                "{PAYMENT_REQUIRED_MARKER}: {msg}"
            )))
        }
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

/// Extract the authenticated user WITHOUT enforcing the billing gate
/// (KYO-805) — the explicit opt-out from [`extract_auth`]'s default-gated
/// behaviour. Use only for the small, named allowlist of endpoints that must
/// keep working while a SaaS workspace's billing is lapsed; every other
/// server fn should call [`extract_auth`] and get the gate for free.
///
/// Still rejects (401) for every ordinary auth failure — missing/expired
/// token, inactive user, revoked membership — via the same shared
/// `kyomi_auth::middleware::load_auth_user` loading path `extract_auth`
/// uses; it just never turns `billing_lapsed = true` into a rejection.
#[cfg(feature = "ssr")]
pub(crate) async fn extract_auth_allow_lapsed() -> Result<kyomi_auth::middleware::AuthUser, leptos::prelude::ServerFnError> {
    use axum::extract::FromRequestParts;

    let ctx = extract_context()?;
    let mut parts = extract_parts().await?;
    match kyomi_auth::middleware::AuthUserAllowLapsed::from_request_parts(&mut parts, &ctx.auth_state).await {
        Ok(wrapped) => Ok(wrapped.into_inner()),
        Err(e) => {
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
    /// Shared tail of [`Self::extract`] and [`Self::extract_allow_lapsed`] —
    /// the two differ only in which extractor produced `auth`, so factoring
    /// out everything after that call is the only way to guarantee they
    /// can't drift (e.g. one gaining a step the other quietly misses).
    async fn from_auth(
        auth: kyomi_auth::middleware::AuthUser,
    ) -> Result<Self, leptos::prelude::ServerFnError> {
        let ctx = extract_context()?;
        let ws_id = workspace_id(&auth)?.to_string();
        Ok(Self { auth, ctx, ws_id })
    }

    pub(crate) async fn extract() -> Result<Self, leptos::prelude::ServerFnError> {
        Self::from_auth(extract_auth().await?).await
    }

    /// Same as [`Self::extract`], but via [`extract_auth_allow_lapsed`] — the
    /// billing-gate opt-out (KYO-805). Use only for the allowlisted billing
    /// server fns that must keep working while the workspace's billing is
    /// itself lapsed (`get_subscription_info`, `create_portal_session`, ...).
    pub(crate) async fn extract_allow_lapsed() -> Result<Self, leptos::prelude::ServerFnError> {
        Self::from_auth(extract_auth_allow_lapsed().await?).await
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

/// Marker for error types that may build their `.into_sfn()` client message
/// from `Display`/`to_string()`.
///
/// `kyomi_core::Error::Display` deliberately carries a log-only variant tag
/// (`"internal: {0}"`, `"not found: {0}"`, ...) — see
/// `docs/standards/error-handling/user-message-not-display-for-user-facing-text.md`.
/// [`IntoServerFnError::into_sfn`] cannot special-case that one type: stable
/// Rust has neither specialization nor negative impls, so a blanket
/// `impl<E: Display>` and a specific `impl for kyomi_core::Error` would
/// overlap (E0119). This sealed marker is the alternative — implemented for
/// every *other* error type actually passed to `.into_sfn()` in this crate,
/// deliberately never for `kyomi_core::Error`.
///
/// `sqlx::Error` is deliberately excluded too, for the same structural
/// reason but a different underlying leak: its `Display` can carry raw
/// constraint/column/table detail straight from the driver (KYO-557; see
/// `docs/standards/error-handling/a-generic-conversion-is-a-leak-site.md`).
/// Unlike `kyomi_core::Error`, `sqlx::Error` isn't hand-constructed at call
/// sites — it also arrives via the `db_fetch_scalar!` / `db_execute!` /
/// `db_fetch_optional!` macros (`crates/kyomi-core/src/db.rs`), which return
/// `sqlx::Result<T>` directly, so a bare `.into_sfn()` after one of those
/// macros bypassed both `Error::user_message()`'s `"internal server error"`
/// fixed string (KYO-350) and `sanitize_error`'s redaction (which only
/// covers URLs/credentials/hostnames, not driver-reported schema detail).
///
/// The payoff: a `.into_sfn()` call site with `E = kyomi_core::Error` or
/// `E = sqlx::Error` is a **compile error**, not a runtime leak. If you land
/// here from one:
/// - `E` is `kyomi_core::Error` → use [`IntoServerFnErrorCore::into_sfn_core`],
///   which reads `user_message()` instead of `Display`.
/// - `E` is `sqlx::Error` → use [`IntoServerFnErrorSqlx::into_sfn_sqlx`],
///   which routes through `kyomi_core::Error::from(e).user_message()` so it
///   collapses to the same fixed `"internal server error"` string.
/// - `E` is a new, different error type → implement this trait for it below,
///   next to the other impls, once you've confirmed its `Display` has no
///   log-only prefix a user shouldn't see.
///
/// (KYO-523 — 195 call sites migrated to `into_sfn_core`; audited by
/// temporarily requiring this bound and reading off every resulting E0599.
/// KYO-557 removed the `sqlx::Error` impl the same way: 20 call sites
/// migrated to `into_sfn_sqlx`.)
#[cfg(feature = "ssr")]
pub(crate) trait NotKyomiCoreError: std::fmt::Display {}

#[cfg(feature = "ssr")]
impl NotKyomiCoreError for kyomi_auth::workspace_ai_config::WorkspaceAiConfigError {}
#[cfg(feature = "ssr")]
impl NotKyomiCoreError for kyomi_connect_protocol::Error {}

/// Extension trait that converts any `Result<T, E: Display>` (other than
/// `Result<T, kyomi_core::Error>` — see [`NotKyomiCoreError`]) into a server
/// function result, replacing the boilerplate
/// `.map_err(|e| ServerFnError::new(e.to_string()))`.
#[cfg(feature = "ssr")]
pub(crate) trait IntoServerFnError<T> {
    fn into_sfn(self) -> Result<T, leptos::prelude::ServerFnError>;
}

#[cfg(feature = "ssr")]
impl<T, E: std::fmt::Display + NotKyomiCoreError> IntoServerFnError<T> for Result<T, E> {
    fn into_sfn(self) -> Result<T, leptos::prelude::ServerFnError> {
        self.map_err(|e| {
            tracing::error!(error = %e, "server function error");
            leptos::prelude::ServerFnError::new(kyomi_core::sanitize_error(&e.to_string()))
        })
    }
}

/// Extension trait that converts `Result<T, kyomi_core::Error>` into a server
/// function result using [`kyomi_core::Error::user_message`], not `Display` —
/// see [`NotKyomiCoreError`] for why this can't just be another arm of
/// [`IntoServerFnError`]. Logging still uses the full `Display` form (`%e`),
/// so the variant tag survives in the log; only the client-facing string is
/// built from `user_message()`.
#[cfg(feature = "ssr")]
pub(crate) trait IntoServerFnErrorCore<T> {
    fn into_sfn_core(self) -> Result<T, leptos::prelude::ServerFnError>;
}

#[cfg(feature = "ssr")]
impl<T> IntoServerFnErrorCore<T> for Result<T, kyomi_core::Error> {
    fn into_sfn_core(self) -> Result<T, leptos::prelude::ServerFnError> {
        self.map_err(|e| {
            tracing::error!(error = %e, "server function error");
            leptos::prelude::ServerFnError::new(kyomi_core::sanitize_error(e.user_message()))
        })
    }
}

/// Extension trait that converts a raw `sqlx::Result<T>` into a server
/// function result via `kyomi_core::Error::user_message()`, never
/// `sqlx::Error`'s own `Display`.
///
/// `kyomi_core::db_fetch_scalar!` / `db_execute!` / `db_fetch_optional!`
/// (`crates/kyomi-core/src/db.rs`) wrap `sqlx::query...().fetch_...()`
/// directly and return `sqlx::Result<T>`, **not** `kyomi_core::Result<T>`.
/// A bare `.map_err(|e| ServerFnError::new(format!("...: {e}")))` on one of
/// those calls therefore bypasses both of this codebase's existing
/// protections: `Error::user_message()`'s fixed `"internal server error"`
/// for `Sqlx`/`Migrate`/`Redis`/`SerdeJson` (KYO-350) never applies because
/// there is no `kyomi_core::Error` to call it on, and `.into_sfn()`'s
/// `sanitize_error` only redacts URLs/credentials/hostnames — not the
/// constraint/column/table detail `sqlx::Error`'s `Display` can carry. Raw
/// driver detail reaches the client either way (KYO-526; see
/// `docs/standards/error-handling/user-message-not-display-for-user-facing-text.md`).
/// KYO-557 finished closing the gap: every remaining `.into_sfn()` call site
/// with `E = sqlx::Error` was migrated to `.into_sfn_sqlx()`, and the
/// `NotKyomiCoreError` impl for `sqlx::Error` was removed so a future one is
/// a compile error, not a silent leak (see [`NotKyomiCoreError`]).
///
/// A distinctly-named method (rather than requiring every such call site to
/// spell out `.map_err(kyomi_core::Error::from)?.into_sfn_core()`) documents
/// intent at the call site: "this is a raw DB error, deliberately collapsed
/// to a fixed string" reads differently from "this is already a
/// `kyomi_core::Error`". The implementation below is just that conversion —
/// `kyomi_core::Error::from` (the `#[from] sqlx::Error` arm) followed by
/// [`IntoServerFnErrorCore::into_sfn_core`] — so the mapping logic itself
/// exists in exactly one place. `Error::Sqlx` is `#[error(transparent)]`, so
/// its `Display` delegates straight to the inner `sqlx::Error` with no added
/// prefix — the `tracing::error!` line `into_sfn_core` emits still carries
/// the full, untouched sqlx detail server-side; only the client-facing
/// string is fixed.
#[cfg(feature = "ssr")]
pub(crate) trait IntoServerFnErrorSqlx<T> {
    fn into_sfn_sqlx(self) -> Result<T, leptos::prelude::ServerFnError>;
}

#[cfg(feature = "ssr")]
impl<T> IntoServerFnErrorSqlx<T> for Result<T, sqlx::Error> {
    fn into_sfn_sqlx(self) -> Result<T, leptos::prelude::ServerFnError> {
        self.map_err(kyomi_core::Error::from).into_sfn_core()
    }
}

#[cfg(all(test, feature = "ssr"))]
mod into_sfn_error_tests {
    use super::*;

    /// KYO-523 regression guard: `.into_sfn_core()` must build the
    /// client-facing message from `user_message()`, so none of
    /// `kyomi_core::Error`'s log-only variant tags (`"internal: "`,
    /// `"not found: "`, `"bad request: "`, ...) reach the client — even
    /// though `Display` (used for the `tracing::error!` log line right next
    /// to it) still carries the tag.
    #[test]
    fn into_sfn_core_strips_the_variant_tag_every_tagged_variant() {
        let cases: Vec<(kyomi_core::Error, &str)> = vec![
            (kyomi_core::Error::NotFound("widget missing".into()), "not found: "),
            (kyomi_core::Error::Unauthorized("no token".into()), "unauthorized: "),
            (kyomi_core::Error::Forbidden("no access".into()), "forbidden: "),
            (kyomi_core::Error::BadRequest("bad input".into()), "bad request: "),
            (kyomi_core::Error::Conflict("already exists".into()), "conflict: "),
            (
                kyomi_core::Error::TooManyRequests("slow down".into(), 30),
                "too many requests: ",
            ),
            (kyomi_core::Error::NotImplemented("soon".into()), "not implemented: "),
            (
                kyomi_core::Error::ServiceUnavailable("down for maintenance".into()),
                "service unavailable: ",
            ),
            (kyomi_core::Error::Internal("stack trace stuff".into()), "internal: "),
        ];

        for (err, tag) in cases {
            // The bug this guards against: Display carries the tag...
            assert!(
                err.to_string().starts_with(tag),
                "test fixture assumption broken: {err} should start with {tag:?}"
            );

            // ...but into_sfn_core's client-facing message must not.
            let sfn_err: Result<(), _> = Err(err).into_sfn_core();
            let client_message = sfn_err.unwrap_err().to_string();
            assert!(
                !client_message.contains(tag),
                "variant tag {tag:?} leaked into client-facing message: {client_message:?}"
            );
        }
    }

    /// Sibling assurance for [`NotKyomiCoreError`]: `.into_sfn()` remains
    /// available for the error types this crate actually pairs with it, so
    /// migrating call sites to `into_sfn_core` didn't accidentally narrow the
    /// blanket impl into uselessness for everything else.
    ///
    /// `sqlx::Error` is deliberately NOT used as the fixture here (KYO-557
    /// removed `impl NotKyomiCoreError for sqlx::Error`, so `.into_sfn()` on
    /// one is now a compile error, not something to assert against at
    /// runtime — see [`into_sfn_sqlx_tests`] instead).
    #[test]
    fn into_sfn_still_works_for_non_core_error_types() {
        let err: Result<(), kyomi_auth::workspace_ai_config::WorkspaceAiConfigError> =
            Err(kyomi_auth::workspace_ai_config::WorkspaceAiConfigError::WorkspaceNotFound(
                "ws_123".into(),
            ));
        assert!(err.into_sfn().is_err());
    }
}

#[cfg(all(test, feature = "ssr"))]
mod into_sfn_sqlx_tests {
    //! KYO-526 regression guard: `.into_sfn_sqlx()` must build the
    //! client-facing message from `kyomi_core::Error::user_message()`, so a
    //! raw `sqlx::Error`'s `Display` — which can carry constraint, column,
    //! or table detail — never reaches the client. If this test is
    //! reverted to asserting against `e.to_string()` (the bug), it fails:
    //! `sqlx::Error::RowNotFound`'s `Display` is `"no rows returned by a
    //! query that expected to return at least one row"`, not
    //! `"internal server error"`.
    //!
    //! KYO-557 extends this guard rather than duplicating it: every
    //! `.into_sfn()` call site in `server_fns/` with `E = sqlx::Error` was
    //! migrated to `.into_sfn_sqlx()`, and
    //! [`maps_column_not_found_constraint_style_detail_to_the_fixed_string`]
    //! below reproduces the exact leak the ticket proved against sqlx-core
    //! 0.8.6's real `Display` output: `no column found for name: ssn`.
    use super::IntoServerFnErrorSqlx;

    #[test]
    fn maps_sqlx_error_to_the_fixed_safe_string_not_its_raw_display() {
        let e = sqlx::Error::RowNotFound;
        // Sanity check on the fixture: if sqlx::Error's Display were already
        // "internal server error" this test couldn't distinguish the fix
        // from the bug it guards against.
        assert_ne!(e.to_string(), "internal server error");

        let result: Result<(), sqlx::Error> = Err(e);
        let client_message = result.into_sfn_sqlx().unwrap_err().to_string();
        // `ServerFnError::new(...)`'s own `Display` prepends "error running
        // server function: " to whatever string it's given, so the assertion
        // checks the fixed inner message survives intact rather than
        // depending on leptos's exact wrapper format.
        assert!(
            client_message.ends_with("internal server error"),
            "raw sqlx::Error detail must never reach the client (KYO-526) — expected the \
             message to end with the fixed \"internal server error\" string, got \
             {client_message:?}"
        );
        assert!(
            !client_message.contains("no rows returned"),
            "sqlx::Error::RowNotFound's raw Display leaked into the client-facing \
             message: {client_message:?}"
        );
    }

    /// KYO-557: reproduces the exact leak reported against sqlx-core 0.8.6 —
    /// a schema detail (here, a column name; the ticket also verified a
    /// unique-constraint name byte-for-byte) sitting in `sqlx::Error`'s
    /// `Display` and reaching the client verbatim through the old bare
    /// `.into_sfn()`. Mutation-proof: revert the call site this guards (or
    /// `IntoServerFnErrorSqlx::into_sfn_sqlx`'s body) back to
    /// `ServerFnError::new(kyomi_core::sanitize_error(&e.to_string()))` and
    /// this test fails, because `sanitize_error`'s three regex passes (URL /
    /// `key=value` credential / hostname redaction) do not touch column or
    /// constraint names — they were never built to.
    #[test]
    fn maps_column_not_found_constraint_style_detail_to_the_fixed_string() {
        let e = sqlx::Error::ColumnNotFound("ssn".into());
        // Sanity check on the fixture: this is sqlx-core 0.8.6's real
        // Display output for this variant, proven verbatim against a live
        // query in the ticket. If it ever stopped containing the column
        // name this test couldn't distinguish the fix from the bug.
        assert_eq!(e.to_string(), "no column found for name: ssn");

        let result: Result<(), sqlx::Error> = Err(e);
        let client_message = result.into_sfn_sqlx().unwrap_err().to_string();
        assert!(
            client_message.ends_with("internal server error"),
            "raw sqlx::Error column detail must never reach the client (KYO-557) — \
             expected the message to end with the fixed \"internal server error\" \
             string, got {client_message:?}"
        );
        assert!(
            !client_message.contains("ssn") && !client_message.contains("column"),
            "sqlx::Error::ColumnNotFound's raw Display (a schema column name) leaked \
             into the client-facing message: {client_message:?}"
        );
    }
}

/// KYO-805: pins the small, explicit "opt out of the billing gate" allowlist
/// by source inspection — the same technique
/// `server_fns::workspace::tests::get_workspace_slack_status_requires_manage_integrations`
/// (KYO-321) uses, for the same reason: these are `#[cfg(feature = "ssr")]`
/// functions whose auth extraction needs a real Leptos/Axum request context
/// leptos_axum provides, which a plain unit test can't fake (see
/// `kyomi_auth::permissions::tests::gated_server_fn`'s doc comment). A
/// `kyomi_auth::middleware::tests` extractor-level test proves the
/// *mechanism* (`AuthUser` vs `AuthUserAllowLapsed`) rejects/admits
/// correctly; this proves each server fn actually calls the extractor its
/// role in the KYO-805 ticket's allowlist requires — a correct extractor
/// called from the wrong place is just as broken as an incorrect one called
/// from the right place, and neither failure is visible to the other test.
#[cfg(all(test, feature = "ssr"))]
mod kyo_805_billing_gate_allowlist_tests {
    use crate::test_support::extract_between;

    const SECURITY_SRC: &str = include_str!("security.rs");
    const CONTEXT_SRC: &str = include_str!("context.rs");
    const SIDEBAR_SRC: &str = include_str!("sidebar.rs");
    const WORKSPACE_SRC: &str = include_str!("workspace.rs");
    const BILLING_SRC: &str = include_str!("billing.rs");

    #[test]
    fn logout_all_sessions_is_allow_lapsed() {
        let body = extract_between(
            SECURITY_SRC,
            "pub async fn logout_all_sessions() -> Result<String, ServerFnError> {",
            "\n// ---------------------------------------------------------------------------\n// Passkey management server functions",
        );
        assert!(
            body.contains("extract_auth_allow_lapsed()"),
            "logout_all_sessions must opt out of the KYO-805 billing gate — a lapsed \
             workspace must still be able to log out everywhere"
        );
    }

    #[test]
    fn other_security_fns_remain_gated() {
        // Spot-check a sibling in the same file: passkeys/2FA/password stay
        // on the strict gate — only logout_all_sessions (and the
        // already-unauthenticated logout()) opt out.
        let body = extract_between(
            SECURITY_SRC,
            "pub async fn set_password(new_password: String) -> Result<String, ServerFnError> {",
            "\n/// Change password for a user who already has one.",
        );
        assert!(body.contains("extract_auth()"));
        assert!(!body.contains("extract_auth_allow_lapsed()"));
    }

    #[test]
    fn get_user_context_is_allow_lapsed() {
        let body = extract_between(
            CONTEXT_SRC,
            "pub async fn get_user_context() -> Result<UserContext, ServerFnError> {",
            "\n/// Convert the Capabilities struct into a HashMap<String, bool> for the frontend.",
        );
        assert!(body.contains("extract_auth_allow_lapsed()"));
    }

    #[test]
    fn get_sidebar_user_is_allow_lapsed_and_reads_billing_lapsed_off_workspace_context() {
        let body = extract_between(
            SIDEBAR_SRC,
            "pub async fn get_sidebar_user() -> Result<SidebarUser, ServerFnError> {",
            "\n}\n",
        );
        assert!(body.contains("extract_auth_allow_lapsed()"));
        // KYO-805: must read the single-computation-site verdict off
        // WorkspaceContext, not re-derive it with a second workspace load
        // (that was the pre-KYO-805 shape of this function).
        assert!(body.contains("auth.workspace.billing_lapsed"));
        assert!(!body.contains("is_billing_lapsed("));
    }

    #[test]
    fn get_recent_sessions_remains_gated() {
        let body = extract_between(
            SIDEBAR_SRC,
            "pub async fn get_recent_sessions() -> Result<Vec<SidebarSession>, ServerFnError> {",
            "\n/// Load current user info for the sidebar user menu.",
        );
        assert!(body.contains("extract_auth()"));
        assert!(!body.contains("extract_auth_allow_lapsed"));
    }

    #[test]
    fn workspace_switcher_fns_are_allow_lapsed() {
        let list_body = extract_between(
            WORKSPACE_SRC,
            "pub async fn list_my_workspaces() -> Result<Vec<WorkspaceSummary>, ServerFnError> {",
            "\n/// Switch the caller's active workspace and re-mint their session.",
        );
        assert!(list_body.contains("extract_auth_allow_lapsed()"));

        let switch_body = extract_between(
            WORKSPACE_SRC,
            "pub async fn switch_workspace(workspace_id: String) -> Result<(), ServerFnError> {",
            "\n// ─────────────────────────────────────────────────────────────────────────────\n// Helpers (server-only)",
        );
        assert!(switch_body.contains("extract_auth_allow_lapsed()"));
    }

    #[test]
    fn workspace_settings_writer_remains_gated() {
        let body = extract_between(
            WORKSPACE_SRC,
            "pub async fn update_workspace_name(name: String) -> Result<(), ServerFnError> {",
            "\n/// Update the workspace default AI model.",
        );
        assert!(body.contains("AuthenticatedContext::extract()"));
        assert!(!body.contains("extract_allow_lapsed"));
    }

    #[test]
    fn allowlisted_billing_fns_use_the_allow_lapsed_extractor() {
        let cases: &[(&str, &str, &str)] = &[
            (
                "pub async fn get_subscription_info() -> Result<SubscriptionInfo, ServerFnError> {",
                "\n/// Fetch recent invoices for the current workspace.",
                "AuthenticatedContext::extract_allow_lapsed()",
            ),
            (
                "pub async fn get_invoices() -> Result<Vec<InvoiceRecord>, ServerFnError> {",
                "\n/// Create a Stripe checkout session for subscription.",
                "AuthenticatedContext::extract_allow_lapsed()",
            ),
            (
                "pub async fn create_checkout(",
                "\n/// Fetch the details the full-screen billing paywall renders from",
                "AuthenticatedContext::extract_allow_lapsed()",
            ),
            (
                "pub async fn get_billing_paywall() -> Result<BillingPaywall, ServerFnError> {",
                "\n/// Start past-due payment recovery:",
                "AuthenticatedContext::extract_allow_lapsed()",
            ),
            (
                "pub async fn start_payment_recovery() -> Result<EmbeddedCheckoutSession, ServerFnError> {",
                "\n/// Complete past-due payment recovery",
                "AuthenticatedContext::extract_allow_lapsed()",
            ),
            (
                "pub async fn complete_payment_recovery(",
                "\n/// Sync a completed new-subscription Checkout Session's resulting",
                "AuthenticatedContext::extract_allow_lapsed()",
            ),
            (
                "pub async fn sync_checkout_subscription(session_id: String) -> Result<(), ServerFnError> {",
                "\n/// Cancel the current subscription at period end.",
                "AuthenticatedContext::extract_allow_lapsed()",
            ),
            (
                "pub async fn create_portal_session() -> Result<RedirectUrl, ServerFnError> {",
                "\n/// Purchase an AI token bundle via embedded Stripe checkout.",
                "AuthenticatedContext::extract_allow_lapsed()",
            ),
            (
                "pub async fn get_stripe_publishable_key() -> Result<Option<String>, ServerFnError> {",
                "\n/// Check the status of a checkout session (for verifying completion).",
                "extract_auth_allow_lapsed()",
            ),
            (
                "pub async fn get_checkout_session_status(",
                "\n#[cfg(all(test, feature = \"ssr\"))]\nmod tests {",
                "extract_auth_allow_lapsed()",
            ),
        ];

        for (start, end, expected) in cases {
            let body = extract_between(BILLING_SRC, start, end);
            assert!(
                body.contains(expected),
                "expected {start:?}'s body to call {expected}, got:\n{body}"
            );
        }
    }

    #[test]
    fn non_allowlisted_billing_fns_remain_gated() {
        let cases: &[(&str, &str)] = &[
            (
                "pub async fn cancel_subscription() -> Result<BillingResult, ServerFnError> {",
                "\n/// Reactivate a cancelled subscription.",
            ),
            (
                "pub async fn reactivate_subscription() -> Result<BillingResult, ServerFnError> {",
                "\npub const UNLIMITED_SEAT_CAP",
            ),
            (
                "pub async fn update_user_limit(limit: i32) -> Result<i32, ServerFnError> {",
                "\n/// Create a Stripe billing portal session and return the redirect URL.",
            ),
            (
                "pub async fn purchase_ai_bundle(quantity: u32) -> Result<EmbeddedCheckoutSession, ServerFnError> {",
                "\n/// Purchase an analytics event bundle via embedded Stripe checkout.",
            ),
            (
                "pub async fn purchase_analytics_bundle(quantity: u32) -> Result<EmbeddedCheckoutSession, ServerFnError> {",
                "\n/// Get the Stripe publishable key (needed for embedded checkout on the frontend).",
            ),
        ];

        for (start, end) in cases {
            let body = extract_between(BILLING_SRC, start, end);
            assert!(
                body.contains("AuthenticatedContext::extract()"),
                "expected {start:?}'s body to still call the gated AuthenticatedContext::extract()"
            );
            assert!(!body.contains("extract_allow_lapsed"));
        }
    }
}

/// KYO-806: closes the class on a future `#[server(...)]` silently bypassing
/// the paywall.
///
/// `server_fn` 0.8 has no global request/response hook (see
/// `paywall_client`'s module doc) — every `#[server(...)]` attribute must
/// individually opt in to 402 interception via
/// `client = crate::server_fns::paywall_client::PaywallAwareClient`. Nothing
/// stops a future server fn from being added without that argument (the
/// macro silently falls back to the default `BrowserClient`, which never
/// calls `report_payment_required`) — this test scans every `#[server(...)]`
/// attribute in every file under `server_fns/` by source inspection (same
/// technique `kyo_805_billing_gate_allowlist_tests` uses, for the same
/// reason: this is compile-time source text, not something a runtime test
/// against a live request context can observe) and fails if even one is
/// missing the client override.
#[cfg(all(test, feature = "ssr"))]
mod kyo_806_paywall_client_allowlist_tests {
    const AI_SRC: &str = include_str!("ai.rs");
    const ANALYTICS_SRC: &str = include_str!("analytics.rs");
    const AUTH_SRC: &str = include_str!("auth.rs");
    const BILLING_SRC: &str = include_str!("billing.rs");
    const CHAT_SRC: &str = include_str!("chat.rs");
    const COLLECTIONS_SRC: &str = include_str!("collections.rs");
    const CONNECT_SRC: &str = include_str!("connect.rs");
    const CONTEXT_SRC: &str = include_str!("context.rs");
    const COPILOT_SRC: &str = include_str!("copilot.rs");
    const DASHBOARDS_SRC: &str = include_str!("dashboards.rs");
    const DATASOURCE_OAUTH_SRC: &str = include_str!("datasource_oauth.rs");
    const DATASOURCES_SRC: &str = include_str!("datasources.rs");
    const FEEDBACK_SRC: &str = include_str!("feedback.rs");
    const HOME_SRC: &str = include_str!("home.rs");
    const KNOWLEDGE_SRC: &str = include_str!("knowledge.rs");
    const ONBOARDING_SRC: &str = include_str!("onboarding.rs");
    const OWNERSHIP_SRC: &str = include_str!("ownership.rs");
    const PROFILE_SRC: &str = include_str!("profile.rs");
    const SECURITY_SRC: &str = include_str!("security.rs");
    const SETUP_SRC: &str = include_str!("setup.rs");
    const SIDEBAR_SRC: &str = include_str!("sidebar.rs");
    const SLACK_SRC: &str = include_str!("slack.rs");
    const SQL_EDITOR_SRC: &str = include_str!("sql_editor.rs");
    const TEAM_SRC: &str = include_str!("team.rs");
    const UNSUBSCRIBE_SRC: &str = include_str!("unsubscribe.rs");
    const USAGE_SRC: &str = include_str!("usage.rs");
    const WATCHES_SRC: &str = include_str!("watches.rs");
    const WORKSPACE_SRC: &str = include_str!("workspace.rs");

    /// Every file under `server_fns/` that declares at least one
    /// `#[server(...)]` attribute, paired with its source text. Extending
    /// this list is how a newly-added `server_fns/*.rs` file gets covered —
    /// `all_server_attrs_have_paywall_client` iterates it, so a file added
    /// here and left off is the only way this test could miss one.
    const FILES: &[(&str, &str)] = &[
        ("ai.rs", AI_SRC),
        ("analytics.rs", ANALYTICS_SRC),
        ("auth.rs", AUTH_SRC),
        ("billing.rs", BILLING_SRC),
        ("chat.rs", CHAT_SRC),
        ("collections.rs", COLLECTIONS_SRC),
        ("connect.rs", CONNECT_SRC),
        ("context.rs", CONTEXT_SRC),
        ("copilot.rs", COPILOT_SRC),
        ("dashboards.rs", DASHBOARDS_SRC),
        ("datasource_oauth.rs", DATASOURCE_OAUTH_SRC),
        ("datasources.rs", DATASOURCES_SRC),
        ("feedback.rs", FEEDBACK_SRC),
        ("home.rs", HOME_SRC),
        ("knowledge.rs", KNOWLEDGE_SRC),
        ("onboarding.rs", ONBOARDING_SRC),
        ("ownership.rs", OWNERSHIP_SRC),
        ("profile.rs", PROFILE_SRC),
        ("security.rs", SECURITY_SRC),
        ("setup.rs", SETUP_SRC),
        ("sidebar.rs", SIDEBAR_SRC),
        ("slack.rs", SLACK_SRC),
        ("sql_editor.rs", SQL_EDITOR_SRC),
        ("team.rs", TEAM_SRC),
        ("unsubscribe.rs", UNSUBSCRIBE_SRC),
        ("usage.rs", USAGE_SRC),
        ("watches.rs", WATCHES_SRC),
        ("workspace.rs", WORKSPACE_SRC),
    ];

    const REQUIRED_CLIENT_ARG: &str =
        "client = crate::server_fns::paywall_client::PaywallAwareClient";

    #[test]
    fn all_server_attrs_have_paywall_client() {
        let mut total_server_attrs = 0usize;

        for (file, src) in FILES {
            for line in src.lines() {
                let trimmed = line.trim_start();
                if !trimmed.starts_with("#[server(") {
                    continue;
                }
                total_server_attrs += 1;
                assert!(
                    trimmed.contains(REQUIRED_CLIENT_ARG),
                    "{file}: found a #[server(...)] attribute missing `{REQUIRED_CLIENT_ARG}` \
                     (KYO-806) — every server fn in this crate must route through \
                     PaywallAwareClient so a 402 payment_required response is never silently \
                     ignored. Offending attribute: {trimmed}"
                );
            }
        }

        // Floor check (same idea as `lib.rs`'s `SERVER_FN_REGISTRY_FLOOR`):
        // guards against `FILES` silently going stale — a `server_fns/*.rs`
        // file added without an entry here would make this test vacuously
        // pass. 205 is the count as of KYO-806; only grows over time.
        assert!(
            total_server_attrs >= 205,
            "expected at least 205 #[server(...)] attributes across FILES, found \
             {total_server_attrs} — either a server_fns/*.rs file gained a #[server] fn \
             (fine, just bump this floor) or FILES is missing a file entirely (not fine — \
             add it, otherwise its server fns are invisible to this allowlist)"
        );
    }
}
