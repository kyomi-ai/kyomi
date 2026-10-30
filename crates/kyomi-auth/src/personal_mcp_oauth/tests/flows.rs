// SPDX-License-Identifier: AGPL-3.0-or-later
use super::*;
async fn matrix(pool: DbPool) {
    let suffix = uuid::Uuid::new_v4();
    let owner = format!("oauth-owner-{suffix}");
    let other = format!("oauth-other-{suffix}");
    for actor in [&owner, &other] {
        db_execute!(
            &pool,
            "INSERT INTO users (user_id,email) VALUES ($1,$2)",
            actor,
            format!("{actor}@example.com")
        )
        .unwrap();
    }
    let provider = Arc::new(Provider::default());
    let service = service(provider.clone());
    let (connection, started) = begin(&service, &pool, &owner, "success").await;
    let authorization = Url::parse(&started.authorization_url).unwrap();
    let fields = authorization.query_pairs().collect::<BTreeMap<_, _>>();
    assert_eq!(fields.get("resource").unwrap(), RESOURCE);
    assert_eq!(fields.get("scope").unwrap(), "read");
    assert_eq!(fields.get("code_challenge_method").unwrap(), "S256");
    assert_eq!(started.generation, 2);
    let encrypted: String = db_fetch_scalar!(
        &pool,
        String,
        "SELECT encrypted_payload FROM user_mcp_auth_transactions WHERE connection_id=$1",
        &connection.connection_id
    )
    .unwrap();
    assert!(!encrypted.contains("verifier"));
    assert!(!encrypted.contains("test-dcr-client"));
    let pending: Pending =
        serde_json::from_str(&crate::encryption::decrypt(&encrypted, &KEY).unwrap()).unwrap();
    assert_eq!(
        fields.get("code_challenge").unwrap().as_ref(),
        URL_SAFE_NO_PAD.encode(Sha256::digest(pending.verifier.as_bytes()))
    );
    assert_ne!(pending.verifier, fields.get("state").unwrap().as_ref());
    // Wrong browser login and mix-up never consume another owner's state or exchange.
    assert!(
        service
            .callback(&pool, &other, callback(&started), &KEY)
            .await
            .is_err()
    );
    let mut wrong = callback(&started);
    wrong.iss = Some("https://other.example.com/tenant".into());
    assert!(service.callback(&pool, &owner, wrong, &KEY).await.is_err());
    assert_eq!(provider.token_calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        service
            .callback(&pool, &owner, callback(&started), &KEY)
            .await
            .unwrap(),
        "/settings/profile?mcp=connected"
    );
    assert!(
        service
            .callback(&pool, &owner, callback(&started), &KEY)
            .await
            .is_err()
    );
    assert_eq!(provider.token_calls.load(Ordering::SeqCst), 1);
    let ready = store::get_connection(&pool, &owner, &connection.connection_id)
        .await
        .unwrap();
    assert_eq!(ready.status, "ready");
    assert_eq!(ready.generation, 3);
    let encoded: String = db_fetch_scalar!(
        &pool,
        String,
        "SELECT encrypted_secret FROM user_mcp_secrets WHERE connection_id=$1",
        &connection.connection_id
    )
    .unwrap();
    assert!(!encoded.contains(TOKEN));
    let credential: Credential =
        serde_json::from_str(&crate::encryption::decrypt(&encoded, &KEY).unwrap()).unwrap();
    assert_eq!(credential.resource, RESOURCE);
    assert_eq!(credential.scopes, vec!["read"]);
    assert!(uuid::Uuid::parse_str(&credential.account_revision).is_ok());
    let public = serde_json::to_string(&ready).unwrap();
    assert!(!public.contains(TOKEN));
    assert!(!public.contains("refresh_token"));
    {
        let requests = provider.requests.lock().unwrap();
        let request = requests.iter().find(|r| r.url.ends_with("/token")).unwrap();
        let form = url::form_urlencoded::parse(&request.body).collect::<BTreeMap<_, _>>();
        assert_eq!(form.get("resource").unwrap(), RESOURCE);
        assert_eq!(
            form.get("code_verifier").unwrap().as_ref(),
            pending.verifier
        );
        assert_eq!(
            request.content_type.as_deref(),
            Some("application/x-www-form-urlencoded")
        );
        assert!(!form.contains_key("scope"));
    }
    for scenario in [
        "denied",
        "missing-issuer",
        "expired",
        "edited",
        "disabled",
        "over-scoped",
    ] {
        let (connection, started) = begin(&service, &pool, &owner, scenario).await;
        let before = provider.token_calls.load(Ordering::SeqCst);
        let mut query = callback(&started);
        match scenario {
            "denied" => {
                query.error = Some("access_denied with remote secret".into());
                query.code = None;
            }
            "missing-issuer" => query.iss = None,
            "expired" => {
                db_execute!(
                    &pool,
                    "UPDATE user_mcp_auth_transactions SET expires_at=$1 WHERE connection_id=$2",
                    chrono::Utc::now().timestamp() - 1,
                    &connection.connection_id
                )
                .unwrap();
            }
            "edited" => {
                store::update_connection(
                    &pool,
                    &owner,
                    &connection.connection_id,
                    started.generation,
                    SavePersonalMcpConnection {
                        name: connection.name,
                        endpoint: "https://resource.example.com/edited".into(),
                        auth_kind: McpAuthKind::Oauth,
                        bearer_secret: None,
                    },
                    &KEY,
                )
                .await
                .unwrap();
            }
            "disabled" => {
                store::disable_connection(
                    &pool,
                    &owner,
                    &connection.connection_id,
                    started.generation,
                )
                .await
                .unwrap();
            }
            "over-scoped" => provider.over_scoped.store(true, Ordering::SeqCst),
            _ => unreachable!(),
        }
        let error = service
            .callback(&pool, &owner, query, &KEY)
            .await
            .unwrap_err();
        assert!(!error.to_string().contains("remote secret"));
        assert_eq!(
            provider.token_calls.load(Ordering::SeqCst),
            before + usize::from(scenario == "over-scoped")
        );
        assert_ne!(
            store::get_connection(&pool, &owner, &connection.connection_id)
                .await
                .unwrap()
                .status,
            "ready"
        );
        provider.over_scoped.store(false, Ordering::SeqCst);
    }
    // A consumed old callback cannot commit after reconnect creates new state.
    let (race, old) = begin(&service, &pool, &owner, "reconnect-race").await;
    provider.hold_token.store(true, Ordering::SeqCst);
    let future = service.callback(&pool, &owner, callback(&old), &KEY);
    tokio::pin!(future);
    tokio::select! {result=&mut future=>panic!("callback unexpectedly completed: {}",result.is_ok()),_=provider.entered.notified()=>{}}
    let replacement = service
        .start(
            &pool,
            &owner,
            &race.connection_id,
            input(old.generation),
            &KEY,
        )
        .await
        .unwrap();
    provider.release.notify_one();
    assert!(future.await.is_err());
    assert_eq!(
        store::get_connection(&pool, &owner, &race.connection_id)
            .await
            .unwrap()
            .status,
        "needs_auth"
    );
    provider.hold_token.store(false, Ordering::SeqCst);
    assert!(
        service
            .callback(&pool, &owner, callback(&replacement), &KEY)
            .await
            .is_ok()
    );
    let (_, double) = begin(&service, &pool, &owner, "concurrent-callback").await;
    let before = provider.token_calls.load(Ordering::SeqCst);
    let (first, second) = tokio::join!(
        service.callback(&pool, &owner, callback(&double), &KEY),
        service.callback(&pool, &owner, callback(&double), &KEY),
    );
    assert_ne!(first.is_ok(), second.is_ok());
    assert_eq!(provider.token_calls.load(Ordering::SeqCst), before + 1);
    // Discovery/consent failures cannot produce a ready connection.
    for flag in [
        &provider.unsupported,
        &provider.bad_issuer,
        &provider.bad_resource,
        &provider.malicious_endpoint,
    ] {
        flag.store(true, Ordering::SeqCst);
        let connection =
            new_connection(&pool, &owner, &format!("rejected-{}", uuid::Uuid::new_v4())).await;
        assert!(
            service
                .discover(&pool, &owner, &connection.connection_id)
                .await
                .is_err()
        );
        assert!(
            service
                .start(&pool, &owner, &connection.connection_id, input(1), &KEY)
                .await
                .is_err()
        );
        flag.store(false, Ordering::SeqCst);
    }
    let connection = new_connection(&pool, &owner, "consent").await;
    let mut request = input(1);
    request.consent = false;
    assert!(
        service
            .start(&pool, &owner, &connection.connection_id, request, &KEY)
            .await
            .is_err()
    );
    let mut request = input(1);
    request.scopes = vec!["admin".into()];
    assert!(
        service
            .start(&pool, &owner, &connection.connection_id, request, &KEY)
            .await
            .is_err()
    );
    let mut request = input(1);
    request.return_to = "//evil.example.com".into();
    assert!(
        service
            .start(&pool, &owner, &connection.connection_id, request, &KEY)
            .await
            .is_err()
    );
    let before = provider.requests.lock().unwrap().len();
    for kind in [McpAuthKind::None, McpAuthKind::Bearer] {
        let c = store::create_connection(
            &pool,
            &owner,
            SavePersonalMcpConnection {
                name: format!("bypass-{}", kind.as_str()),
                endpoint: RESOURCE.into(),
                auth_kind: kind,
                bearer_secret: if kind == McpAuthKind::Bearer {
                    Some("test-static-token".into())
                } else {
                    None
                },
            },
            &KEY,
        )
        .await
        .unwrap();
        assert!(
            service
                .discover(&pool, &owner, &c.connection_id)
                .await
                .is_err()
        );
        assert!(
            service
                .start(&pool, &owner, &c.connection_id, input(1), &KEY)
                .await
                .is_err()
        );
    }
    assert_eq!(provider.requests.lock().unwrap().len(), before);
    // No workspace memberships were created: explicit UUID-scoped teardown.
    db_execute!(
        &pool,
        "DELETE FROM user_mcp_connections WHERE owner_user_id=$1",
        &owner
    )
    .unwrap();
    for actor in [&owner, &other] {
        assert!(
            crate::user_service::delete_user(&pool, actor)
                .await
                .unwrap()
        );
    }
}
#[tokio::test]
async fn personal_mcp_oauth_sqlite_security_matrix() {
    matrix(crate::test_support::test_pool().await).await;
}
#[tokio::test]
async fn personal_mcp_oauth_postgres_security_matrix() {
    matrix(
        kyomi_core::test_db::connect_test_pool()
            .await
            .expect("Postgres is required for OAuth parity"),
    )
    .await;
}
