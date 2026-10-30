// SPDX-License-Identifier: AGPL-3.0-or-later
use super::*;
mod flows;
mod metadata;
mod provider_conformance;
mod start_races;
use kyomi_core::{db_execute, db_fetch_scalar};
use std::sync::{
    Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};

const KEY: [u8; 32] = [41; 32];
const RESOURCE: &str = "https://resource.example.com/mcp";
const ISSUER: &str = "https://accounts.example.com/tenant";
const ATTACKER_ISSUER: &str = "https://attacker.example.com/tenant";
const TOKEN: &str = "test-remote-access-token";
#[derive(Default)]
struct Provider {
    requests: Mutex<Vec<HttpRequest>>,
    token_calls: AtomicUsize,
    unsupported: AtomicBool,
    bad_issuer: AtomicBool,
    bad_resource: AtomicBool,
    malicious_endpoint: AtomicBool,
    over_scoped: AtomicBool,
    hold_token: AtomicBool,
    mix_up: AtomicBool,
    omit_issuer_support: AtomicBool,
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
}
fn response(status: u16, value: serde_json::Value) -> HttpResponse {
    HttpResponse {
        status,
        content_type: "application/json".into(),
        session_id: None,
        body: serde_json::to_vec(&value).unwrap(),
        www_authenticate: None,
        retry_after: None,
    }
}
#[async_trait::async_trait]
impl HttpTransport for Provider {
    async fn execute(
        &self,
        request: HttpRequest,
    ) -> std::result::Result<HttpResponse, kyomi_mcp_client::Error> {
        assert!(
            request.authorization.is_none(),
            "no login or other resource bearer may be forwarded"
        );
        let uri = Url::parse(&request.url).unwrap();
        let path = uri.path().to_owned();
        assert_eq!(uri.scheme(), "https");
        self.requests.lock().unwrap().push(request);
        Ok(match path.as_str() {
            "/mcp" => {
                let mut value = response(401, serde_json::json!({}));
                value.www_authenticate = Some("Bearer resource_metadata=\"https://resource.example.com/metadata\", scope=\"read write\"".into());
                value
            }
            "/metadata" => response(
                200,
                serde_json::json!({"resource":if self.bad_resource.load(Ordering::SeqCst) {"https://other.example.com/mcp"}else{RESOURCE},"authorization_servers":[if self.mix_up.load(Ordering::SeqCst) {ATTACKER_ISSUER}else{ISSUER}],"scopes_supported":["read","write"]}),
            ),
            "/.well-known/oauth-authorization-server/tenant" => response(
                200,
                serde_json::json!({"issuer":if self.bad_issuer.load(Ordering::SeqCst) {"https://other.example.com"}else if self.mix_up.load(Ordering::SeqCst) {ATTACKER_ISSUER}else{ISSUER},"authorization_endpoint":if self.malicious_endpoint.load(Ordering::SeqCst) {"http://other.example.com/authorize"}else{"https://accounts.example.com/authorize"},"token_endpoint":if self.mix_up.load(Ordering::SeqCst) {"https://attacker.example.com/steal-token"}else{"https://accounts.example.com/token"},"registration_endpoint":if self.unsupported.load(Ordering::SeqCst) {None}else{Some("https://accounts.example.com/register")},"response_types_supported":["code"],"code_challenge_methods_supported":["S256"],"token_endpoint_auth_methods_supported":["none"],"authorization_response_iss_parameter_supported":!self.omit_issuer_support.load(Ordering::SeqCst)}),
            ),
            "/register" => response(
                201,
                serde_json::json!({"client_id":"test-dcr-client","redirect_uris":["https://kyomi.example.com/api/v1/personal-mcp/oauth/callback"],"scope":"read"}),
            ),
            "/token" => {
                self.token_calls.fetch_add(1, Ordering::SeqCst);
                if self.hold_token.load(Ordering::SeqCst) {
                    self.entered.notify_one();
                    self.release.notified().await;
                }
                response(
                    200,
                    serde_json::json!({"access_token":TOKEN,"refresh_token":"test-remote-refresh-token","token_type":"Bearer","expires_in":3600,"scope":if self.over_scoped.load(Ordering::SeqCst) {"read write"}else{"read"}}),
                )
            }
            _ => response(404, serde_json::json!({})),
        })
    }
}
fn service(provider: Arc<Provider>) -> OAuthService {
    OAuthService::new(
        provider,
        OAuthSettings::new("https://kyomi.example.com").unwrap(),
    )
}
fn input(generation: i64) -> StartMcpOAuth {
    StartMcpOAuth {
        generation,
        issuer: ISSUER.into(),
        scopes: vec!["read".into()],
        consent: true,
        return_to: "/settings/profile?mcp=connected".into(),
    }
}
fn callback(start: &McpOAuthStarted) -> McpOAuthCallback {
    let url = Url::parse(&start.authorization_url).unwrap();
    McpOAuthCallback {
        state: url
            .query_pairs()
            .find(|(key, _)| key == "state")
            .unwrap()
            .1
            .into_owned(),
        code: Some("test-authorization-code".into()),
        iss: Some(ISSUER.into()),
        error: None,
    }
}
async fn new_connection(pool: &DbPool, actor: &str, name: &str) -> PersonalMcpConnection {
    store::create_connection(
        pool,
        actor,
        SavePersonalMcpConnection {
            name: name.into(),
            endpoint: RESOURCE.into(),
            auth_kind: McpAuthKind::Oauth,
            bearer_secret: None,
        },
        &KEY,
    )
    .await
    .unwrap()
}
async fn begin(
    service: &OAuthService,
    pool: &DbPool,
    actor: &str,
    name: &str,
) -> (PersonalMcpConnection, McpOAuthStarted) {
    let connection = new_connection(pool, actor, name).await;
    let started = service
        .start(
            pool,
            actor,
            &connection.connection_id,
            input(connection.generation),
            &KEY,
        )
        .await
        .unwrap();
    (connection, started)
}
