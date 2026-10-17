// SPDX-License-Identifier: AGPL-3.0-or-later
//! Outbound OAuth orchestration, shared by HTTP and server-function callers.
//! No inbound issuer/login grants, provider enum, workspace ownership shortcut,
//! remote error text, credential logging, automatic escalation or mutation retry.
mod callback;
mod credentials;
use callback::Pending;
mod metadata;
use credentials::{Credential, valid_token};
pub use credentials::{account_revision, authorization_from_secret};
use metadata::*;
pub use metadata::{authorization_required, safe_return_path};

use crate::personal_mcp_service::{self as store, McpAuthContext, McpAuthTransaction, McpSecret};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use kyomi_core::{DbPool, Error, Result, db_fetch_optional};
use kyomi_mcp_client::{
    DeploymentPolicy, HttpRequest, HttpResponse, HttpTransport, Limits, SafeHttpClient,
};
use kyomi_types::personal_mcp::{McpAuthKind, PersonalMcpConnection, SavePersonalMcpConnection};
use kyomi_types::personal_mcp_oauth::{
    McpAuthorizationRequired, McpOAuthCallback, McpOAuthDiscovery, McpOAuthStarted, StartMcpOAuth,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, sync::Arc, time::Duration};
use url::Url;

const MAX_DOCUMENT: usize = 64 * 1024;
const TTL: i64 = 600;
const CALLBACK_PREFIX: &str = "/api/v1/personal-mcp/oauth/";

fn invalid(message: &'static str) -> Error {
    Error::BadRequest(message.into())
}
fn provider_error() -> Error {
    invalid("OAuth provider response is invalid; reconnect or configure a supported client")
}

/// Trusted deployment configuration only. Never deserialize this from a browser.
/// Deliberately no Debug: registered client credentials may be confidential.
#[derive(Clone, Deserialize)]
pub struct RegisteredClient {
    pub client_id: String,
    pub client_secret: Option<String>,
    /// `none`, `client_secret_post`, or `client_secret_basic`.
    pub token_endpoint_auth_method: String,
}
#[derive(Clone)]
pub struct OAuthSettings {
    pub public_origin: String,
    /// Exact issuer keys. Credentials are never reused for a different issuer.
    pub registered_clients: BTreeMap<String, RegisteredClient>,
    /// HTTPS metadata document owned by this deployment, or None to use DCR.
    pub client_metadata_url: Option<String>,
}
impl OAuthSettings {
    pub fn new(public_origin: &str) -> Result<Self> {
        let url = https_url(public_origin)?;
        if url.path() != "/" || url.query().is_some() {
            return Err(invalid("OAuth public origin must be an HTTPS origin"));
        }
        Ok(Self {
            public_origin: url.origin().ascii_serialization(),
            registered_clients: BTreeMap::new(),
            client_metadata_url: None,
        })
    }
    /// Read only trusted operator settings, never request data. Parsing errors
    /// deliberately omit values because the JSON can include client secrets.
    pub fn from_deployment(public_origin: &str) -> Result<Self> {
        let mut settings = Self::new(public_origin)?;
        settings.client_metadata_url = Some(format!(
            "{}{CALLBACK_PREFIX}client-metadata.json",
            settings.public_origin
        ));
        if let Ok(value) = std::env::var("PERSONAL_MCP_OAUTH_CLIENTS_JSON") {
            settings.registered_clients = serde_json::from_str(&value)
                .map_err(|_| invalid("PERSONAL_MCP_OAUTH_CLIENTS_JSON is invalid"))?;
        }
        Ok(settings)
    }
    pub fn metadata_document(&self) -> serde_json::Value {
        serde_json::json!({"client_id": self.client_metadata_url, "client_name":"Kyomi personal MCP", "redirect_uris": [format!("{}{CALLBACK_PREFIX}callback", self.public_origin)], "grant_types":["authorization_code","refresh_token"], "response_types":["code"], "token_endpoint_auth_method":"none"})
    }
    fn redirect_uri(&self) -> String {
        format!("{}{CALLBACK_PREFIX}callback", self.public_origin)
    }
}

