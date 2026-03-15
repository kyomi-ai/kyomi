// SPDX-License-Identifier: AGPL-3.0-or-later

//! kyomi-api — Axum HTTP server for the Kyomi backend.
//!
//! Assembles all routes, middleware, and shared application state.

pub mod cancel_registry;
pub mod connect;
pub mod frontend;
pub mod health;
pub mod helpers;
pub mod mcp_session_manager;
pub mod middleware;
pub mod routes;
pub mod state;

use std::net::SocketAddr;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Router;
use tower::Layer;
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::normalize_path::NormalizePathLayer;
use tower_http::trace::TraceLayer;

/// Build the axum Router with all core routes and middleware.
///
/// Returns a `Router<()>` (state already applied). Platform-specific routes
/// (e.g. Slack) can be merged into this router before wrapping with
/// `NormalizePathLayer`.
pub fn build_router(state: state::AppState) -> Router {
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
    wrap_service(build_router(state))
}
