// SPDX-License-Identifier: AGPL-3.0-or-later

//! kyomi-api — Axum HTTP server for the Kyomi backend.
//!
//! Assembles all routes, middleware, and shared application state.

pub mod cancel_registry;
pub mod connect;
pub mod frontend;
pub mod health;
pub mod helpers;
pub mod leptos_frontend;
pub mod mcp_session_manager;
pub mod middleware;
pub mod routes;
pub mod state;

use std::net::SocketAddr;

use axum::extract::{FromRef, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Router;
use tower::Layer;
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::normalize_path::NormalizePathLayer;
use tower_http::trace::TraceLayer;

use kyomi_core::db::DbPool;
use kyomi_core::sql_compat;
use kyomi_core::{db_execute, db_fetch_scalar};

/// Auto-provision a local user and workspace for personal (desktop) mode.
///
/// Called once at startup when `config.is_personal()` is true. If the `users`
/// table is empty, creates a fixed local user, workspace, and membership so the
/// app is immediately usable without any signup or authentication flow.
///
/// Idempotent: skips silently if any users already exist.
pub async fn auto_provision_personal_mode(db: &DbPool) -> Result<(), kyomi_core::Error> {
    let is_pg = db.is_postgres();

    let user_count: i64 = db_fetch_scalar!(db, i64, "SELECT COUNT(*) FROM users")?;
    if user_count > 0 {
        return Ok(());
    }

    let now = sql_compat::now(is_pg);
    let bool_true = sql_compat::bool_true(is_pg);

    // Create local user
    let user_sql = format!(
        "INSERT INTO users (user_id, email, name, verified, active, created_at, updated_at) \
         VALUES ($1, $2, $3, {bool_true}, {bool_true}, {now}, {now})"
    );
    db_execute!(db, &user_sql, "user-local", "local@localhost", "Local User")?;

    // Create workspace
    let workspace_sql = format!(
        "INSERT INTO workspaces (workspace_id, name, owner_user_id, subscription_tier, subscription_status, created_at, updated_at) \
         VALUES ($1, $2, $3, $4, $5, {now}, {now})"
    );
    db_execute!(
        db,
        &workspace_sql,
        "workspace-local",
        "My Workspace",
        "user-local",
        "enterprise",
        "active"
    )?;

    // Create workspace membership
    let membership_sql = format!(
        "INSERT INTO workspace_users (workspace_id, user_id, role, active, created_at) \
         VALUES ($1, $2, $3, {bool_true}, {now})"
    );
    db_execute!(
        db,
        &membership_sql,
        "workspace-local",
        "user-local",
        "workspace_admin"
    )?;

    tracing::info!("Personal mode: auto-provisioned local user and workspace");

    Ok(())
}

/// Optional components that platform-specific features inject into the
/// Leptos server-function context. Passed to [`build_router`] so the
/// server-context struct can include them without adding enterprise
/// dependencies to `AppState`.
#[derive(Default)]
pub struct ServerExtras {
    /// Slack HTTP client for Slack Web API calls (channel listing, etc.).
    /// Present only when the `slack` feature is enabled and Slack is configured.
    #[cfg(feature = "slack")]
    pub slack_client: Option<kyomi_slack::client::SlackClient>,
}

/// Build the axum Router with all core routes and middleware.
///
/// Returns a `Router<()>` (state already applied). Platform-specific routes
/// (e.g. Slack) can be merged into this router before wrapping with
/// `NormalizePathLayer`.
///
/// `extras` carries optional platform-specific components (e.g. Slack client)
/// that need to reach the Leptos server-function context without polluting
/// `AppState`.
pub fn build_router(state: state::AppState, extras: ServerExtras) -> Router {
    let demo_mode = state.config.demo_mode;

    Router::new()
        // Health check at both /api/health and /health (alias for nginx-less deployments)
        .route("/api/health", axum::routing::get(health::health_check))
        .route("/health", axum::routing::get(health::health_check))
        // Auth routes under /api/v1/auth
        .nest("/api/v1/auth", routes::auth::routes())
        .nest("/api/v1/auth", routes::auth_google_oauth::routes())
        .nest("/api/v1/auth", routes::auth_passkeys::routes())
        .nest("/api/v1/auth", routes::auth_password::routes())
        .nest("/api/v1/auth", routes::auth_totp::routes())
        .nest("/api/v1/auth", routes::auth_recovery::routes())
        .nest("/api/v1/auth/oauth", routes::auth_datasource_oauth::routes())
        .nest("/api/v1/users", routes::users::routes())
        .nest("/api/v1/workspaces", routes::workspaces::routes())
        .nest("/api/v1/workspaces", routes::learnings::routes())
        .nest("/api/v1/workspaces", routes::knowledge_files::routes())
        .nest(
            "/api/v1/datasources",
            routes::datasources::routes().merge(routes::catalog::routes()),
        )
        .nest("/api/v1/chat", routes::chat::routes())
        .nest("/api/v1/chat/copilot", routes::copilot::routes())
        .nest("/api/v1/sql", routes::sql_history::routes())
        .nest("/api/v1/bigquery", routes::bigquery::routes())
        .nest("/api/v1/trial", routes::trial_chat::router())
        .nest("/api/v1/watches", routes::watches::routes())
        .nest("/api/v1/billing", routes::billing::routes())
        .nest("/api/v1/integrations", routes::integrations::routes())
        .nest("/api/v1/push", routes::push::routes())
        .nest("/mcp", routes::mcp::routes())
        // OAuth well-known discovery at root level (RFC 8414, RFC 9728, OpenID)
        .merge(routes::oauth::well_known_routes())
        .nest("/api/v1/oauth", routes::oauth::routes())
        .nest("/api/v1/dashboards", routes::dashboards::routes())
        .nest("/api/v1/collections", routes::collections::routes())
        .nest("/api/v1/chart-context", routes::chart_context::routes())
        .nest("/api/v1/chart", routes::chart_generate::routes())
        .nest("/api/v1/chartml", routes::chartml::routes())
        .nest("/api/v1/usage", routes::usage::routes())
        .nest("/api/v1/analytics/sites", routes::analytics_sites::routes())
        .nest("/api/v1/analytics/usage", routes::analytics_sites::usage_routes())
        // Subscribe routes (public, no auth required)
        .nest("/api/v1", routes::subscribe::routes())
        // System config route (public, no auth required — frontend needs this before login)
        .nest("/api/v1/system", routes::system_config::routes())
        .nest("/api/v1/feedback", routes::feedback::routes())
        // JWKS endpoint for Kyomi Connect token verification (public, no auth)
        .route("/.well-known/jwks.json", axum::routing::get(jwks_handler))
        // Glama MCP directory — server ownership claim
        .route("/.well-known/glama.json", axum::routing::get(glama_json_handler))
        // Kyomi Connect WebSocket (JWT-authenticated, separate from user WebSockets)
        // /connect/v1 — via app.kyomi.ai (internal/dev)
        // /v1         — via connect.kyomi.ai (production)
        .route("/connect/v1", axum::routing::get(connect::handler::connect_websocket_handler))
        .route("/v1", axum::routing::get(connect::handler::connect_websocket_handler))
        // Kyomi Connect info endpoint (JWT-authenticated, returns datasource metadata)
        .route("/api/v1/connect/info", axum::routing::get(connect::info::connect_info))
        // WebSocket routes at root level (not under /api/v1)
        .route("/ws/{user_id}", axum::routing::get(routes::websocket::ws_handler))
        .route("/ws/trial/{session_id}", axum::routing::get(routes::websocket::ws_trial_handler))
        // Leptos frontend routes — auth pages
        .route("/login", axum::routing::get(leptos_frontend::serve_leptos_shell))
        .route("/signup/complete", axum::routing::get(leptos_frontend::serve_leptos_shell))
        .route("/auth/google/callback", axum::routing::get(leptos_frontend::serve_leptos_shell))
        .route("/account/recover", axum::routing::get(leptos_frontend::serve_leptos_shell))
        .route("/account/recover/complete", axum::routing::get(leptos_frontend::serve_leptos_shell))
        .route("/auth/passkey-signup", axum::routing::get(leptos_frontend::serve_leptos_shell))
        .route("/auth/recover-passkey", axum::routing::get(leptos_frontend::serve_leptos_shell))
        .route("/auth/recover-passkey/complete", axum::routing::get(leptos_frontend::serve_leptos_shell))
        // Leptos frontend routes — dashboard pages
        .route("/dashboards", axum::routing::get(leptos_frontend::serve_leptos_shell))
        .route("/dashboard/{*path}", axum::routing::get(leptos_frontend::serve_leptos_shell))
        // Leptos frontend routes — settings pages
        .route("/settings/profile", axum::routing::get(leptos_frontend::serve_leptos_shell))
        .route("/settings/security", axum::routing::get(leptos_frontend::serve_leptos_shell))
        .route("/settings/usage", axum::routing::get(leptos_frontend::serve_leptos_shell))
        .route("/settings/workspace", axum::routing::get(leptos_frontend::serve_leptos_shell))
        .route("/settings/analytics", axum::routing::get(leptos_frontend::serve_leptos_shell))
        .route("/settings/team", axum::routing::get(leptos_frontend::serve_leptos_shell))
        .route("/settings/billing", axum::routing::get(leptos_frontend::serve_leptos_shell))
        .route("/settings/datasources", axum::routing::get(leptos_frontend::serve_leptos_shell))
        .route("/leptos/{*path}", axum::routing::get(leptos_frontend::serve_leptos_asset))
        // Leptos server functions — typed RPC replacing REST calls
        // Uses /leptos-api/ prefix to avoid conflicts with /api/v1/ REST routes
        .route("/leptos-api/{*fn_name}", axum::routing::post({
            let server_ctx = kyomi_ui::server_fns::ServerContext {
                db: state.db.clone(),
                config: state.config.clone(),
                auth_state: kyomi_auth::middleware::AuthState::from_ref(&state),
                encryption_key: Some(state.encryption_key.clone()),
                kv: Some(state.kv.clone()),
                webauthn: Some(state.webauthn.clone()),
                embedding: state.embedding.clone(),
                connect_registry: Some(state.connect_registry.clone()),
                ws_manager: Some(state.ws_manager.clone()),
                cancel_registry: Some(kyomi_ui::server_fns::CancelRegistry::from_shared(
                    state.cancel_registry.tokens.clone(),
                )),
                platforms: Some(state.platforms.clone()),
                #[cfg(feature = "slack")]
                slack_client: extras.slack_client,
            };
            move |req: axum::http::Request<axum::body::Body>| {
                let ctx = server_ctx.clone();
                async move {
                    leptos_axum::handle_server_fns_with_context(
                        move || {
                            leptos::prelude::provide_context(ctx.clone());
                        },
                        req,
                    )
                    .await
                }
            }
        }))
        .fallback(frontend::serve)
        .with_state(state)
        .layer(axum::middleware::from_fn(middleware::security_headers))
        .layer(axum::Extension(middleware::DemoModeFlag(demo_mode)))
        .layer(middleware::cors_layer())
        .layer(TraceLayer::new_for_http())
        .layer(RequestBodyLimitLayer::new(10 * 1024 * 1024)) // 10 MB
}

/// JWKS endpoint handler — returns the Connect public key for token verification.
///
/// Public endpoint (no authentication required). Returns 404 if Connect is not configured.
async fn jwks_handler(State(state): State<state::AppState>) -> impl IntoResponse {
    match &state.connect_token {
        Some(service) => (
            StatusCode::OK,
            [("content-type", "application/json")],
            service.jwks().to_string(),
        )
            .into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

/// Glama MCP directory — static JSON for server ownership claim.
///
/// <https://glama.ai/mcp/servers> uses `/.well-known/glama.json` to verify
/// domain ownership of MCP servers listed in their directory.
async fn glama_json_handler() -> impl IntoResponse {
    (
        StatusCode::OK,
        [("content-type", "application/json")],
        r#"{"$schema":"https://glama.ai/mcp/schemas/connector.json","maintainers":[{"email":"jason@yellowgorilla.net"}]}"#,
    )
}

/// Wrap a completed `Router` with path normalization and connect-info extraction.
///
/// Called from `main.rs` after all routes (including platform-specific ones) have
/// been merged into the router.
pub fn wrap_service(
    router: Router,
) -> axum::extract::connect_info::IntoMakeServiceWithConnectInfo<
    tower_http::normalize_path::NormalizePath<Router>,
    SocketAddr,
> {
    let app = NormalizePathLayer::trim_trailing_slash().layer(router);
    axum::ServiceExt::<axum::extract::Request>::into_make_service_with_connect_info::<SocketAddr>(
        app,
    )
}

/// Build the normalized service ready for `axum::serve`.
///
/// Convenience function that builds the core router and wraps it.
/// For platform-specific route mounting, use [`build_router`] + [`wrap_service`] instead.
pub fn build_service(
    state: state::AppState,
) -> axum::extract::connect_info::IntoMakeServiceWithConnectInfo<
    tower_http::normalize_path::NormalizePath<Router>,
    SocketAddr,
> {
    wrap_service(build_router(state, ServerExtras::default()))
}
