// SPDX-License-Identifier: AGPL-3.0-or-later

//! Real MCP refresh grants earn a sliding inactivity window after validation.

use chrono::{DateTime, Duration, Utc};
use kyomi_auth::{jwt, token_service};
use kyomi_test_harness::{AuthContext, cleanup_test_user, setup_auth_context};
use serde_json::{Value, json};

#[derive(sqlx::FromRow)]
struct Expiry {
    expires_at: DateTime<Utc>,
    replaced_at: Option<DateTime<Utc>>,
}

async fn context(tag: &str) -> AuthContext {
    setup_auth_context("Sliding MCP User", "slidingmcp", tag)
        .await
        .expect("sliding MCP contract requires an in-process test server")
}

async fn registered(ctx: &AuthContext) -> String {
    let response = reqwest::Client::new()
        .post(format!("{}/api/v1/oauth/register", ctx.base_url))
        .json(&json!({"redirect_uris": ["https://example.com/callback"]}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    response.json::<Value>().await.unwrap()["client_id"]
        .as_str()
        .unwrap()
        .to_string()
}

async fn token(ctx: &AuthContext, client: Option<&str>, expiry: DateTime<Utc>) -> (String, String) {
    let raw = jwt::create_refresh_token();
    let id = token_service::store_refresh_token(
        &ctx.db,
        &ctx.user_id,
        &token_service::hash_refresh_token(&raw),
        expiry,
        &token_service::DeviceInfo {
            user_agent: None,
            ip_address: None,
            country_code: None,
            oauth_client_id: client.map(str::to_string),
        },
        &token_service::generate_family_id(),
    )
    .await
    .unwrap();
    (id, raw)
}

async fn expiry(ctx: &AuthContext, id: &str) -> Expiry {
    kyomi_core::db_fetch_one!(
        &ctx.db,
        Expiry,
        "SELECT expires_at, replaced_at FROM refresh_tokens WHERE token_id = $1",
        id
    )
    .unwrap()
}

async fn refresh(ctx: &AuthContext, client: &str, raw: &str) -> reqwest::Response {
    reqwest::Client::new()
        .post(format!("{}/api/v1/oauth/token", ctx.base_url))
        .form(&[
            ("grant_type", "refresh_token"),
            ("client_id", client),
            ("refresh_token", raw),
        ])
        .send()
        .await
        .unwrap()
}

async fn cleanup(ctx: &AuthContext, tag: &str, clients: &[&str]) {
    kyomi_core::db_execute!(
        &ctx.db,
        "DELETE FROM refresh_tokens WHERE user_id = $1",
        &ctx.user_id
    )
    .unwrap();
    for client in clients {
        kyomi_core::db_execute!(
            &ctx.db,
            "DELETE FROM oauth_clients WHERE client_id = $1",
            *client
        )
        .unwrap();
    }
    cleanup_test_user(
        &ctx.db,
        &format!("slidingmcp-test-{tag}@contract-test.local"),
    )
    .await;
}

fn now() -> DateTime<Utc> {
    DateTime::from_timestamp_micros(Utc::now().timestamp_micros()).unwrap()
}

#[tokio::test]
async fn active_mcp_refresh_keeps_raw_token_and_renews_inactivity_timeout() {
    let tag = format!("active-{}", uuid::Uuid::new_v4());
    let ctx = context(&tag).await;
    let client = registered(&ctx).await;
    let original = now() + Duration::hours(1);
    let (id, raw) = token(&ctx, Some(&client), original).await;
    for _ in 0..2 {
        let before = Utc::now();
        let response = refresh(&ctx, &client, &raw).await;
        assert_eq!(response.status(), 200);
        let body: Value = response.json().await.unwrap();
        assert_eq!(body["refresh_token"], raw);
        assert_eq!(body["scope"], "mcp");
        let signed =
            jwt::validate_token(body["access_token"].as_str().unwrap(), &ctx.jwt_secret).unwrap();
        assert_eq!(signed.claims.sub, ctx.user_id);
        let saved = expiry(&ctx, &id).await;
        let ttl = Duration::days(kyomi_core::constants::get().jwt.refresh_token_expire_days);
        assert!(saved.expires_at >= before + ttl - Duration::seconds(1));
        assert!(saved.expires_at <= Utc::now() + ttl + Duration::seconds(1));
        assert!(
            saved.expires_at > original,
            "the grant survives its original deadline"
        );
        assert!(saved.replaced_at.is_none(), "MCP must never rotate");
    }
    // Explicit inactivity fixture beyond the renewed deadline: no multi-day sleep.
    let inactive = now() - Duration::days(1);
    kyomi_core::db_execute!(
        &ctx.db,
        "UPDATE refresh_tokens SET expires_at = $1 WHERE token_id = $2",
        &inactive,
        &id
    )
    .unwrap();
    let response = refresh(&ctx, &client, &raw).await;
    assert_eq!(response.status(), 400);
    assert_eq!(
        response.json::<Value>().await.unwrap(),
        json!({"error": "invalid_grant: refresh token invalid or expired"})
    );
    assert_eq!(expiry(&ctx, &id).await.expires_at, inactive);
    cleanup(&ctx, &tag, &[&client]).await;
}

#[tokio::test]
async fn rejected_mcp_grants_never_earn_expiry_extension() {
    let tag = format!("rejected-{}", uuid::Uuid::new_v4());
    let ctx = context(&tag).await;
    let client = registered(&ctx).await;
    let other = registered(&ctx).await;
    let original = now() + Duration::hours(1);
    let (id, raw) = token(&ctx, Some(&client), original).await;
    for rejected_client in [&other, "unknown-client"] {
        assert_eq!(refresh(&ctx, rejected_client, &raw).await.status(), 400);
        assert_eq!(expiry(&ctx, &id).await.expires_at, original);
    }
    // The active-client gate runs before the refresh grant can be renewed.
    kyomi_core::db_execute!(
        &ctx.db,
        "UPDATE oauth_clients SET active = $1 WHERE client_id = $2",
        &false,
        &client
    )
    .unwrap();
    assert_eq!(refresh(&ctx, &client, &raw).await.status(), 400);
    assert_eq!(expiry(&ctx, &id).await.expires_at, original);
    kyomi_core::db_execute!(
        &ctx.db,
        "UPDATE oauth_clients SET active = $1 WHERE client_id = $2",
        &true,
        &client
    )
    .unwrap();
    assert_eq!(
        refresh(&ctx, &client, "unknown-refresh").await.status(),
        400
    );
    let (browser_id, browser_raw) = token(&ctx, None, original).await;
    assert_eq!(refresh(&ctx, &client, &browser_raw).await.status(), 400);
    assert_eq!(expiry(&ctx, &browser_id).await.expires_at, original);
    kyomi_core::db_execute!(
        &ctx.db,
        "UPDATE users SET active = $1 WHERE user_id = $2",
        &false,
        &ctx.user_id
    )
    .unwrap();
    assert_eq!(refresh(&ctx, &client, &raw).await.status(), 400);
    assert_eq!(expiry(&ctx, &id).await.expires_at, original);
    kyomi_core::db_execute!(
        &ctx.db,
        "UPDATE users SET active = $1, last_workspace_id = NULL WHERE user_id = $2",
        &true,
        &ctx.user_id
    )
    .unwrap();
    kyomi_core::db_execute!(
        &ctx.db,
        "UPDATE workspace_users SET active = $1 WHERE user_id = $2",
        &false,
        &ctx.user_id
    )
    .unwrap();
    let missing_workspace = refresh(&ctx, &client, &raw).await;
    assert_eq!(missing_workspace.status(), 400);
    assert_eq!(
        missing_workspace.json::<Value>().await.unwrap(),
        json!({"error": "invalid_grant: no workspace access"})
    );
    assert_eq!(expiry(&ctx, &id).await.expires_at, original);
    token_service::revoke_refresh_token(&ctx.db, &id)
        .await
        .unwrap();
    assert_eq!(refresh(&ctx, &client, &raw).await.status(), 400);
    assert_eq!(expiry(&ctx, &id).await.expires_at, original);
    cleanup(&ctx, &tag, &[&client, &other]).await;
}

#[tokio::test]
async fn grace_mcp_refresh_preserves_rotation_deadlines_and_theft_detection() {
    let tag = format!("grace-{}", uuid::Uuid::new_v4());
    let ctx = context(&tag).await;
    let client = registered(&ctx).await;
    let original = now() + Duration::hours(1);
    let (id, raw) = token(&ctx, Some(&client), original).await;
    let replaced = now();
    kyomi_core::db_execute!(
        &ctx.db,
        "UPDATE refresh_tokens SET replaced_at = $1 WHERE token_id = $2",
        &replaced,
        &id
    )
    .unwrap();
    let response = refresh(&ctx, &client, &raw).await;
    assert_eq!(response.status(), 200);
    assert_eq!(
        response.json::<Value>().await.unwrap()["refresh_token"],
        raw
    );
    assert_eq!(expiry(&ctx, &id).await.expires_at, original);
    assert_eq!(expiry(&ctx, &id).await.replaced_at, Some(replaced));
    let past_grace = now()
        - Duration::seconds(
            kyomi_core::constants::get()
                .jwt
                .refresh_token_grace_period_seconds
                + 5,
        );
    kyomi_core::db_execute!(
        &ctx.db,
        "UPDATE refresh_tokens SET replaced_at = $1 WHERE token_id = $2",
        &past_grace,
        &id
    )
    .unwrap();
    let response = refresh(&ctx, &client, &raw).await;
    assert_eq!(response.status(), 400);
    assert_eq!(
        response.json::<Value>().await.unwrap(),
        json!({"error": "invalid_grant: refresh token revoked"})
    );
    assert_eq!(expiry(&ctx, &id).await.expires_at, original);
    assert!(matches!(
        token_service::verify_refresh_token(&ctx.db, &raw)
            .await
            .unwrap(),
        token_service::RefreshTokenVerifyResult::Invalid
    ));
    cleanup(&ctx, &tag, &[&client]).await;
}

#[tokio::test]
async fn failed_expiry_persistence_cannot_return_a_successful_grant() {
    let tag = format!("persistence-{}", uuid::Uuid::new_v4());
    let ctx = context(&tag).await;
    let client = registered(&ctx).await;
    let original = now() + Duration::hours(1);
    let (id, raw) = token(&ctx, Some(&client), original).await;
    let kyomi_core::DbPool::Postgres(pg) = &ctx.db else {
        panic!("this contract requires the private Postgres test harness");
    };
    // Test-generated identifiers and token ids only; the trigger rejects just
    // this fixture's expiry update, leaving verification's last_used write intact.
    let name = format!("k900_{}", uuid::Uuid::new_v4().simple());
    sqlx::query(&format!(
        "CREATE FUNCTION {name}() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'controlled expiry persistence failure'; END $$"
    )).execute(pg).await.unwrap();
    sqlx::query(&format!(
        "CREATE TRIGGER {name} BEFORE UPDATE OF expires_at ON refresh_tokens FOR EACH ROW WHEN (OLD.token_id = '{id}') EXECUTE FUNCTION {name}()"
    )).execute(pg).await.unwrap();
    let response = refresh(&ctx, &client, &raw).await;
    sqlx::query(&format!("DROP FUNCTION {name}() CASCADE"))
        .execute(pg)
        .await
        .unwrap();
    assert_eq!(response.status(), 500);
    assert_eq!(
        response.json::<Value>().await.unwrap(),
        json!({"error": "internal_error"})
    );
    assert_eq!(expiry(&ctx, &id).await.expires_at, original);
    cleanup(&ctx, &tag, &[&client]).await;
}
