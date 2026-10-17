// SPDX-License-Identifier: AGPL-3.0-or-later
//! Bounded OAuth metadata/challenge validation. Never trusts remote error prose.
use super::*;
#[derive(Deserialize)]
pub(super) struct DcrResponse {
    pub(super) client_id: String,
    pub(super) client_secret: Option<String>,
    pub(super) token_endpoint_auth_method: Option<String>,
    pub(super) redirect_uris: Option<Vec<String>>,
}
#[derive(Deserialize)]
pub(super) struct ResourceMetadata {
    pub(super) resource: String,
    pub(super) authorization_servers: Vec<String>,
    #[serde(default)]
    pub(super) scopes_supported: Vec<String>,
}
#[derive(Deserialize)]
pub(super) struct AuthorizationMetadata {
    pub(super) issuer: String,
    pub(super) authorization_endpoint: String,
    pub(super) token_endpoint: String,
    pub(super) registration_endpoint: Option<String>,
    #[serde(default)]
    pub(super) response_types_supported: Vec<String>,
    #[serde(default)]
    pub(super) code_challenge_methods_supported: Vec<String>,
    #[serde(default)]
    pub(super) token_endpoint_auth_methods_supported: Option<Vec<String>>,
    #[serde(default)]
    pub(super) client_id_metadata_document_supported: bool,
    #[serde(default)]
    pub(super) authorization_response_iss_parameter_supported: bool,
}
pub(super) struct Discovery {
    pub(super) resource: String,
    pub(super) scopes: Vec<String>,
    pub(super) server: AuthorizationMetadata,
}
impl Discovery {
    pub(super) fn public(&self, registration: &str) -> McpOAuthDiscovery {
        McpOAuthDiscovery {
            issuer: self.server.issuer.clone(),
            resource: self.resource.clone(),
            scopes: self.scopes.clone(),
            registration: registration.into(),
        }
    }
}
pub(super) fn https_url(value: &str) -> Result<Url> {
    let url = Url::parse(value).map_err(|_| invalid("OAuth metadata URL must be valid HTTPS"))?;
    if value.len() > 4096
        || url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return Err(invalid(
            "OAuth metadata URL must be HTTPS without credentials or fragment",
        ));
    }
    Ok(url)
}
pub(super) fn validate_scopes(scopes: &[String]) -> Result<()> {
    if scopes.len() > 64
        || scopes.iter().any(|s| {
            s.is_empty()
                || s.len() > 256
                || !s
                    .bytes()
                    .all(|b| b == 0x21 || (0x23..=0x5b).contains(&b) || (0x5d..=0x7e).contains(&b))
        })
    {
        return Err(invalid("OAuth scopes are invalid or too large"));
    }
    Ok(())
}
pub(super) fn validate_client(
    client: &RegisteredClient,
    server: &AuthorizationMetadata,
) -> Result<()> {
    if client.client_id.is_empty()
        || client.client_id.len() > 4096
        || client.client_id.chars().any(char::is_control)
    {
        return Err(provider_error());
    }
    if !matches!(
        client.token_endpoint_auth_method.as_str(),
        "none" | "client_secret_post" | "client_secret_basic"
    ) || (client.token_endpoint_auth_method != "none"
        && client
            .client_secret
            .as_ref()
            .is_none_or(|s| s.is_empty() || s.len() > 16384))
        || (client.token_endpoint_auth_method == "none" && client.client_secret.is_some())
    {
        return Err(provider_error());
    }
    // RFC8414 defaults omitted methods to client_secret_basic. Legacy Trakkt
    // explicitly advertises none; never infer confidential credentials.
    if !server
        .token_endpoint_auth_methods_supported
        .as_ref()
        .map(|v| v.contains(&client.token_endpoint_auth_method))
        .unwrap_or(client.token_endpoint_auth_method == "client_secret_basic")
    {
        return Err(invalid(
            "Provider does not advertise the selected client authentication method",
        ));
    }
    Ok(())
}
/// Relative paths only; rejects scheme-relative, encoded separator/control, and
/// browser backslash interpretation. Always resolve against a trusted origin.
pub fn safe_return_path(path: &str) -> Result<String> {
    if !path.starts_with('/')
        || path.starts_with("//")
        || path.len() > 2048
        || path.contains('\\')
        || path.contains('#')
        || path.chars().any(char::is_control)
        || path.to_ascii_lowercase().contains('%')
    {
        return Err(invalid(
            "OAuth return path must be a local path without encoded separators",
        ));
    }
    let base = Url::parse("https://example.invalid").map_err(|_| provider_error())?;
    let joined = base.join(path).map_err(|_| provider_error())?;
    if joined.origin() != base.origin() {
        return Err(invalid("OAuth return path must stay on this site"));
    }
    Ok(path.into())
}
pub(super) fn resource_metadata_urls(endpoint: &str) -> Result<Vec<String>> {
    let url = https_url(endpoint)?;
    let origin = url.origin().ascii_serialization();
    let mut urls = vec![format!(
        "{origin}/.well-known/oauth-protected-resource{}",
        url.path()
    )];
    let root = format!("{origin}/.well-known/oauth-protected-resource");
    if !urls.contains(&root) {
        urls.push(root);
    }
    Ok(urls)
}
pub(super) fn authorization_metadata_urls(issuer: &str) -> Result<Vec<String>> {
    let url = https_url(issuer)?;
    if url.query().is_some() {
        return Err(invalid("OAuth issuer cannot have a query"));
    }
    let origin = url.origin().ascii_serialization();
    let path = url.path().trim_end_matches('/');
    if path.is_empty() {
        Ok(vec![
            format!("{origin}/.well-known/oauth-authorization-server"),
            format!("{origin}/.well-known/openid-configuration"),
        ])
    } else {
        Ok(vec![
            format!("{origin}/.well-known/oauth-authorization-server{path}"),
            format!("{origin}/.well-known/openid-configuration{path}"),
            format!("{}{}/.well-known/openid-configuration", origin, path),
        ])
    }
}
/// Bounded Bearer challenge parser. Quoted commas and escaped quotes are not
/// delimiters. Duplicate attributes fail closed instead of choosing an attacker value.
pub(super) fn parse_bearer_challenge(header: &str) -> Result<BTreeMap<String, String>> {
    if header.len() > 8192 {
        return Err(provider_error());
    }
    let mut pieces = Vec::new();
    let mut start = 0;
    let mut quoted = false;
    let mut escaped = false;
    for (offset, ch) in header.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if quoted && ch == '\\' {
            escaped = true;
        } else if ch == '"' {
            quoted = !quoted;
        } else if ch == ',' && !quoted {
            pieces.push(&header[start..offset]);
            start = offset + 1;
        }
    }
    if quoted || escaped {
        return Err(provider_error());
    }
    pieces.push(&header[start..]);
    let mut bearer = false;
    let mut fields = BTreeMap::new();
    for piece in pieces {
        let piece = piece.trim();
        let value = if piece
            .get(..7)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("Bearer "))
        {
            bearer = true;
            piece[7..].trim()
        } else if piece
            .split_once(' ')
            .is_some_and(|(first, _)| !first.contains('='))
        {
            bearer = false;
            continue;
        } else {
            piece
        };
        if !bearer || value.is_empty() {
            continue;
        }
        let (key, value) = value.split_once('=').ok_or_else(provider_error)?;
        let key = key.trim().to_ascii_lowercase();
        let raw = value.trim();
        let value = if raw.starts_with('"') && raw.ends_with('"') && raw.len() >= 2 {
            let mut result = String::new();
            let mut escaped = false;
            for ch in raw[1..raw.len() - 1].chars() {
                if escaped {
                    result.push(ch);
                    escaped = false;
                } else if ch == '\\' {
                    escaped = true;
                } else {
                    result.push(ch);
                }
            }
            result
        } else {
            raw.to_owned()
        };
        if fields.insert(key, value).is_some() {
            return Err(provider_error());
        }
    }
    Ok(fields)
}
/// Interpret an MCP authentication response without fetching tokens or expanding scope.
pub fn authorization_required(
    status: u16,
    www_authenticate: Option<&str>,
) -> Result<Option<McpAuthorizationRequired>> {
    let challenge = parse_bearer_challenge(www_authenticate.unwrap_or(""))?;
    if status == 401 {
        return Ok(Some(McpAuthorizationRequired::Reconnect));
    }
    if status == 403
        && challenge
            .get("error")
            .is_some_and(|v| v == "insufficient_scope")
    {
        let scopes = challenge
            .get("scope")
            .map(|v| v.split_whitespace().map(str::to_owned).collect::<Vec<_>>())
            .unwrap_or_default();
        validate_scopes(&scopes)?;
        return Ok(Some(McpAuthorizationRequired::StepUp { scopes }));
    }
    Ok(None)
}
