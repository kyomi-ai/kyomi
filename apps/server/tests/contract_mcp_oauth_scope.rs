// SPDX-License-Identifier: AGPL-3.0-or-later

//! OAuth access and refresh tokens stay bound to the MCP resource.

use kyomi_test_harness::{cleanup_test_user, setup_auth_context};
use serde_json::{Value, json};

const VERIFIER: &str = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
const CHALLENGE: &str = "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM";

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap()
}

async fn assert_mcp_only(base: &str, token: &str) {
    let mcp = client()
        .post(format!("{base}/mcp"))
        .header("origin", "http://localhost:5173")
        .bearer_auth(token)
        .json(&json!({"jsonrpc": "2.0", "id": 1, "method": "initialize"}))
        .send()
        .await
        .unwrap();
    assert_eq!(
        mcp.status(),
        200,
        "MCP OAuth token must authorize MCP initialize"
    );

    let rest = client()
        .get(format!("{base}/api/v1/push/subscriptions"))
        .bearer_auth(token)
        .send()
        .await
        .unwrap();
    assert_eq!(
        rest.status(),
        401,
        "MCP OAuth token must not authorize ordinary REST"
    );

    let browser = client()
        .get(format!(
            "{base}/api/v1/oauth/authorize/continue?state=unused"
        ))
        .header("cookie", format!("access_token={token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(
        browser.status(),
        401,
        "MCP OAuth token must not become a browser session"
    );
}

#[tokio::test]
async fn exchanged_and_refreshed_mcp_tokens_cannot_authorize_browser_or_rest() {
    let Some(ctx) = setup_auth_context("MCP Scope User", "mcpscope", "oauth-boundary").await else {
        assert_ne!(
            std::env::var("KYOMI_REQUIRE_POSTGRES_TESTS").as_deref(),
            Ok("1"),
            "KYOMI_REQUIRE_POSTGRES_TESTS=1 requires the MCP OAuth scope contract to run with an in-process database",
        );
        eprintln!("SKIP: requires Rust backend mode");
        return;
    };
    let base = &ctx.base_url;
    let redirect_uri = "https://example.com/callback";

    let registration: Value = client()
        .post(format!("{base}/api/v1/oauth/register"))
        .json(&json!({"redirect_uris": [redirect_uri]}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let client_id = registration["client_id"].as_str().unwrap();

    // Seed a valid authorization code so this contract exercises the token
    // exchange independently of the browser consent flow.
    let code = kyomi_auth::redis_ops::generate_token();
    kyomi_auth::redis_ops::store_oauth_state(
        &ctx.kv,
        "oauth_code",
        &code,
        &json!({
            "user_id": ctx.user_id,
            "workspace_id": ctx.workspace_id,
            "client_id": client_id,
            "redirect_uri": redirect_uri,
            "code_challenge": CHALLENGE,
        }),
    )
    .await
    .unwrap();

    let exchange: Value = client()
        .post(format!("{base}/api/v1/oauth/token"))
        .form(&[
            ("grant_type", "authorization_code"),
            ("client_id", client_id),
            ("code", code.as_str()),
            ("redirect_uri", redirect_uri),
            ("code_verifier", VERIFIER),
        ])
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let issued_access = exchange["access_token"].as_str().unwrap();
    let refresh_token = exchange["refresh_token"].as_str().unwrap();
    assert_eq!(exchange["scope"], "mcp");
    assert_mcp_only(base, issued_access).await;

    let session_rest = client()
        .get(format!("{base}/api/v1/push/subscriptions"))
        .bearer_auth(&ctx.access_token)
        .send()
        .await
        .unwrap();
    assert_eq!(
        session_rest.status(),
        200,
        "normal session JWT must still authorize REST"
    );

    // An MCP refresh token must not be exchangeable at the browser refresh route.
    let browser_refresh = client()
        .post(format!("{base}/api/v1/auth/refresh"))
        .header("cookie", format!("refresh_token={refresh_token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(browser_refresh.status(), 401);

    let refreshed: Value = client()
        .post(format!("{base}/api/v1/oauth/token"))
        .form(&[
            ("grant_type", "refresh_token"),
            ("client_id", client_id),
            ("refresh_token", refresh_token),
        ])
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let refreshed_access = refreshed["access_token"].as_str().unwrap();
    assert_eq!(refreshed["scope"], "mcp");
    assert_mcp_only(base, refreshed_access).await;

    cleanup_test_user(&ctx.db, "mcpscope-test-oauth-boundary@contract-test.local").await;
}
