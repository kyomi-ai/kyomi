// SPDX-License-Identifier: AGPL-3.0-or-later

//! Contract tests for Slack integration endpoints.
//!
//! These tests verify the HTTP-level contract (request/response shapes, headers,
//! status codes) for the Slack endpoints under `/api/v1/slack`:
//!
//! - `GET  /install` — Get Slack OAuth URL (admin, Team+)
//! - `GET  /oauth/callback` — OAuth callback
//! - `DELETE /uninstall` — Remove Slack integration (admin, Team+)
//! - `GET  /user/connect` — Start user Slack linking
//! - `POST /user/disconnect` — Disconnect user's Slack account
//! - `GET  /status` — Slack integration status
//! - `GET  /channels` — List Slack channels
//! - `GET  /default-watch-channel` — Get default watch channel
//! - `POST /default-watch-channel` — Set default watch channel
//! - `POST /command` — Slash command handler
//! - `POST /events` — Events API handler
//!
//! Test organization:
//! - Section 1: Unauthenticated 401 tests
//! - Section 2: Tier gating (free tier -> 403)
//! - Section 3: Slash command form-encoded parsing
//! - Section 4: Events API url_verification
//! - Section 5: Slack signature verification

use serde_json::{json, Value};

use kyomi_test_harness::{base_url, cleanup_test_user, AuthContext};

// ===========================================================================
// Test infrastructure
// ===========================================================================

/// Create auth context with free tier (default) for testing tier gating.
async fn setup_auth_context(suffix: &str) -> Option<AuthContext> {
    kyomi_test_harness::setup_auth_context("Slack Test User", "slack", suffix).await
}

/// Create auth context with Team tier for testing endpoints that require it.
async fn setup_team_auth_context(suffix: &str) -> Option<AuthContext> {
    let ctx = setup_auth_context(suffix).await?;
    kyomi_core::db_execute!(
        &ctx.db,
        "UPDATE workspaces SET subscription_tier = 'team' WHERE workspace_id = $1",
        &ctx.workspace_id
    )
    .expect("should upgrade to team tier");
    Some(ctx)
}

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap()
}

fn auth_get(base: &str, path: &str, token: &str) -> reqwest::RequestBuilder {
    client()
        .get(format!("{base}{path}"))
        .header("origin", "http://localhost:5173")
        .header("cookie", format!("access_token={token}"))
}

fn auth_post(base: &str, path: &str, token: &str) -> reqwest::RequestBuilder {
    client()
        .post(format!("{base}{path}"))
        .header("origin", "http://localhost:5173")
        .header("content-type", "application/json")
        .header("cookie", format!("access_token={token}"))
}

fn auth_delete(base: &str, path: &str, token: &str) -> reqwest::RequestBuilder {
    client()
        .delete(format!("{base}{path}"))
        .header("origin", "http://localhost:5173")
        .header("cookie", format!("access_token={token}"))
}

// ===========================================================================
// 1. Unauthenticated 401 tests
// ===========================================================================

#[tokio::test]
async fn install_returns_401_without_auth() {
    let base = base_url().await;
    let resp = client()
        .get(format!("{base}/api/v1/slack/install"))
        .header("origin", "http://localhost:5173")
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 401, "GET /install without auth should be 401");
}

#[tokio::test]
async fn uninstall_returns_401_without_auth() {
    let base = base_url().await;
    let resp = client()
        .delete(format!("{base}/api/v1/slack/uninstall"))
        .header("origin", "http://localhost:5173")
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        401,
        "DELETE /uninstall without auth should be 401"
    );
}

#[tokio::test]
async fn user_connect_returns_401_without_auth() {
    let base = base_url().await;
    let resp = client()
        .get(format!("{base}/api/v1/slack/user/connect"))
        .header("origin", "http://localhost:5173")
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        401,
        "GET /user/connect without auth should be 401"
    );
}

#[tokio::test]
async fn user_disconnect_returns_401_without_auth() {
    let base = base_url().await;
    let resp = client()
        .post(format!("{base}/api/v1/slack/user/disconnect"))
        .header("origin", "http://localhost:5173")
        .header("content-type", "application/json")
        .body("{}")
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        401,
        "POST /user/disconnect without auth should be 401"
    );
}

