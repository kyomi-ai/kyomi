// SPDX-License-Identifier: AGPL-3.0-or-later

//! REST routes — external callers only.
//!
//! This module's public surface serves external callers: Stripe webhooks,
//! OAuth providers (Google, datasource), MCP clients, `kyomi-connect` CLI,
//! WebAuthn, public unauthenticated endpoints, and WebSocket upgrades.
//!
//! New internal-only HTTP endpoints belong in `crates/kyomi-ui/src/server_fns/`,
//! not here.

pub mod admin_notify;
pub mod auth_datasource_oauth;
pub mod auth_google_oauth;
pub mod auth_passkeys;
pub mod auth_token;
pub mod dashboard_export;
pub mod chartml;
pub mod integrations;
pub mod subscribe;
pub mod system_config;
pub mod billing;
pub mod mcp;
pub mod oauth;
pub mod push;
pub mod query_arrow;
// Slack routes moved to enterprise/kyomi-slack crate (Phase 12).
pub mod websocket;
