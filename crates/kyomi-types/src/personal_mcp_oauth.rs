// SPDX-License-Identifier: AGPL-3.0-or-later
//! Public outbound OAuth contracts. Credentials and PKCE material never enter these DTOs.
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum McpAuthorizationRequired {
    Reconnect,
    StepUp { scopes: Vec<String> },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct McpOAuthDiscovery {
    pub issuer: String,
    pub resource: String,
    pub scopes: Vec<String>,
    pub registration: String,
}

#[derive(Deserialize)]
pub struct StartMcpOAuth {
    pub generation: i64,
    pub issuer: String,
    pub scopes: Vec<String>,
    pub consent: bool,
    pub return_to: String,
}

/// Contains an opaque, short-lived state in the authorization URL. Never cache or log.
#[derive(Serialize, Deserialize)]
pub struct McpOAuthStarted {
    pub authorization_url: String,
    pub generation: i64,
}

/// Callback parameters are intentionally neither Debug nor Serialize.
#[derive(Deserialize, Default)]
pub struct McpOAuthCallback {
    pub state: String,
    pub code: Option<String>,
    pub iss: Option<String>,
    pub error: Option<String>,
}