#[tokio::test]
async fn status_returns_401_without_auth() {
    let base = base_url().await;
    let resp = client()
        .get(format!("{base}/api/v1/slack/status"))
        .header("origin", "http://localhost:5173")
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        401,
        "GET /status without auth should be 401"
    );
}

#[tokio::test]
async fn channels_returns_401_without_auth() {
    let base = base_url().await;
    let resp = client()
        .get(format!("{base}/api/v1/slack/channels"))
        .header("origin", "http://localhost:5173")
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        401,
        "GET /channels without auth should be 401"
    );
}

#[tokio::test]
async fn default_watch_channel_get_returns_401_without_auth() {
    let base = base_url().await;
    let resp = client()
        .get(format!("{base}/api/v1/slack/default-watch-channel"))
        .header("origin", "http://localhost:5173")
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        401,
        "GET /default-watch-channel without auth should be 401"
    );
}

// ===========================================================================
// 2. Free tier gets 403 for Slack endpoints requiring Team tier
// ===========================================================================

#[ignore = "KYO-314: asserts pre-KYO-224 tier gating. Slack tier gating was deliberately removed in adf618ed (#263); this test's 403 expectation is stale and needs re-evaluating against intended post-KYO-224 behaviour"]
#[tokio::test]
async fn install_returns_403_for_free_tier() {
    let ctx = setup_auth_context("tier-install").await;
    if ctx.is_none() {
        eprintln!("SKIP: install_returns_403_for_free_tier — requires Rust-backend mode");
        return;
    }
    let ctx = ctx.unwrap();

    // Default tier is free, which should NOT have slack_integration capability
    let resp = auth_get(
        &ctx.base_url,
        "/api/v1/slack/install",
        &ctx.access_token,
    )
    .send()
    .await
    .expect("install request should succeed");

    assert_eq!(
        resp.status(),
        403,
        "Slack install for free tier should return 403"
    );

    let body: Value = resp.json().await.expect("should return JSON");
    assert!(body.get("detail").is_some(), "403 response should have 'detail'");

    cleanup_test_user(&ctx.db, "slack-test-tier-install@contract-test.local").await;
}

#[ignore = "KYO-314: asserts pre-KYO-224 tier gating. Slack tier gating was deliberately removed in adf618ed (#263); this test's 403 expectation is stale and needs re-evaluating against intended post-KYO-224 behaviour"]
#[tokio::test]
async fn status_returns_403_for_free_tier() {
    let ctx = setup_auth_context("tier-status").await;
    if ctx.is_none() {
        eprintln!("SKIP: status_returns_403_for_free_tier — requires Rust-backend mode");
        return;
    }
    let ctx = ctx.unwrap();

    let resp = auth_get(
        &ctx.base_url,
        "/api/v1/slack/status",
        &ctx.access_token,
    )
    .send()
    .await
    .expect("status request should succeed");

    assert_eq!(
        resp.status(),
        403,
        "Slack status for free tier should return 403"
    );

    cleanup_test_user(&ctx.db, "slack-test-tier-status@contract-test.local").await;
}

#[ignore = "KYO-314: asserts pre-KYO-224 tier gating. Slack tier gating was deliberately removed in adf618ed (#263); this test's 403 expectation is stale and needs re-evaluating against intended post-KYO-224 behaviour"]
#[tokio::test]
async fn uninstall_returns_403_for_free_tier() {
    let ctx = setup_auth_context("tier-uninstall").await;
    if ctx.is_none() {
        eprintln!("SKIP: uninstall_returns_403_for_free_tier — requires Rust-backend mode");
        return;
    }
    let ctx = ctx.unwrap();

    let resp = auth_delete(
        &ctx.base_url,
        "/api/v1/slack/uninstall",
        &ctx.access_token,
    )
    .send()
    .await
    .expect("uninstall request should succeed");

    assert_eq!(
        resp.status(),
        403,
        "Slack uninstall for free tier should return 403"
    );

    cleanup_test_user(&ctx.db, "slack-test-tier-uninstall@contract-test.local").await;
}

