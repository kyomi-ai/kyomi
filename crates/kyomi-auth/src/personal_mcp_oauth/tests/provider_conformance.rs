// SPDX-License-Identifier: AGPL-3.0-or-later
//! Multi-issuer mix-up regression and truthful local Trakkt-shaped compatibility.
use super::*;

/// Shape of local Trakkt 9efb657cf4dc1b7708c99d597522463900302125,
/// apps/server/src/routes/{mcp.rs:121,oauth.rs:105,152,174}. Hosts are synthetic.
struct TrakktShape {
    requests: Mutex<Vec<HttpRequest>>,
    corrected_resource_challenge: bool,
}
#[async_trait::async_trait]
impl HttpTransport for TrakktShape {
    async fn execute(
        &self,
        request: HttpRequest,
    ) -> std::result::Result<HttpResponse, kyomi_mcp_client::Error> {
        assert!(request.authorization.is_none());
        let path = Url::parse(&request.url).unwrap().path().to_owned();
        self.requests.lock().unwrap().push(request);
        Ok(match path.as_str() {
            "/mcp" => {
                let mut result = response(401, serde_json::json!({}));
                let suffix = if self.corrected_resource_challenge {
                    "/mcp"
                } else {
                    ""
                };
                result.www_authenticate = Some(format!(
                    "Bearer realm=\"OAuth\", resource_metadata=\"https://trakkt.example.com/.well-known/oauth-protected-resource{suffix}\", error=\"invalid_token\", error_description=\"Missing or invalid access token\""
                ));
                result
            }
            "/.well-known/oauth-protected-resource" => response(
                200,
                serde_json::json!({"resource":"https://trakkt.example.com","authorization_servers":["https://trakkt.example.com"],"scopes_supported":["mcp"],"bearer_methods_supported":["header"]}),
            ),
            "/.well-known/oauth-protected-resource/mcp" => response(
                200,
                serde_json::json!({"resource":"https://trakkt.example.com/mcp","authorization_servers":["https://trakkt.example.com"],"scopes_supported":["mcp"],"bearer_methods_supported":["header"]}),
            ),
            "/.well-known/oauth-authorization-server" => response(
                200,
                serde_json::json!({"issuer":"https://trakkt.example.com","authorization_endpoint":"https://trakkt.example.com/api/v1/oauth/authorize","token_endpoint":"https://trakkt.example.com/api/v1/oauth/token","registration_endpoint":"https://trakkt.example.com/api/v1/oauth/register","scopes_supported":["mcp"],"response_types_supported":["code"],"grant_types_supported":["authorization_code","refresh_token"],"token_endpoint_auth_methods_supported":["none"],"code_challenge_methods_supported":["S256"]}),
            ),
            _ => panic!("unsupported provider must stop before registration or token HTTP"),
        })
    }
}

