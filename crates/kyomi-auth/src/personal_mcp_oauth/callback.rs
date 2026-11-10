// SPDX-License-Identifier: AGPL-3.0-or-later
//! Single-use callback consumption and code exchange. No database lock crosses HTTP.
use super::*;

#[derive(sqlx::FromRow)]
struct TransactionContext {
    connection_id: String,
    generation: i64,
    issuer: String,
    resource: String,
    redirect_uri: String,
}
#[derive(Serialize, Deserialize)]
pub(super) struct Pending {
    pub(super) verifier: String,
    pub(super) token_endpoint: String,
    pub(super) client_id: String,
    pub(super) client_secret: Option<String>,
    pub(super) auth_method: String,
    pub(super) return_to: String,
    pub(super) scopes: Vec<String>,
}
#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    token_type: String,
    refresh_token: Option<String>,
    scope: Option<String>,
    expires_in: Option<i64>,
}

impl OAuthService {
    /// Authenticated owner + opaque state select persisted context. No browser
    /// issuer/resource/generation is trusted to construct the token exchange.
    pub async fn callback(
        &self,
        pool: &DbPool,
        actor: &str,
        query: McpOAuthCallback,
        key: &[u8; 32],
    ) -> Result<String> {
        self.bounded(self.callback_inner(pool, actor, query, key))
            .await
    }

    async fn callback_inner(
        &self,
        pool: &DbPool,
        actor: &str,
        query: McpOAuthCallback,
        key: &[u8; 32],
    ) -> Result<String> {
        if query.state.len() < 32 || query.state.len() > 1024 {
            return Err(invalid("OAuth state is invalid or expired; reconnect"));
        }
        let hash = format!("{:x}", Sha256::digest(query.state.as_bytes()));
        let context = db_fetch_optional!(
            pool,
            TransactionContext,
            "SELECT t.connection_id,t.generation,t.issuer,t.resource,t.redirect_uri FROM user_mcp_auth_transactions t JOIN user_mcp_connections c ON c.connection_id=t.connection_id JOIN users u ON u.user_id=c.owner_user_id WHERE t.state_hash=$1 AND c.owner_user_id=$2 AND u.active=TRUE AND c.generation=t.generation AND c.endpoint=t.endpoint AND c.status!='disabled' AND t.expires_at>$3",
            hash,
            actor,
            chrono::Utc::now().timestamp()
        )?
        .ok_or_else(|| invalid("OAuth state is invalid or expired; reconnect"))?;
        // Mandatory even for older persisted payloads and error responses:
        // a malicious issuer may pair an honest authorization endpoint with
        // its own token endpoint to steal the code AND PKCE verifier.
        if query.iss.as_deref() != Some(context.issuer.as_str())
            || context.redirect_uri != self.settings.redirect_uri()
        {
            return Err(invalid("OAuth callback issuer mismatch"));
        }
        let payload = store::consume_auth_transaction(
            pool,
            actor,
            &context.connection_id,
            context.generation,
            McpAuthContext {
                state: &query.state,
                issuer: &context.issuer,
                resource: &context.resource,
                redirect_uri: &context.redirect_uri,
            },
            key,
        )
        .await?;
        let pending: Pending =
            serde_json::from_str(payload.expose()).map_err(|_| provider_error())?;
        if query.error.is_some() {
            return Err(invalid(
                "OAuth authorization was denied; no account was connected",
            ));
        }
        let code = query
            .code
            .filter(|value| !value.is_empty() && value.len() <= 8192)
            .ok_or_else(|| invalid("OAuth callback is missing an authorization code"))?;
        let (body, authorization) = token_request(&pending, &context, &code)?;
        let response = self
            .request(
                reqwest::Method::POST,
                &pending.token_endpoint,
                body,
                Some("application/x-www-form-urlencoded"),
                authorization,
            )
            .await?;
        if response.status != 200 {
            return Err(invalid(
                "OAuth code exchange was rejected; reconnect explicitly",
            ));
        }
        let token: TokenResponse =
            serde_json::from_slice(&response.body).map_err(|_| provider_error())?;
        if !token.token_type.eq_ignore_ascii_case("bearer")
            || !valid_token(&token.access_token)
            || token
                .refresh_token
                .as_ref()
                .is_some_and(|value| !valid_token(value))
        {
            return Err(provider_error());
        }
        let scopes = token
            .scope
            .as_deref()
            .map(|value| {
                value
                    .split_whitespace()
                    .map(str::to_owned)
                    .collect::<Vec<_>>()
            })
            .unwrap_or(pending.scopes.clone());
        if scopes.iter().any(|scope| !pending.scopes.contains(scope)) {
            return Err(invalid("Provider returned unconsented OAuth scopes"));
        }
        let expires_at = match token.expires_in {
            Some(value) if value > 0 && value <= 31_536_000 => {
                Some(chrono::Utc::now().timestamp() + value)
            }
            Some(_) => return Err(provider_error()),
            None => None,
        };
        let credential = Credential {
            access_token: token.access_token,
            refresh_token: token.refresh_token,
            issuer: context.issuer,
            resource: context.resource,
            token_endpoint: pending.token_endpoint,
            client_id: pending.client_id,
            client_secret: pending.client_secret,
            auth_method: pending.auth_method,
            scopes,
            expires_at,
            account_revision: uuid::Uuid::new_v4().to_string(),
        };
        let encoded = serde_json::to_string(&credential).map_err(|_| provider_error())?;
        store::commit_oauth_secret(
            pool,
            actor,
            &context.connection_id,
            context.generation,
            &encoded,
            key,
        )
        .await?;
        safe_return_path(&pending.return_to)
    }
}

/// All form values are encoded once. Basic authentication also applies the
/// OAuth form encoding to each credential before base64, per RFC6749 2.3.1.
fn token_request(
    pending: &Pending,
    context: &TransactionContext,
    code: &str,
) -> Result<(Vec<u8>, Option<String>)> {
    let mut form = url::form_urlencoded::Serializer::new(String::new());
    form.extend_pairs([
        ("grant_type", "authorization_code"),
        ("code", code),
        ("redirect_uri", &context.redirect_uri),
        ("code_verifier", &pending.verifier),
        ("resource", &context.resource),
    ]);
    let authorization = match pending.auth_method.as_str() {
        "none" => {
            form.append_pair("client_id", &pending.client_id);
            None
        }
        "client_secret_post" => {
            form.append_pair("client_id", &pending.client_id);
            form.append_pair(
                "client_secret",
                pending
                    .client_secret
                    .as_deref()
                    .ok_or_else(provider_error)?,
            );
            None
        }
        "client_secret_basic" => {
            use base64::engine::general_purpose::STANDARD;
            let encode = |value: &str| {
                url::form_urlencoded::Serializer::new(String::new())
                    .append_pair("", value)
                    .finish()
                    .trim_start_matches('=')
                    .to_owned()
            };
            Some(format!(
                "Basic {}",
                STANDARD.encode(format!(
                    "{}:{}",
                    encode(&pending.client_id),
                    encode(
                        pending
                            .client_secret
                            .as_deref()
                            .ok_or_else(provider_error)?
                    )
                ))
            ))
        }
        _ => return Err(provider_error()),
    };
    Ok((form.finish().into_bytes(), authorization))
}