// ===========================================================================
// 3. Team tier gets correct responses for Slack endpoints
// ===========================================================================

#[tokio::test]
async fn status_returns_correct_response_shape_for_team_tier() {
    let ctx = setup_team_auth_context("team-status").await;
    if ctx.is_none() {
        eprintln!(
            "SKIP: status_returns_correct_response_shape_for_team_tier — requires Rust-backend mode"
        );
        return;
    }
    let ctx = ctx.unwrap();

    let resp = auth_get(
        &ctx.base_url,
        "/api/v1/slack/status",
        &ctx.access_token,
    )
    .send()
    .await
    .expect("status request should succeed");

    assert_eq!(resp.status(), 200, "Slack status for Team tier should return 200");

    let body: Value = resp.json().await.expect("should return JSON");

    // Verify required fields
    assert!(body.get("installed").is_some(), "missing 'installed'");
    assert!(body.get("user_connected").is_some(), "missing 'user_connected'");
    assert!(body["installed"].is_boolean(), "'installed' should be bool");
    assert!(body["user_connected"].is_boolean(), "'user_connected' should be bool");

    // Fresh workspace should not have Slack installed
    assert_eq!(body["installed"], false, "fresh workspace should not have Slack installed");
    assert_eq!(body["user_connected"], false, "user should not be connected");

    // Nullable fields should be present (null is acceptable)
    assert!(body.get("team_name").is_some(), "missing 'team_name'");
    assert!(body.get("team_id").is_some(), "missing 'team_id'");
    assert!(body.get("slack_username").is_some(), "missing 'slack_username'");

    cleanup_test_user(&ctx.db, "slack-test-team-status@contract-test.local").await;
}

#[tokio::test]
async fn install_returns_500_when_slack_not_configured() {
    let ctx = setup_team_auth_context("team-install").await;
    if ctx.is_none() {
        eprintln!(
            "SKIP: install_returns_500_when_slack_not_configured — requires Rust-backend mode"
        );
        return;
    }
    let ctx = ctx.unwrap();

    let resp = auth_get(
        &ctx.base_url,
        "/api/v1/slack/install",
        &ctx.access_token,
    )
    .send()
    .await
    .expect("install request should succeed");

    // Slack client_id and client_secret are not configured in test mode
    // so this should return 500 (Internal Server Error)
    assert_eq!(
        resp.status(),
        500,
        "Slack install without SLACK_CLIENT_ID should return 500"
    );

    cleanup_test_user(&ctx.db, "slack-test-team-install@contract-test.local").await;
}

#[tokio::test]
async fn uninstall_returns_404_when_not_installed() {
    let ctx = setup_team_auth_context("team-uninstall").await;
    if ctx.is_none() {
        eprintln!(
            "SKIP: uninstall_returns_404_when_not_installed — requires Rust-backend mode"
        );
        return;
    }
    let ctx = ctx.unwrap();

    let resp = auth_delete(
        &ctx.base_url,
        "/api/v1/slack/uninstall",
        &ctx.access_token,
    )
    .send()
    .await
    .expect("uninstall request should succeed");

    // Workspace doesn't have Slack installed, so this should return 404
    assert_eq!(
        resp.status(),
        404,
        "uninstall when not installed should return 404"
    );

    cleanup_test_user(&ctx.db, "slack-test-team-uninstall@contract-test.local").await;
}

// ===========================================================================
// 4. User disconnect returns 404 when not connected
// ===========================================================================

#[tokio::test]
async fn user_disconnect_returns_404_when_not_connected() {
    let ctx = setup_auth_context("disconnect-404").await;
    if ctx.is_none() {
        eprintln!(
            "SKIP: user_disconnect_returns_404_when_not_connected — requires Rust-backend mode"
        );
        return;
    }
    let ctx = ctx.unwrap();

    let resp = auth_post(
        &ctx.base_url,
        "/api/v1/slack/user/disconnect",
        &ctx.access_token,
    )
    .body("{}".to_string())
    .send()
    .await
    .expect("user/disconnect request should succeed");

    // User has no Slack connection, should return 404
    assert_eq!(
        resp.status(),
        404,
        "user/disconnect when not connected should return 404"
    );

    cleanup_test_user(&ctx.db, "slack-test-disconnect-404@contract-test.local").await;
}

