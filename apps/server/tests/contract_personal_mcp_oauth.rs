// SPDX-License-Identifier: AGPL-3.0-or-later
//! Actual production route handlers + auth extractor on both databases. No
//! sockets, browser, UI bundle, external provider or skipped Postgres coverage.
use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use kyomi_auth::{
    jwt,
    middleware::AuthState,
    personal_mcp_oauth::{OAuthService, OAuthSettings},
    personal_mcp_service,
};
use kyomi_core::{Config, DbPool, db_execute};
use kyomi_mcp_client::{HttpRequest, HttpResponse, HttpTransport};
use kyomi_server::routes::personal_mcp_oauth::{OAuthRouteState, routes};
use kyomi_types::personal_mcp::{McpAuthKind, SavePersonalMcpConnection};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};
use tower::ServiceExt;

const KEY: [u8; 32] = [43; 32];
const RESOURCE: &str = "https://resource.example.com/mcp";
const ISSUER: &str = "https://provider.example.com";
#[derive(Default)]
struct Provider {
    token_calls: AtomicUsize,
}
#[async_trait::async_trait]
impl HttpTransport for Provider {
    async fn execute(&self, request: HttpRequest) -> Result<HttpResponse, kyomi_mcp_client::Error> {
        assert!(
            request.authorization.is_none(),
            "Kyomi browser bearer must never reach the provider"
        );
        let path = url::Url::parse(&request.url).unwrap().path().to_owned();
        let (status,body,challenge)=match path.as_str() {
            "/mcp"=>(401,json!({}),Some("Bearer resource_metadata=\"https://resource.example.com/metadata\", scope=\"read\"".into())),
            "/metadata"=>(200,json!({"resource":RESOURCE,"authorization_servers":[ISSUER],"scopes_supported":["read"]}),None),
            "/.well-known/oauth-authorization-server"=>(200,json!({"issuer":ISSUER,"authorization_endpoint":"https://provider.example.com/authorize","token_endpoint":"https://provider.example.com/token","registration_endpoint":"https://provider.example.com/register","response_types_supported":["code"],"code_challenge_methods_supported":["S256"],"token_endpoint_auth_methods_supported":["none"],"authorization_response_iss_parameter_supported":true}),None),
            "/register"=>(201,json!({"client_id":"test-registered-client","token_endpoint_auth_method":"none"}),None),
            "/token"=> { self.token_calls.fetch_add(1,Ordering::SeqCst);let form=url::form_urlencoded::parse(&request.body).collect::<HashMap<_,_>>();assert_eq!(form.get("resource").unwrap(),RESOURCE);assert!(!form.contains_key("access_token"));(200,json!({"access_token":"test-outgoing-secret","token_type":"Bearer","expires_in":3600,"scope":"read"}),None) },
            _=>(404,json!({}),None)
        };
        Ok(HttpResponse {
            status,
            content_type: "application/json".into(),
            session_id: None,
            body: serde_json::to_vec(&body).unwrap(),
            www_authenticate: challenge,
            retry_after: None,
        })
    }
}
fn request(
    method: &str,
    path: &str,
    token: Option<&str>,
    body: Value,
    origin: Option<&str>,
) -> Request<Body> {
    let mut builder = Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json");
    if let Some(token) = token {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }
    if let Some(origin) = origin {
        builder = builder.header("origin", origin);
    }
    builder
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap()
}
fn completion_request(body: &str, token: Option<&str>, origin: &str) -> Request<Body> {
    let mut builder = Request::builder()
        .method("POST")
        .uri("/api/v1/personal-mcp/oauth/callback/complete")
        .header("content-type", "application/x-www-form-urlencoded")
        .header("origin", origin);
    if let Some(token) = token {
        builder = builder.header(
            "cookie",
            format!(
                "{}={token}",
                kyomi_core::constants::get().cookies.access_token_name
            ),
        );
    }
    builder.body(Body::from(body.to_owned())).unwrap()
}
async fn json_body(response: axum::response::Response) -> Value {
    serde_json::from_slice(&to_bytes(response.into_body(), 65536).await.unwrap()).unwrap()
}
async fn matrix(pool: DbPool) {
    let _ = kyomi_core::constants::load_with_fallback();
    let suffix = uuid::Uuid::new_v4();
    let owner = format!("oauth-route-a-{suffix}");
    let other = format!("oauth-route-b-{suffix}");
    for actor in [&owner, &other] {
        db_execute!(
            &pool,
            "INSERT INTO users (user_id,email) VALUES ($1,$2)",
            actor,
            format!("{actor}@example.com")
        )
        .unwrap();
    }
    let mut config = Config::test_config();
    config.personal_mcp_enabled = true;
    config.frontend_url = "https://kyomi.example.com".into();
    config.self_hosted = true;
    let auth = AuthState {
        jwt_secret: config.jwt_secret.clone(),
        db: pool.clone(),
        is_personal: false,
        self_hosted: true,
    };
    let owner_token =
        jwt::create_access_token_str(&owner, &config.jwt_secret, 15, HashMap::new()).unwrap();
    let other_token =
        jwt::create_access_token_str(&other, &config.jwt_secret, 15, HashMap::new()).unwrap();
    let provider = Arc::new(Provider::default());
    let mut settings = OAuthSettings::new("https://kyomi.example.com").unwrap();
    settings.client_metadata_url =
        Some("https://kyomi.example.com/api/v1/personal-mcp/oauth/client-metadata.json".into());
    let service = Arc::new(OAuthService::new(provider.clone(), settings));
    let route_state = OAuthRouteState {
        auth,
        config: Arc::new(config.clone()),
        key: Arc::new(KEY),
        service: Some(service),
    };
    // Mount at the same prefix as production: OriginalUri/auth audience checks
    // see the outbound route, never the inbound /mcp bearer-token resource.
    let app = axum::Router::new()
        .nest(
            "/api/v1/personal-mcp/oauth",
            routes::<()>(route_state.clone()),
        )
        .layer(axum::middleware::from_fn(
            kyomi_server::middleware::security_headers,
        ));
    let metadata = app
        .clone()
        .oneshot(request(
            "GET",
            "/api/v1/personal-mcp/oauth/client-metadata.json",
            None,
            json!(null),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(metadata.status(), StatusCode::OK);
    let metadata = json_body(metadata).await;
    assert_eq!(
        metadata["client_id"],
        "https://kyomi.example.com/api/v1/personal-mcp/oauth/client-metadata.json"
    );
    assert_eq!(
        metadata["redirect_uris"],
        json!(["https://kyomi.example.com/api/v1/personal-mcp/oauth/callback"])
    );
    assert!(metadata.get("client_secret").is_none());
    let connection = personal_mcp_service::create_connection(
        &pool,
        &owner,
        SavePersonalMcpConnection {
            name: "route account".into(),
            endpoint: RESOURCE.into(),
            auth_kind: McpAuthKind::Oauth,
            bearer_secret: None,
        },
        &KEY,
    )
    .await
    .unwrap();
    let prefix = format!("/api/v1/personal-mcp/oauth/{}", connection.connection_id);
    let input = json!({"generation":1,"issuer":ISSUER,"scopes":["read"],"consent":true,"return_to":"/settings/profile?mcp=connected"});
    let unauth = app
        .clone()
        .oneshot(request(
            "POST",
            &format!("{prefix}/start"),
            None,
            input.clone(),
            Some("https://kyomi.example.com"),
        ))
        .await
        .unwrap();
    assert_eq!(unauth.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(unauth.headers()["cache-control"], "no-store");
    let forbidden = app
        .clone()
        .oneshot(request(
            "POST",
            &format!("{prefix}/start"),
            Some(&owner_token),
            input.clone(),
            Some("https://evil.example.com"),
        ))
        .await
        .unwrap();
    assert_eq!(forbidden.status(), StatusCode::FORBIDDEN);
    let mut malicious = input.clone();
    malicious["return_to"] = json!("//evil.example.com");
    let rejected = app
        .clone()
        .oneshot(request(
            "POST",
            &format!("{prefix}/start"),
            Some(&owner_token),
            malicious,
            Some("https://kyomi.example.com"),
        ))
        .await
        .unwrap();
    assert_eq!(rejected.status(), StatusCode::BAD_REQUEST);
    let discovery = app
        .clone()
        .oneshot(request(
            "GET",
            &format!("{prefix}/discover"),
            Some(&owner_token),
            json!(null),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(discovery.status(), StatusCode::OK);
    let public = json_body(discovery).await;
    assert_eq!(public["registration"], "dynamic_registration");
    assert_eq!(public["scopes"], json!(["read"]));
    let started = app
        .clone()
        .oneshot(request(
            "POST",
            &format!("{prefix}/start"),
            Some(&owner_token),
            input,
            Some("https://kyomi.example.com"),
        ))
        .await
        .unwrap();
    assert_eq!(started.status(), StatusCode::OK);
    assert_eq!(started.headers()["cache-control"], "no-store");
    assert_eq!(started.headers()["referrer-policy"], "no-referrer");
    let body = json_body(started).await;
    assert_eq!(body["generation"], 2);
    let authorization = url::Url::parse(body["authorization_url"].as_str().unwrap()).unwrap();
    let state = authorization
        .query_pairs()
        .find(|(key, _)| key == "state")
        .unwrap()
        .1
        .into_owned();
    let query = url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs([
            ("state", state.as_str()),
            ("code", "test-provider-code"),
            ("iss", ISSUER),
        ])
        .finish();
    let callback = format!("/api/v1/personal-mcp/oauth/callback?{query}");
    let landing = app
        .clone()
        .oneshot(request("GET", &callback, None, json!(null), None))
        .await
        .unwrap();
    assert_eq!(landing.status(), StatusCode::OK);
    assert_eq!(landing.headers()["cache-control"], "no-store");
    assert_eq!(landing.headers()["referrer-policy"], "no-referrer");
    let csp = landing.headers()["content-security-policy"]
        .to_str()
        .unwrap();
    assert!(csp.contains("form-action 'self'"));
    assert!(csp.contains("default-src 'none'"));
    let html =
        String::from_utf8(to_bytes(landing.into_body(), 65536).await.unwrap().to_vec()).unwrap();
    assert!(html.contains("/api/v1/personal-mcp/oauth/callback/complete"));
    assert!(html.contains("method=\"post\""));
    assert!(html.contains("<title>Complete MCP connection · Kyomi</title>"));
    assert!(html.contains("auth-brand-panel"));
    assert!(html.contains("Complete connection"));
    assert!(html.contains("type=\"submit\""));
    assert!(!html.contains("data-ssr"));
    assert!(!html.contains("<script"));
    assert_eq!(provider.token_calls.load(Ordering::SeqCst), 0);
    let escaped_query = url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs([
            ("state", state.as_str()),
            ("code", "\"><script>alert(1)</script>"),
            ("iss", ISSUER),
        ])
        .finish();
    let escaped = app
        .clone()
        .oneshot(request(
            "GET",
            &format!("/api/v1/personal-mcp/oauth/callback?{escaped_query}"),
            None,
            json!(null),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(escaped.status(), StatusCode::OK);
    let escaped =
        String::from_utf8(to_bytes(escaped.into_body(), 65536).await.unwrap().to_vec()).unwrap();
    assert!(!escaped.contains("<script>"));
    assert!(escaped.contains("&lt;script&gt;"));
    let unauth = app
        .clone()
        .oneshot(completion_request(
            &query,
            None,
            "https://kyomi.example.com",
        ))
        .await
        .unwrap();
    assert_eq!(unauth.status(), StatusCode::UNAUTHORIZED);
    let wrong_user = app
        .clone()
        .oneshot(completion_request(
            &query,
            Some(&other_token),
            "https://kyomi.example.com",
        ))
        .await
        .unwrap();
    assert_eq!(wrong_user.status(), StatusCode::BAD_REQUEST);
    assert_eq!(provider.token_calls.load(Ordering::SeqCst), 0);
    let wrong_origin = app
        .clone()
        .oneshot(completion_request(
            &query,
            Some(&owner_token),
            "https://evil.example.com",
        ))
        .await
        .unwrap();
    assert_eq!(wrong_origin.status(), StatusCode::FORBIDDEN);
    let wrong_query = url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs([
            ("state", state.as_str()),
            ("code", "test-provider-code"),
            ("iss", "https://evil.example.com"),
        ])
        .finish();
    let wrong = app
        .clone()
        .oneshot(completion_request(
            &wrong_query,
            Some(&owner_token),
            "https://kyomi.example.com",
        ))
        .await
        .unwrap();
    assert_eq!(wrong.status(), StatusCode::BAD_REQUEST);
    assert_eq!(provider.token_calls.load(Ordering::SeqCst), 0);
    let success = app
        .clone()
        .oneshot(completion_request(
            &query,
            Some(&owner_token),
            "https://kyomi.example.com",
        ))
        .await
        .unwrap();
    assert_eq!(success.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        success.headers()["location"],
        "/settings/profile?mcp=connected"
    );
    assert_eq!(success.headers()["cache-control"], "no-store");
    assert_eq!(success.headers()["referrer-policy"], "no-referrer");
    assert!(
        to_bytes(success.into_body(), 65536)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(provider.token_calls.load(Ordering::SeqCst), 1);
    let replay = app
        .clone()
        .oneshot(completion_request(
            &query,
            Some(&owner_token),
            "https://kyomi.example.com",
        ))
        .await
        .unwrap();
    assert_eq!(replay.status(), StatusCode::BAD_REQUEST);
    let replay_body = to_bytes(replay.into_body(), 65536).await.unwrap();
    assert!(!String::from_utf8_lossy(&replay_body).contains("test-provider-code"));
    assert!(!String::from_utf8_lossy(&replay_body).contains("test-outgoing-secret"));
    assert_eq!(provider.token_calls.load(Ordering::SeqCst), 1);
    config.personal_mcp_enabled = false;
    let disabled = routes::<()>(OAuthRouteState {
        config: Arc::new(config),
        ..route_state
    });
    let denied = disabled
        .oneshot(request(
            "GET",
            &format!("/{}/discover", connection.connection_id),
            Some(&owner_token),
            json!(null),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(denied.status(), StatusCode::FORBIDDEN);
    db_execute!(
        &pool,
        "DELETE FROM user_mcp_connections WHERE owner_user_id=$1",
        &owner
    )
    .unwrap();
    for actor in [&owner, &other] {
        assert!(
            kyomi_auth::user_service::delete_user(&pool, actor)
                .await
                .unwrap()
        );
    }
}
#[tokio::test]
async fn personal_mcp_oauth_routes_sqlite() {
    matrix(DbPool::connect("sqlite::memory:").await.unwrap()).await;
}
#[tokio::test]
async fn personal_mcp_oauth_routes_postgres() {
    matrix(
        kyomi_core::test_db::connect_test_pool()
            .await
            .expect("Postgres is required for OAuth route parity"),
    )
    .await;
}
