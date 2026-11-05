// SPDX-License-Identifier: AGPL-3.0-or-later
//! A start must retain its own CAS generation across the store's post-commit read.
use super::*;

async fn matrix(pool: DbPool) {
    let actor = format!("oauth-race-{}", uuid::Uuid::new_v4());
    db_execute!(
        &pool,
        "INSERT INTO users (user_id,email) VALUES ($1,$2)",
        &actor,
        format!("{actor}@example.com")
    )
    .unwrap();
    let provider = Arc::new(Provider::default());
    let service = service(provider.clone());
    let (admitted, started) = begin(&service, &pool, &actor, "admitted start").await;
    assert!(
        service
            .callback(&pool, &actor, callback(&started), &KEY)
            .await
            .is_ok()
    );
    assert_eq!(
        store::get_connection(&pool, &actor, &admitted.connection_id)
            .await
            .unwrap()
            .status,
        "ready"
    );
    let token_calls = provider.token_calls.load(Ordering::SeqCst);
    let connection = new_connection(&pool, &actor, "start reread race").await;
    let committed = Arc::new(tokio::sync::Notify::new());
    let resume = Arc::new(tokio::sync::Notify::new());
    let original = store::UPDATE_COMMITTED.scope(
        (committed.clone(), resume.clone()),
        service.start(&pool, &actor, &connection.connection_id, input(1), &KEY),
    );
    tokio::pin!(original);
    tokio::time::timeout(Duration::from_secs(10), async {
        tokio::select! {
            result = &mut original => panic!("start completed before barrier: {}", result.is_ok()),
            _ = committed.notified() => {}
        }
    })
    .await
    .expect("start must commit its generation before the concurrent edit");

    // Advance the real connection while the original update has committed but
    // has not yet reread it. This task is outside the scoped test hook.
    let endpoint = "https://resource.example.com/replacement";
    let newer = store::update_connection(
        &pool,
        &actor,
        &connection.connection_id,
        2,
        SavePersonalMcpConnection {
            name: "replacement".into(),
            endpoint: endpoint.into(),
            auth_kind: McpAuthKind::Oauth,
            bearer_secret: None,
        },
        &KEY,
    )
    .await
    .unwrap();
    assert_eq!(newer.generation, 3);
    let state = random_secret();
    let redirect_uri = service.settings.redirect_uri();
    store::store_auth_transaction(
        &pool,
        &actor,
        &connection.connection_id,
        newer.generation,
        McpAuthTransaction {
            state: &state,
            issuer: ISSUER,
            resource: endpoint,
            redirect_uri: &redirect_uri,
            payload: "replacement authorization",
            expires_at: chrono::Utc::now().timestamp() + TTL,
        },
        &KEY,
    )
    .await
    .unwrap();
    resume.notify_one();
    let result = tokio::time::timeout(Duration::from_secs(10), &mut original)
        .await
        .expect("original start must finish after releasing its reread");
    assert!(matches!(result, Err(Error::Conflict(_))));
    let current = store::get_connection(&pool, &actor, &connection.connection_id)
        .await
        .unwrap();
    assert_eq!(current.endpoint, endpoint);
    assert_eq!(current.generation, newer.generation);
    assert_eq!(current.status, "needs_auth");
    assert_eq!(
        db_fetch_scalar!(
            &pool,
            i64,
            "SELECT COUNT(*) FROM user_mcp_auth_transactions WHERE connection_id=$1",
            &connection.connection_id
        )
        .unwrap(),
        1
    );
    let replacement = store::consume_auth_transaction(
        &pool,
        &actor,
        &connection.connection_id,
        newer.generation,
        McpAuthContext {
            state: &state,
            issuer: ISSUER,
            resource: endpoint,
            redirect_uri: &redirect_uri,
        },
        &KEY,
    )
    .await
    .unwrap();
    assert_eq!(replacement.expose(), "replacement authorization");
    assert_eq!(provider.token_calls.load(Ordering::SeqCst), token_calls);

    // Exhaustion must reject before discovery, registration or a DB mutation.
    let before = provider.requests.lock().unwrap().len();
    assert!(matches!(
        service
            .start(
                &pool,
                &actor,
                &connection.connection_id,
                input(i64::MAX),
                &KEY
            )
            .await,
        Err(Error::Conflict(_))
    ));
    assert_eq!(provider.requests.lock().unwrap().len(), before);
    assert_eq!(
        store::get_connection(&pool, &actor, &connection.connection_id)
            .await
            .unwrap()
            .generation,
        newer.generation
    );
    assert!(
        crate::user_service::delete_user(&pool, &actor)
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn personal_mcp_oauth_sqlite_start_rejects_post_commit_generation() {
    matrix(crate::test_support::test_pool().await).await;
}

#[tokio::test]
async fn personal_mcp_oauth_postgres_start_rejects_post_commit_generation() {
    matrix(
        kyomi_core::test_db::connect_test_pool()
            .await
            .expect("Postgres is required for OAuth start race parity"),
    )
    .await;
}