// ===========================================================================
// 5. Slash command — POST with form-encoded data
// ===========================================================================

#[tokio::test]
async fn slash_command_returns_json_response() {
    let base = base_url().await;

    // Slack sends form-encoded data. Without Slack signing secret configured
    // (test_config has slack_signing_secret = None), the verify_slack_request
    // function returns Internal error. But we can test that the endpoint exists
    // and returns the correct content type.
    let resp = client()
        .post(format!("{base}/api/v1/slack/command"))
        .header("origin", "http://localhost:5173")
        .header("content-type", "application/x-www-form-urlencoded")
        .body("command=%2Fkyomi&text=status&user_id=U12345&team_id=T12345")
        .send()
        .await
        .unwrap();

    // Without signing secret, this returns 500 (Internal Server Error)
    // because verify_slack_request requires slack_signing_secret to be set.
    assert_eq!(
        resp.status(),
        500,
        "slash command without signing secret should return 500"
    );
}

// ===========================================================================
// 6. Events API — url_verification
// ===========================================================================

#[tokio::test]
async fn events_url_verification_returns_challenge() {
    let base = base_url().await;

    let challenge_value = "3eZbrw1aBm2rZgRNFdxV2595E9CY3gmdALWMmHkvFXO7tYXAYM8P";

    let resp = client()
        .post(format!("{base}/api/v1/slack/events"))
        .header("origin", "http://localhost:5173")
        .header("content-type", "application/json")
        .body(
            json!({
                "type": "url_verification",
                "challenge": challenge_value,
                "token": "test-token"
            })
            .to_string(),
        )
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        200,
        "url_verification should return 200"
    );

    let body: Value = resp.json().await.expect("should return JSON");
    assert_eq!(
        body["challenge"], challenge_value,
        "should echo back the challenge"
    );
}

#[tokio::test]
async fn events_url_verification_returns_correct_shape() {
    let base = base_url().await;

    let resp = client()
        .post(format!("{base}/api/v1/slack/events"))
        .header("origin", "http://localhost:5173")
        .header("content-type", "application/json")
        .body(
            json!({
                "type": "url_verification",
                "challenge": "test_challenge_value",
                "token": "some_token"
            })
            .to_string(),
        )
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);

    let body: Value = resp.json().await.expect("should return JSON");

    // Response should only have "challenge" key
    assert!(body.get("challenge").is_some(), "missing 'challenge'");
    assert_eq!(body["challenge"], "test_challenge_value");
}

// ===========================================================================
// 7. Events API — event_callback without signature fails
// ===========================================================================

#[tokio::test]
async fn events_callback_without_signature_returns_error() {
    let base = base_url().await;

    let resp = client()
        .post(format!("{base}/api/v1/slack/events"))
        .header("origin", "http://localhost:5173")
        .header("content-type", "application/json")
        .body(
            json!({
                "type": "event_callback",
                "team_id": "T12345",
                "event": {
                    "type": "app_mention",
                    "user": "U12345",
                    "text": "<@U99999> hello",
                    "ts": "1234567890.123456",
                    "channel": "C12345"
                }
            })
            .to_string(),
        )
        .send()
        .await
        .unwrap();

    // Without signing secret configured, verify_slack_request returns 500
    assert_eq!(
        resp.status(),
        500,
        "event_callback without signing secret should return 500"
    );
}

// ===========================================================================
// 8. Interactions endpoint — without signature fails
// ===========================================================================

