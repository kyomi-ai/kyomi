// SPDX-License-Identifier: AGPL-3.0-or-later
//! Trusted persisted credential adapter; never a browser response or Debug field.
use super::*;
/// Persisted encrypted by the connection store, never returned to a browser.
#[derive(Serialize, Deserialize)]
pub(super) struct Credential {
    pub(super) access_token: String,
    pub(super) refresh_token: Option<String>,
    pub(super) issuer: String,
    pub(super) resource: String,
    pub(super) token_endpoint: String,
    pub(super) client_id: String,
    pub(super) client_secret: Option<String>,
    pub(super) auth_method: String,
    pub(super) scopes: Vec<String>,
    pub(super) expires_at: Option<i64>,
    pub(super) account_revision: String,
}
pub(super) fn valid_token(token: &str) -> bool {
    !token.is_empty()
        && token.len() <= 16384
        && !token.chars().any(|c| c.is_whitespace() || c.is_control())
}
/// Trusted dispatch adapter. Call only after owner/workspace/generation checks.
/// The returned header is secret and must never be logged or cached in a DTO.
pub fn authorization_from_secret(
    connection: &PersonalMcpConnection,
    secret: Option<&McpSecret>,
) -> Result<Option<String>> {
    match connection.auth_kind {
        McpAuthKind::None => Ok(None),
        McpAuthKind::Bearer => {
            let token = secret
                .ok_or_else(|| invalid("MCP bearer credential is missing"))?
                .expose();
            if !valid_token(token) {
                return Err(provider_error());
            }
            Ok(Some(format!("Bearer {token}")))
        }
        McpAuthKind::Oauth => {
            let credential = credential(connection, secret)?;
            if credential
                .expires_at
                .is_some_and(|expiry| expiry <= chrono::Utc::now().timestamp())
            {
                return Err(invalid("MCP OAuth token expired; reconnect"));
            }
            Ok(Some(format!("Bearer {}", credential.access_token)))
        }
    }
}
/// Stable random account revision; changes only when OAuth credentials commit.
pub fn account_revision(
    connection: &PersonalMcpConnection,
    secret: Option<&McpSecret>,
) -> Result<Option<String>> {
    if connection.auth_kind == McpAuthKind::Oauth {
        Ok(Some(credential(connection, secret)?.account_revision))
    } else {
        Ok(None)
    }
}
fn credential(
    connection: &PersonalMcpConnection,
    secret: Option<&McpSecret>,
) -> Result<Credential> {
    let credential: Credential = serde_json::from_str(
        secret
            .ok_or_else(|| invalid("MCP OAuth credential is missing; reconnect"))?
            .expose(),
    )
    .map_err(|_| provider_error())?;
    if credential.resource != connection.endpoint
        || !valid_token(&credential.access_token)
        || uuid::Uuid::parse_str(&credential.account_revision).is_err()
    {
        return Err(invalid(
            "MCP OAuth credential binding is invalid; reconnect",
        ));
    }
    Ok(credential)
}
