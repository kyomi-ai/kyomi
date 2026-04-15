// SPDX-License-Identifier: AGPL-3.0-or-later

//! API route modules.

pub mod admin_notify;
pub mod analytics_sites;
pub mod auth;
pub mod auth_datasource_oauth;
pub mod auth_google_oauth;
pub mod auth_passkeys;
pub mod auth_recovery;
pub mod auth_password;
pub mod auth_totp;
pub mod bigquery;
pub mod catalog;
pub mod chart_context;
pub mod chart_generate;
pub mod chartml;
pub mod chat;
pub mod copilot;
pub mod collections;
pub mod dashboards;
pub mod datasources;
pub mod feedback;
pub mod integrations;
pub mod learnings;
pub mod sql_history;
pub mod subscribe;
pub mod system_config;
pub mod usage;
pub mod users;
pub mod billing;
pub mod mcp;
pub mod oauth;
pub mod push;
// Slack routes moved to enterprise/kyomi-slack crate (Phase 12).
pub mod watches;
pub mod websocket;
pub mod workspaces;
