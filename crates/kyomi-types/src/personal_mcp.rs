// SPDX-License-Identifier: AGPL-3.0-or-later

//! Secret-free personal external MCP contracts. Ownership never belongs to a workspace.
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum McpAuthKind {
    None,
    Bearer,
    Oauth,
}
impl McpAuthKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Bearer => "bearer",
            Self::Oauth => "oauth",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersonalMcpConnection {
    pub connection_id: String,
    pub owner_user_id: String,
    pub name: String,
    pub endpoint: String,
    pub auth_kind: McpAuthKind,
    pub status: String,
    pub generation: i64,
}

/// Requests have no generic headers or credential maps. Bearer input is write-only:
/// Deserialize only, with a custom Debug implementation that never renders its value.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SavePersonalMcpConnection {
    pub name: String,
    pub endpoint: String,
    pub auth_kind: McpAuthKind,
    pub bearer_secret: Option<String>,
}
impl std::fmt::Debug for SavePersonalMcpConnection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SavePersonalMcpConnection")
            .field("name", &self.name)
            .field("endpoint", &self.endpoint)
            .field("auth_kind", &self.auth_kind)
            .field("bearer_secret", &"[REDACTED]")
            .finish()
    }
}
