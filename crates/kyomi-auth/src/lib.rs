// SPDX-License-Identifier: AGPL-3.0-or-later

//! kyomi-auth — Authentication & encryption for the Kyomi backend.
//!
//! Provides:
//! - JWT creation and validation (HS256, shared secret with Python backend)
//! - AES-256-GCM encryption for credentials at rest
//! - Password hashing (bcrypt for legacy, argon2 for new)
//! - Axum middleware for extracting authenticated users
//! - User service (CRUD operations)
//! - Token service (refresh tokens, verification tokens)
//! - Cookie helpers
//! - Redis-backed rate limiting

pub mod analytics_clickhouse;
pub mod analytics_notifications;
pub mod analytics_quota;
pub mod analytics_site_service;
pub mod billing_service;
pub mod catalog;
pub mod chat_service;
pub mod collection_service;
pub mod connect_token;
pub mod cookies;
pub mod credential_service;
pub mod dashboard_service;
pub mod datasource_auth_service;
pub mod datasource_oauth;
pub mod datasource_service;
pub mod email_service;
pub mod embedding_persistence;
pub mod encryption;
pub mod google_oauth;
pub mod jwt;
pub mod learning_service;
pub mod middleware;
pub mod notifications;
pub mod password;
pub mod push_service;
pub mod rate_limiter;
pub mod redis_ops;
pub mod session;
// Slack client and helpers moved to enterprise/kyomi-slack crate (Phase 12).
pub mod sql_history_service;
pub mod stripe_config;
pub mod stripe_service;
pub mod totp;
pub mod token_service;
pub mod user_service;
pub mod watch_service;
pub mod webauthn;
pub mod websocket;
pub mod workspace_service;

/// Build a shared HTTP client with a proper User-Agent header.
///
/// Some APIs (notably Snowflake) reject requests without a User-Agent.
/// All HTTP clients in this crate should use this function.
pub fn http_client() -> kyomi_core::Result<reqwest::Client> {
    reqwest::Client::builder()
        .user_agent("Kyomi/1.0")
        .build()
        .map_err(|e| kyomi_core::Error::Internal(format!("Failed to build HTTP client: {e}")))
}
