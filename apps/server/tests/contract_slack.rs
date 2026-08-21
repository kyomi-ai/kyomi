// SPDX-License-Identifier: AGPL-3.0-or-later

//! Contract tests for Slack integration endpoints.
//!
//! These tests verify the HTTP-level contract (request/response shapes, headers,
//! status codes) for the Slack endpoints under `/api/v1/slack`:
//!
//! - `GET  /install` — Get Slack OAuth URL (admin)
//! - `GET  /oauth/callback` — OAuth callback
//! - `DELETE /uninstall` — Remove Slack integration (admin)
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
//! - Section 2: Slack endpoints are not tier-gated (free/team parity)
//! - Section 3: Slash command form-encoded parsing
//! - Section 4: Events API url_verification
//! - Section 5: Slack signature verification

use serde_json::{json, Value};

use kyomi_test_harness::{base_url, cleanup_test_user, AuthContext};

// ===========================================================================
// Test infrastructure
// ===========================================================================

/// Create an auth context on the default free tier.
///
/// Slack access is not tier-gated (KYO-224 removed the gate outright; see
/// Section 2 below), so this is simply the default context for tests that
/// don't care about tier, and the free-tier arm of the parity checks in
/// Section 2.
async fn setup_auth_context(suffix: &str) -> Option<AuthContext> {
    kyomi_test_harness::setup_auth_context("Slack Test User", "slack", suffix).await
}

/// Create an auth context on Team tier.
///
/// Nothing in this file requires Team tier. This is the non-free arm of the
/// tier-parity comparisons in Section 2; Section 3's tests also happen to run
/// against it, but that's incidental — their responses come from the test
/// fixture (Slack unconfigured / not installed), not from the tier.
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
// 2. Slack endpoints are not tier-gated (free/team parity)
// ===========================================================================
//
// KYO-224 (adf618ed) deleted Slack's subscription-tier gate outright:
// `require_slack_capability` and its four call sites are gone, and
// `has_capability` no longer exists anywhere in the codebase. `status` and
// `channels` have no 403 branch left to reach at all. `uninstall` retains
// one (`require_workspace_admin`); `install` retains two — that same
// `require_workspace_admin` check, plus a self-hosted + non-enterprise
// licence check ahead of it. Neither is tier-related, and neither fires
// here: the harness always creates workspace admins and never sets
// `self_hosted`.
//
// The obvious replacement — assert whatever status free tier currently gets —
// would test the wrong thing. Slack is unconfigured and not installed in this
// harness, so install/uninstall/channels return 500/404/400 as *fixture*
// artifacts of that setup, not because of tier. Those exact codes are already
// asserted by four sibling tests below (`install_returns_500_when_slack_not_configured`,
// `status_returns_correct_response_shape_for_team_tier`,
// `uninstall_returns_404_when_not_installed`, `channels_returns_400_when_not_installed`),
// all running at team tier. Duplicating them here at free tier would add a
// second copy of an existing assertion while still saying nothing about tier.
//
// So each test below sends the identical request as a free-tier and a
// team-tier workspace admin and asserts the two responses agree. That is an
// assertion about tier and nothing else — it holds no matter what the fixture
// returns, and it fails the moment a tier gate is reintroduced on any of
// these four endpoints.
// ===========================================================================

#[tokio::test]
async fn install_is_not_tier_gated() {
    let ctx = setup_auth_context("tier-install-free").await;
    if ctx.is_none() {
        eprintln!("SKIP: install_is_not_tier_gated — requires Rust-backend mode");
        return;
    }
    let free_ctx = ctx.unwrap();
    let team_ctx = setup_team_auth_context("tier-install-team")
        .await
        .expect("Rust-backend mode already confirmed via the free-tier context above");

    let free_status = auth_get(&free_ctx.base_url, "/api/v1/slack/install", &free_ctx.access_token)
        .send()
        .await
        .expect("install request should succeed")
        .status();
    let team_status = auth_get(&team_ctx.base_url, "/api/v1/slack/install", &team_ctx.access_token)
        .send()
        .await
        .expect("install request should succeed")
        .status();

    // No absolute status assertion here on purpose: with Slack unconfigured
    // in this harness, install already returns 500 regardless of tier, and
    // that exact code is asserted by `install_returns_500_when_slack_not_configured`
    // below. The comparison here is what actually proves tier is irrelevant.
    assert_eq!(
        free_status, team_status,
        "install response must not depend on subscription tier"
    );
    assert_ne!(
        free_status,
        reqwest::StatusCode::FORBIDDEN,
        "free tier must not be denied access to install"
    );

    cleanup_test_user(&free_ctx.db, "slack-test-tier-install-free@contract-test.local").await;
    cleanup_test_user(&team_ctx.db, "slack-test-tier-install-team@contract-test.local").await;
}

