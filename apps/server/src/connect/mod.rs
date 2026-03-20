// SPDX-License-Identifier: AGPL-3.0-or-later

//! Kyomi Connect — WebSocket endpoint for customer-deployed Connect instances.
//!
//! The Connect binary establishes an outbound WebSocket to this endpoint,
//! authenticates with a JWT token, and then acts as a bridge between the Kyomi
//! backend and the customer's on-premise database.
//!
//! ## Module structure
//!
//! - [`handler`] — Axum WebSocket upgrade handler and message loop
//! - [`info`] — HTTP endpoint returning datasource metadata for the setup wizard
//! - [`registry`] — Maps `datasource_config_id` to active WebSocket connections

use axum::http::HeaderMap;

pub mod handler;
pub mod info;
pub mod provider;
pub mod registry;

/// Extract a Bearer token from the `Authorization` header.
///
/// Shared by the WebSocket handler and the info endpoint.
pub(crate) fn extract_bearer_token(headers: &HeaderMap) -> Option<String> {
    let value = headers.get("authorization")?.to_str().ok()?;
    let stripped = value.strip_prefix("Bearer ")?;
    if stripped.is_empty() {
        return None;
    }
    Some(stripped.to_string())
}