async fn matrix(pool: DbPool) {
    let actor = format!("oauth-conf-{}", uuid::Uuid::new_v4());
    db_execute!(
        &pool,
        "INSERT INTO users (user_id,email) VALUES ($1,$2)",
        &actor,
        format!("{actor}@example.com")
    )
    .unwrap();
    let provider = Arc::new(Provider::default());
    provider.mix_up.store(true, Ordering::SeqCst);
    provider.omit_issuer_support.store(true, Ordering::SeqCst);
    let service = service(provider.clone());
    let connection = new_connection(&pool, &actor, "malicious issuer").await;
    let mut consent = input(1);
    consent.issuer = ATTACKER_ISSUER.into();
    let error = service
        .start(&pool, &actor, &connection.connection_id, consent, &KEY)
        .await
        .err()
        .unwrap();
    assert!(error.to_string().contains("RFC9207"));
    assert_eq!(
        db_fetch_scalar!(
            &pool,
            i64,
            "SELECT COUNT(*) FROM user_mcp_auth_transactions WHERE connection_id=$1",
            &connection.connection_id
        )
        .unwrap(),
        0
    );
    // Even an attacker claiming RFC9207 support cannot make an issuer-less
    // honest-AS response disclose its code/verifier to the attacker's token URL.
    provider.omit_issuer_support.store(false, Ordering::SeqCst);
    let mut consent = input(1);
    consent.issuer = ATTACKER_ISSUER.into();
    let started = service
        .start(&pool, &actor, &connection.connection_id, consent, &KEY)
        .await
        .unwrap();
    assert!(
        started
            .authorization_url
            .starts_with("https://accounts.example.com/authorize?")
    );
    for issuer in [None, Some(ISSUER.to_owned())] {
        let mut response = callback(&started);
        response.iss = issuer;
        assert!(
            service
                .callback(&pool, &actor, response, &KEY)
                .await
                .is_err()
        );
        assert_eq!(
            db_fetch_scalar!(
                &pool,
                i64,
                "SELECT COUNT(*) FROM user_mcp_auth_transactions WHERE connection_id=$1",
                &connection.connection_id
            )
            .unwrap(),
            1,
            "issuer is checked before state consumption"
        );
    }
    assert!(
        !provider
            .requests
            .lock()
            .unwrap()
            .iter()
            .any(|request| request.url == "https://attacker.example.com/steal-token")
    );
    assert_eq!(provider.token_calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        store::get_connection(&pool, &actor, &connection.connection_id)
            .await
            .unwrap()
            .status,
        "needs_auth"
    );
    for corrected in [false, true] {
        let provider = Arc::new(TrakktShape {
            requests: Mutex::new(vec![]),
            corrected_resource_challenge: corrected,
        });
        let service = OAuthService::new(
            provider.clone(),
            OAuthSettings::new("https://kyomi.example.com").unwrap(),
        );
        let connection = store::create_connection(
            &pool,
            &actor,
            SavePersonalMcpConnection {
                name: format!("Trakkt corrected resource {corrected}"),
                endpoint: "https://trakkt.example.com/mcp".into(),
                auth_kind: McpAuthKind::Oauth,
                bearer_secret: None,
            },
            &KEY,
        )
        .await
        .unwrap();
        let mut consent = input(1);
        consent.issuer = "https://trakkt.example.com".into();
        consent.scopes = vec!["mcp".into()];
        let error = service
            .start(&pool, &actor, &connection.connection_id, consent, &KEY)
            .await
            .err()
            .unwrap();
        assert!(error.to_string().contains(if corrected {
            "RFC9207"
        } else {
            "does not match"
        }));
        assert_eq!(
            store::get_connection(&pool, &actor, &connection.connection_id)
                .await
                .unwrap()
                .status,
            "needs_auth"
        );
        assert_eq!(
            db_fetch_scalar!(
                &pool,
                i64,
                "SELECT COUNT(*) FROM user_mcp_auth_transactions WHERE connection_id=$1",
                &connection.connection_id
            )
            .unwrap(),
            0
        );
        let requests = provider.requests.lock().unwrap();
        assert!(
            requests
                .iter()
                .all(|request| request.method == reqwest::Method::GET)
        );
        assert_eq!(requests.len(), if corrected { 3 } else { 2 });
    }
    db_execute!(
        &pool,
        "DELETE FROM user_mcp_connections WHERE owner_user_id=$1",
        &actor
    )
    .unwrap();
    assert!(
        crate::user_service::delete_user(&pool, &actor)
            .await
            .unwrap()
    );
}
#[tokio::test]
async fn personal_mcp_oauth_sqlite_mix_up_and_trakkt_conformance() {
    matrix(crate::test_support::test_pool().await).await;
}
#[tokio::test]
async fn personal_mcp_oauth_postgres_mix_up_and_trakkt_conformance() {
    matrix(
        kyomi_core::test_db::connect_test_pool()
            .await
            .expect("Postgres is required for OAuth conformance"),
    )
    .await;
}
