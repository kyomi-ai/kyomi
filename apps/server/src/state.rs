// SPDX-License-Identifier: AGPL-3.0-or-later

//! Shared application state passed to all axum handlers.

use axum::extract::FromRef;
use kyomi_auth::connect_token::ConnectTokenService;
use kyomi_auth::middleware::AuthState;
// SlackClient moved to enterprise/kyomi-slack crate (Phase 12).
use kyomi_auth::stripe_service::StripeService;
use kyomi_auth::websocket::WebSocketManager;
use webauthn_rs::Webauthn;

use kyomi_core::platform::PlatformRegistry;

use crate::cancel_registry::CancelRegistry;
use crate::connect::registry::ConnectRegistry;
use crate::mcp_session_manager::MCPSessionManager;

/// Application-wide shared state.
///
/// Implements `Clone` (all fields are Arc-wrapped or cheaply cloneable).
#[derive(Clone)]
pub struct AppState {
    pub db: kyomi_core::DbPool,
    /// KVStore abstraction for auth, rate limiting, and session ops.
    /// Backed by Redis when `REDIS_URL` is set, otherwise in-memory (single-instance mode).
    pub kv: kyomi_core::KVPool,
    /// Raw Redis connection pool for components that require direct Redis access:
    /// agent execution (Lua scripts, pub/sub), ConnectRegistry, analytics counters,
    /// and trial chat sessions. `None` when running without Redis (in-memory mode).
    pub redis: Option<kyomi_core::RedisPool>,
    pub config: std::sync::Arc<kyomi_core::Config>,
    /// Derived 32-byte AES-256-GCM encryption key for credentials at rest.
    /// Derived once at startup from `Config::encryption_key` (base64url string).
    pub encryption_key: std::sync::Arc<[u8; 32]>,
    /// WebAuthn instance for passkey operations. Built once at startup.
    pub webauthn: std::sync::Arc<Webauthn>,
    /// Lazy-loaded embedding model for catalog search (all-MiniLM-L6-v2, 384 dims).
    /// Loads on a background thread; endpoints that need it get 503 during warmup (~440ms).
    pub embedding: kyomi_embed::LazyEmbedding,
    /// WebSocket manager with Redis pub/sub for multi-replica delivery.
    /// Cheaply cloneable (inner Arc).
    pub ws_manager: WebSocketManager,
    /// Stripe service for billing operations.
    /// `None` when `STRIPE_SECRET_KEY` is not configured (dev/test).
    pub stripe: Option<std::sync::Arc<StripeService>>,
    // SlackClient field removed — Slack integration moved to enterprise/kyomi-slack (Phase 12).
    /// MCP session manager for Streamable HTTP session tracking.
    /// Redis-backed for multi-replica consistency. Cheaply cloneable.
    pub mcp_sessions: MCPSessionManager,
    /// Registry for cancelling in-flight agent tasks via WebSocket `cancel_request`.
    /// Cheaply cloneable (inner Arc + DashMap).
    pub cancel_registry: CancelRegistry,
    /// JWT token service for Kyomi Connect (ES256 asymmetric signing).
    /// `None` when `CONNECT_JWT_PRIVATE_KEY` is not configured — Connect features unavailable.
    pub connect_token: Option<std::sync::Arc<ConnectTokenService>>,
    /// Registry of active Kyomi Connect WebSocket connections.
    /// Maps `datasource_config_id` to the command channel for each connection.
    /// Cheaply cloneable (inner `Arc` + `DashMap`).
    pub connect_registry: ConnectRegistry,
    /// Registry of messaging platform implementations (Slack, Teams, etc.).
    /// Immutable after startup — populated during `AppState` construction.
    pub platforms: std::sync::Arc<PlatformRegistry>,
}

// Allow extracting AuthState from AppState for the auth middleware.
impl FromRef<AppState> for AuthState {
    fn from_ref(state: &AppState) -> Self {
        AuthState {
            jwt_secret: state.config.jwt_secret.clone(),
            db: state.db.clone(),
            is_personal: state.config.is_personal(),
        }
    }
}