#[tokio::test]
async fn status_is_not_tier_gated() {
    let ctx = setup_auth_context("tier-status-free").await;
    if ctx.is_none() {
        eprintln!("SKIP: status_is_not_tier_gated — requires Rust-backend mode");
        return;
    }
    let free_ctx = ctx.unwrap();
    let team_ctx = setup_team_auth_context("tier-status-team")
        .await
        .expect("Rust-backend mode already confirmed via the free-tier context above");

    let free_status = auth_get(&free_ctx.base_url, "/api/v1/slack/status", &free_ctx.access_token)
        .send()
        .await
        .expect("status request should succeed")
        .status();
    let team_status = auth_get(&team_ctx.base_url, "/api/v1/slack/status", &team_ctx.access_token)
        .send()
        .await
        .expect("status request should succeed")
        .status();

    // No absolute status assertion here on purpose: status already returns
    // 200 for a fresh workspace regardless of tier, and the response shape is
    // asserted by `status_returns_correct_response_shape_for_team_tier`
    // below. The comparison here is what actually proves tier is irrelevant.
    assert_eq!(
        free_status, team_status,
        "status response must not depend on subscription tier"
    );
    assert_ne!(
        free_status,
        reqwest::StatusCode::FORBIDDEN,
        "free tier must not be denied access to status"
    );

    cleanup_test_user(&free_ctx.db, "slack-test-tier-status-free@contract-test.local").await;
    cleanup_test_user(&team_ctx.db, "slack-test-tier-status-team@contract-test.local").await;
}

#[tokio::test]
async fn uninstall_is_not_tier_gated() {
    let ctx = setup_auth_context("tier-uninstall-free").await;
    if ctx.is_none() {
        eprintln!("SKIP: uninstall_is_not_tier_gated — requires Rust-backend mode");
        return;
    }
    let free_ctx = ctx.unwrap();
    let team_ctx = setup_team_auth_context("tier-uninstall-team")
        .await
        .expect("Rust-backend mode already confirmed via the free-tier context above");

    let free_status = auth_delete(&free_ctx.base_url, "/api/v1/slack/uninstall", &free_ctx.access_token)
        .send()
        .await
        .expect("uninstall request should succeed")
        .status();
    let team_status = auth_delete(&team_ctx.base_url, "/api/v1/slack/uninstall", &team_ctx.access_token)
        .send()
        .await
        .expect("uninstall request should succeed")
        .status();

    // No absolute status assertion here on purpose: with no Slack integration
    // installed, uninstall already returns 404 regardless of tier, and that
    // exact code is asserted by `uninstall_returns_404_when_not_installed`
    // below. The comparison here is what actually proves tier is irrelevant.
    assert_eq!(
        free_status, team_status,
        "uninstall response must not depend on subscription tier"
    );
    assert_ne!(
        free_status,
        reqwest::StatusCode::FORBIDDEN,
        "free tier must not be denied access to uninstall"
    );

    cleanup_test_user(&free_ctx.db, "slack-test-tier-uninstall-free@contract-test.local").await;
    cleanup_test_user(&team_ctx.db, "slack-test-tier-uninstall-team@contract-test.local").await;
}

#[tokio::test]
async fn channels_is_not_tier_gated() {
    let ctx = setup_auth_context("tier-channels-free").await;
    if ctx.is_none() {
        eprintln!("SKIP: channels_is_not_tier_gated — requires Rust-backend mode");
        return;
    }
    let free_ctx = ctx.unwrap();
    let team_ctx = setup_team_auth_context("tier-channels-team")
        .await
        .expect("Rust-backend mode already confirmed via the free-tier context above");

    let free_status = auth_get(&free_ctx.base_url, "/api/v1/slack/channels", &free_ctx.access_token)
        .send()
        .await
        .expect("channels request should succeed")
        .status();
    let team_status = auth_get(&team_ctx.base_url, "/api/v1/slack/channels", &team_ctx.access_token)
        .send()
        .await
        .expect("channels request should succeed")
        .status();

    // No absolute status assertion here on purpose: with no Slack integration
    // installed, channels already returns 400 regardless of tier, and that
    // exact code is asserted by `channels_returns_400_when_not_installed`
    // below. The comparison here is what actually proves tier is irrelevant.
    assert_eq!(
        free_status, team_status,
        "channels response must not depend on subscription tier"
    );
    assert_ne!(
        free_status,
        reqwest::StatusCode::FORBIDDEN,
        "free tier must not be denied access to channels"
    );

    cleanup_test_user(&free_ctx.db, "slack-test-tier-channels-free@contract-test.local").await;
    cleanup_test_user(&team_ctx.db, "slack-test-tier-channels-team@contract-test.local").await;
}

// ===========================================================================
// 3. Slack endpoint responses against the unconfigured / not-installed fixture
// ===========================================================================
//
// These run against a team-tier context, but team tier is incidental — see
// Section 2 above, which proves none of these endpoints care about tier.

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