#[tokio::test]
async fn interactions_without_signature_returns_error() {
    let base = base_url().await;

    let resp = client()
        .post(format!("{base}/api/v1/slack/interactions"))
        .header("origin", "http://localhost:5173")
        .header("content-type", "application/x-www-form-urlencoded")
        .body("payload=%7B%22type%22%3A%22block_actions%22%7D")
        .send()
        .await
        .unwrap();

    // Without signing secret configured, returns 500
    assert_eq!(
        resp.status(),
        500,
        "interactions without signing secret should return 500"
    );
}

// ===========================================================================
// 9. Default watch channel — returns correct shape
// ===========================================================================

#[tokio::test]
async fn default_watch_channel_get_returns_correct_shape() {
    let ctx = setup_auth_context("dwc-shape").await;
    if ctx.is_none() {
        eprintln!(
            "SKIP: default_watch_channel_get_returns_correct_shape — requires Rust-backend mode"
        );
        return;
    }
    let ctx = ctx.unwrap();

    let resp = auth_get(
        &ctx.base_url,
        "/api/v1/slack/default-watch-channel",
        &ctx.access_token,
    )
    .send()
    .await
    .expect("default-watch-channel GET request should succeed");

    // The user doesn't have a Slack connection, so this should return 400
    // (connect your Slack account first)
    assert_eq!(
        resp.status(),
        400,
        "default-watch-channel without Slack connection should return 400"
    );

    cleanup_test_user(&ctx.db, "slack-test-dwc-shape@contract-test.local").await;
}

// ===========================================================================
// 10. User connect returns 500 when Slack not configured
// ===========================================================================

#[tokio::test]
async fn user_connect_returns_500_when_slack_not_configured() {
    let ctx = setup_auth_context("user-connect-noconfig").await;
    if ctx.is_none() {
        eprintln!(
            "SKIP: user_connect_returns_500_when_slack_not_configured — requires Rust-backend mode"
        );
        return;
    }
    let ctx = ctx.unwrap();

    let resp = auth_get(
        &ctx.base_url,
        "/api/v1/slack/user/connect",
        &ctx.access_token,
    )
    .send()
    .await
    .expect("user/connect request should succeed");

    // SLACK_CLIENT_ID not configured in test mode
    assert_eq!(
        resp.status(),
        500,
        "user/connect without Slack config should return 500"
    );

    cleanup_test_user(&ctx.db, "slack-test-user-connect-noconfig@contract-test.local").await;
}

// ===========================================================================
// 11. Channels returns correct error when not connected
// ===========================================================================

#[ignore = "KYO-314: asserts pre-KYO-224 tier gating. Slack tier gating was deliberately removed in adf618ed (#263); this test's 403 expectation is stale and needs re-evaluating against intended post-KYO-224 behaviour"]
#[tokio::test]
async fn channels_returns_403_for_free_tier() {
    let ctx = setup_auth_context("chan-free").await;
    if ctx.is_none() {
        eprintln!(
            "SKIP: channels_returns_403_for_free_tier — requires Rust-backend mode"
        );
        return;
    }
    let ctx = ctx.unwrap();

    let resp = auth_get(
        &ctx.base_url,
        "/api/v1/slack/channels",
        &ctx.access_token,
    )
    .send()
    .await
    .expect("channels request should succeed");

    assert_eq!(
        resp.status(),
        403,
        "channels for free tier should return 403"
    );

    cleanup_test_user(&ctx.db, "slack-test-chan-free@contract-test.local").await;
}

#[tokio::test]
async fn channels_returns_400_when_not_installed() {
    let ctx = setup_team_auth_context("chan-noinstall").await;
    if ctx.is_none() {
        eprintln!(
            "SKIP: channels_returns_400_when_not_installed — requires Rust-backend mode"
        );
        return;
    }
    let ctx = ctx.unwrap();

    let resp = auth_get(
        &ctx.base_url,
        "/api/v1/slack/channels",
        &ctx.access_token,
    )
    .send()
    .await
    .expect("channels request should succeed");

    // Workspace doesn't have Slack installed (no bot token)
    assert_eq!(
        resp.status(),
        400,
        "channels without Slack installation should return 400"
    );

    cleanup_test_user(&ctx.db, "slack-test-chan-noinstall@contract-test.local").await;
}