/// One bounded operation includes discovery, registration and all persisted state work.
#[derive(Clone)]
pub struct OAuthService {
    http: Arc<dyn HttpTransport>,
    settings: OAuthSettings,
    deadline: Duration,
}
impl OAuthService {
    pub fn new(http: Arc<dyn HttpTransport>, settings: OAuthSettings) -> Self {
        Self {
            http,
            settings,
            deadline: Duration::from_secs(30),
        }
    }
    pub fn production(settings: OAuthSettings) -> Result<Self> {
        let limits = Limits {
            body_bytes: MAX_DOCUMENT,
            request_bytes: MAX_DOCUMENT,
            ..Limits::default()
        };
        let http = SafeHttpClient::new(limits, DeploymentPolicy::default())
            .map_err(|_| invalid("OAuth safe transport configuration is invalid"))?;
        Ok(Self::new(Arc::new(http), settings))
    }
    pub fn settings(&self) -> &OAuthSettings {
        &self.settings
    }
    async fn bounded<T>(&self, future: impl std::future::Future<Output = Result<T>>) -> Result<T> {
        tokio::time::timeout(self.deadline, future)
            .await
            .map_err(|_| invalid("OAuth operation timed out; reconnect explicitly"))?
    }
    async fn request(
        &self,
        method: reqwest::Method,
        url: &str,
        body: Vec<u8>,
        content_type: Option<&str>,
        authorization: Option<String>,
    ) -> Result<HttpResponse> {
        https_url(url)?;
        let response = self
            .http
            .execute(HttpRequest {
                method,
                url: url.into(),
                authorization,
                content_type: content_type.map(str::to_owned),
                body,
                mcp_headers: vec![],
            })
            .await
            .map_err(|_| {
                invalid(
                    "OAuth safe egress failed; check provider URL and deployment network policy",
                )
            })?;
        if response.body.len() > MAX_DOCUMENT {
            return Err(provider_error());
        }
        Ok(response)
    }
    async fn document<T: serde::de::DeserializeOwned>(&self, url: &str) -> Result<Option<T>> {
        let response = self
            .request(reqwest::Method::GET, url, vec![], None, None)
            .await?;
        if response.status == 404 {
            return Ok(None);
        }
        if response.status != 200
            || !(response.content_type.split(';').next() == Some("application/json"))
        {
            return Err(provider_error());
        }
        serde_json::from_slice(&response.body)
            .map(Some)
            .map_err(|_| provider_error())
    }
    pub async fn discover(
        &self,
        pool: &DbPool,
        actor: &str,
        id: &str,
    ) -> Result<McpOAuthDiscovery> {
        self.bounded(async {
            let connection = oauth_connection(pool, actor, id).await?;
            let discovered = self.discover_endpoint(&connection.endpoint).await?;
            Ok(discovered.public(self.registration_kind(&discovered.server)?))
        })
        .await
    }
    async fn discover_endpoint(&self, endpoint: &str) -> Result<Discovery> {
        // No Kyomi authentication or saved remote token is sent during discovery.
        let probe = self
            .request(reqwest::Method::GET, endpoint, vec![], None, None)
            .await?;
        if !matches!(probe.status, 200 | 401 | 403 | 405 | 406) {
            return Err(provider_error());
        }
        let challenge = parse_bearer_challenge(probe.www_authenticate.as_deref().unwrap_or(""))?;
        let urls = if let Some(url) = challenge.get("resource_metadata") {
            vec![https_url(url)?.to_string()]
        } else {
            resource_metadata_urls(endpoint)?
        };
        let mut resource = None;
        for url in urls {
            if let Some(value) = self.document::<ResourceMetadata>(&url).await? {
                resource = Some(value);
                break;
            }
        }
        let resource = resource.ok_or_else(|| invalid("Provider has no protected-resource metadata; configure OAuth metadata on the MCP server"))?;
        if resource.resource != endpoint
            || resource.authorization_servers.is_empty()
            || resource.authorization_servers.len() > 16
        {
            return Err(invalid(
                "OAuth resource metadata does not match this MCP endpoint",
            ));
        }
        // Deterministic first advertised issuer; consent binds that exact issuer.
        let issuer = &resource.authorization_servers[0];
        https_url(issuer)?;
        let mut server = None;
        for url in authorization_metadata_urls(issuer)? {
            if let Some(value) = self.document::<AuthorizationMetadata>(&url).await? {
                server = Some(value);
                break;
            }
        }
        let server =
            server.ok_or_else(|| invalid("Authorization server metadata was not found"))?;
        if server.issuer != *issuer {
            return Err(invalid("OAuth authorization server issuer mismatch"));
        }
        https_url(&server.authorization_endpoint)?;
        https_url(&server.token_endpoint)?;
        if !server.response_types_supported.iter().any(|v| v == "code")
            || !server
                .code_challenge_methods_supported
                .iter()
                .any(|v| v == "S256")
        {
            return Err(invalid(
                "Provider must support authorization code with PKCE S256",
            ));
        }
        // A shared redirect needs response issuer identification (RFC9700
        // 4.4.2 (issuer identification)). State and PKCE do not identify the authorization server.
        if !server.authorization_response_iss_parameter_supported {
            return Err(invalid(
                "Provider must advertise RFC9207 authorization response issuer identification; ask the provider to enable it",
            ));
        }
        if let Some(url) = &server.registration_endpoint {
            https_url(url)?;
        }
        let scopes = if let Some(scope) = challenge.get("scope") {
            scope.split_whitespace().map(str::to_owned).collect()
        } else {
            resource.scopes_supported
        };
        validate_scopes(&scopes)?;
        Ok(Discovery {
            resource: resource.resource,
            scopes,
            server,
        })
    }
    fn registration_kind(&self, server: &AuthorizationMetadata) -> Result<&'static str> {
        if self
            .settings
            .registered_clients
            .contains_key(&server.issuer)
        {
            return Ok("pre_registered");
        }
        if server.client_id_metadata_document_supported
            && self.settings.client_metadata_url.is_some()
        {
            return Ok("client_metadata_document");
        }
        if server.registration_endpoint.is_some() {
            return Ok("dynamic_registration");
        }
        Err(invalid(
            "Provider supports neither configured client, client metadata documents nor dynamic registration; ask an administrator to pre-register Kyomi",
        ))
    }
    async fn client(&self, server: &AuthorizationMetadata) -> Result<RegisteredClient> {
        match self.registration_kind(server)? {
            "pre_registered" => {
                let client = self
                    .settings
                    .registered_clients
                    .get(&server.issuer)
                    .ok_or_else(provider_error)?
                    .clone();
                validate_client(&client, server)?;
                Ok(client)
            }
            "client_metadata_document" => {
                let client_id = self
                    .settings
                    .client_metadata_url
                    .clone()
                    .ok_or_else(provider_error)?;
                let url = https_url(&client_id)?;
                if url.path() == "/"
                    || url.origin().ascii_serialization() != self.settings.public_origin
                {
                    return Err(invalid(
                        "Client metadata document must be hosted by this deployment",
                    ));
                }
                let client = RegisteredClient {
                    client_id,
                    client_secret: None,
                    token_endpoint_auth_method: "none".into(),
                };
                validate_client(&client, server)?;
                Ok(client)
            }
            _ => {
                let endpoint = server
                    .registration_endpoint
                    .as_deref()
                    .ok_or_else(provider_error)?;
                let body=serde_json::to_vec(&serde_json::json!({"client_name":"Kyomi personal MCP","redirect_uris":[self.settings.redirect_uri()],"grant_types":["authorization_code","refresh_token"],"response_types":["code"],"token_endpoint_auth_method":"none"})).map_err(|_| provider_error())?;
                let response = self
                    .request(
                        reqwest::Method::POST,
                        endpoint,
                        body,
                        Some("application/json"),
                        None,
                    )
                    .await?;
                if !matches!(response.status, 200 | 201) {
                    return Err(invalid(
                        "Dynamic client registration was rejected; configure a pre-registered client",
                    ));
                }
                let registered: DcrResponse =
                    serde_json::from_slice(&response.body).map_err(|_| provider_error())?;
                if registered
                    .redirect_uris
                    .as_ref()
                    .is_some_and(|v| v != &[self.settings.redirect_uri()])
                {
                    return Err(provider_error());
                }
                let client = RegisteredClient {
                    client_id: registered.client_id,
                    client_secret: registered.client_secret,
                    token_endpoint_auth_method: registered
                        .token_endpoint_auth_method
                        .unwrap_or_else(|| "none".into()),
                };
                validate_client(&client, server)?;
                Ok(client)
            }
        }
    }
    pub async fn start(
        &self,
        pool: &DbPool,
        actor: &str,
        id: &str,
        input: StartMcpOAuth,
        key: &[u8; 32],
    ) -> Result<McpOAuthStarted> {
        self.bounded(async {
            if !input.consent {
                return Err(invalid("Explicit OAuth consent is required"));
            }
            let return_to = safe_return_path(&input.return_to)?;
            validate_scopes(&input.scopes)?;
            let connection = oauth_connection(pool, actor, id).await?;
            if connection.generation != input.generation {
                return Err(Error::Conflict(
                    "MCP connection changed; reload before authorizing".into(),
                ));
            }
            let discovered = self.discover_endpoint(&connection.endpoint).await?;
            if discovered.server.issuer != input.issuer {
                return Err(invalid("OAuth issuer changed; review consent again"));
            }
            if (input.scopes.is_empty() && !discovered.scopes.is_empty())
                || input.scopes.iter().any(|s| !discovered.scopes.contains(s))
            {
                return Err(invalid(
                    "Requested OAuth scope was not offered for explicit consent",
                ));
            }
            let client = self.client(&discovered.server).await?;
            // CAS before storing new state invalidates even a previous callback
            // which already consumed its state and is waiting at the token endpoint.
            let connection = store::update_connection(
                pool,
                actor,
                id,
                input.generation,
                SavePersonalMcpConnection {
                    name: connection.name,
                    endpoint: connection.endpoint,
                    auth_kind: McpAuthKind::Oauth,
                    bearer_secret: None,
                },
                key,
            )
            .await?;
            let state = random_secret();
            let verifier = random_secret();
            let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
            let redirect_uri = self.settings.redirect_uri();
            let mut authorization = https_url(&discovered.server.authorization_endpoint)?;
            if authorization.query_pairs().any(|(key, _)| {
                matches!(
                    key.as_ref(),
                    "client_id"
                        | "state"
                        | "redirect_uri"
                        | "response_type"
                        | "code_challenge"
                        | "code_challenge_method"
                        | "resource"
                        | "scope"
                )
            }) {
                return Err(provider_error());
            }
            authorization.query_pairs_mut().extend_pairs([
                ("response_type", "code"),
                ("client_id", &client.client_id),
                ("redirect_uri", &redirect_uri),
                ("state", &state),
                ("code_challenge", &challenge),
                ("code_challenge_method", "S256"),
                ("resource", &discovered.resource),
                ("scope", &input.scopes.join(" ")),
            ]);
            let payload = Pending {
                verifier,
                token_endpoint: discovered.server.token_endpoint,
                client_id: client.client_id,
                client_secret: client.client_secret,
                auth_method: client.token_endpoint_auth_method,
                return_to,
                scopes: input.scopes,
            };
            let payload = serde_json::to_string(&payload).map_err(|_| provider_error())?;
            store::store_auth_transaction(
                pool,
                actor,
                id,
                connection.generation,
                McpAuthTransaction {
                    state: &state,
                    issuer: &discovered.server.issuer,
                    resource: &discovered.resource,
                    redirect_uri: &redirect_uri,
                    payload: &payload,
                    expires_at: chrono::Utc::now().timestamp() + TTL,
                },
                key,
            )
            .await?;
            Ok(McpOAuthStarted {
                authorization_url: authorization.to_string(),
                generation: connection.generation,
            })
        })
        .await
    }
}
async fn oauth_connection(pool: &DbPool, actor: &str, id: &str) -> Result<PersonalMcpConnection> {
    let connection = store::get_connection(pool, actor, id).await?;
    if connection.auth_kind != McpAuthKind::Oauth || connection.status == "disabled" {
        return Err(invalid("This connection does not accept an OAuth flow"));
    }
    Ok(connection)
}
fn random_secret() -> String {
    use rand::RngCore;
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}
#[cfg(test)]
mod tests;
