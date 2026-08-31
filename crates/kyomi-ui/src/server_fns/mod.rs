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
