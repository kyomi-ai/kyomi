// SPDX-License-Identifier: AGPL-3.0-or-later

//! Contract tests for OAuth 2.0 MCP-client endpoints under `/api/v1/oauth`.
//!
//! KYO-401 converted 4 handlers in `apps/server/src/routes/oauth.rs` from
//! `Result<T, Response>` to `Result<T, RouteError>` to clear
//! `clippy::result_large_err`. Code review confirmed every conversion is a
//! mechanical `.into_response()` -> `.into()`/`?`/`RouteError::from` swap
//! with identical `StatusCode`/`Json`/text arguments — but flagged that,
//! unlike the sibling `mcp.rs` conversions (covered by `contract_mcp.rs`),
//! nothing exercised the oauth.rs handlers at the real HTTP boundary,
//! before or after the change. This file closes that gap by covering the
//! error paths that flow through `RouteError` in each converted handler,
//! asserting the exact status code and exact response body on every one —
//! the regression protection the ticket's acceptance criteria require for
//! these auth-critical endpoints.
//!
//! Test organization:
//! - Section 1: `oauth_authorize` (oauth.rs:226)
//! - Section 2: `oauth_authorize_continue` (oauth.rs:328)
//! - Section 3: `oauth_token` (oauth.rs:438)
//! - Section 4: `register_client` (oauth.rs:761)

use serde_json::{json, Value};

use kyomi_test_harness::base_url;

// ===========================================================================
// Test infrastructure
// ===========================================================================

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap()
}

/// Register a real OAuth client via the public `/register` endpoint (RFC
/// 7591) and return its `client_id`.
///
/// Several error paths under test (`Invalid redirect_uri`, unsupported
/// `grant_type`) only trigger *after* client lookup succeeds, so they need a
/// client that genuinely exists in the database. Going through the real
/// registration endpoint — rather than hand-writing an INSERT against
/// `oauth_clients` — keeps the fixture in sync with whatever shape
/// `register_client` actually persists, and doubles as a smoke test of the
/// success path that the `redirect_uris`-empty test below only otherwise
/// exercises negatively.
async fn register_test_client(base: &str, redirect_uris: &[&str]) -> String {
    let resp = client()
        .post(format!("{base}/api/v1/oauth/register"))
        .json(&json!({ "redirect_uris": redirect_uris }))
        .send()
        .await
        .expect("register_client request should succeed at the transport level");

    assert_eq!(
        resp.status(),
        200,
        "fixture setup: client registration should succeed"
    );

    let body: Value = resp.json().await.expect("registration should return JSON");
    body["client_id"]
        .as_str()
        .expect("registration response should include client_id")
        .to_string()
}

// ===========================================================================
// 1. oauth_authorize (oauth.rs:226)
// ===========================================================================

#[tokio::test]
async fn oauth_authorize_rejects_unsupported_response_type() {
    let base = base_url().await;

    // response_type is checked before client/redirect_uri validation, so an
    // unsupported value short-circuits with garbage client_id/redirect_uri.
    let resp = client()
        .get(format!("{base}/api/v1/oauth/authorize"))
        .query(&[
            ("client_id", "does-not-matter"),
            ("redirect_uri", "https://example.com/callback"),
            ("response_type", "token"),
        ])
        .send()
        .await
        .expect("request should succeed at the transport level");

    assert_eq!(
        resp.status(),
        400,
        "unsupported response_type should return 400"
    );
    let body: Value = resp.json().await.expect("should return JSON");
    assert_eq!(
        body,
        json!({"error": "Only response_type=code is supported"}),
        "body must match the exact pre-KYO-401 error shape"
    );
}

#[tokio::test]
async fn oauth_authorize_rejects_unknown_client_id() {
    let base = base_url().await;

    let resp = client()
        .get(format!("{base}/api/v1/oauth/authorize"))
        .query(&[
            ("client_id", "unknown-client-does-not-exist"),
            ("redirect_uri", "https://example.com/callback"),
            ("response_type", "code"),
        ])
        .send()
        .await
        .expect("request should succeed at the transport level");

    assert_eq!(resp.status(), 400, "unknown client_id should return 400");
    let body: Value = resp.json().await.expect("should return JSON");
    assert_eq!(
        body,
        json!({"error": "Unknown client_id: unknown-client-does-not-exist"}),
        "body must include the exact client_id that was rejected"
    );
}

#[tokio::test]
async fn oauth_authorize_rejects_invalid_redirect_uri() {
    let base = base_url().await;
    let client_id = register_test_client(&base, &["https://example.com/callback"]).await;

    let resp = client()
        .get(format!("{base}/api/v1/oauth/authorize"))
        .query(&[
            ("client_id", client_id.as_str()),
            ("redirect_uri", "https://evil.example/steal"),
            ("response_type", "code"),
        ])
        .send()
        .await
        .expect("request should succeed at the transport level");

    assert_eq!(
        resp.status(),
        400,
        "redirect_uri not in the client's allow-list should return 400"
    );
    let body: Value = resp.json().await.expect("should return JSON");
    assert_eq!(body, json!({"error": "Invalid redirect_uri"}));
}

// ===========================================================================
// 2. oauth_authorize_continue (oauth.rs:328)
// ===========================================================================

#[tokio::test]
async fn oauth_authorize_continue_rejects_missing_session_cookie() {
    let base = base_url().await;

    let resp = client()
        .get(format!("{base}/api/v1/oauth/authorize/continue"))
        .query(&[("state", "some-pending-state")])
        .send()
        .await
        .expect("request should succeed at the transport level");

    assert_eq!(
        resp.status(),
        401,
        "missing access_token cookie should return 401"
    );
    let body = resp.text().await.expect("should return a text body");
    assert_eq!(
        body, "Not logged in",
        "body must match the exact pre-KYO-401 text error"
    );
}

#[tokio::test]
async fn oauth_authorize_continue_rejects_invalid_session_cookie() {
    let base = base_url().await;

    let resp = client()
        .get(format!("{base}/api/v1/oauth/authorize/continue"))
        .query(&[("state", "some-pending-state")])
        .header("cookie", "access_token=not-a-real-jwt")
        .send()
        .await
        .expect("request should succeed at the transport level");

    assert_eq!(
        resp.status(),
        401,
        "an access_token cookie that fails JWT validation should return 401"
    );
    let body = resp.text().await.expect("should return a text body");
    assert_eq!(
        body, "Invalid session",
        "body must match the exact pre-KYO-401 text error"
    );
}

// ===========================================================================
// 3. oauth_token (oauth.rs:438)
// ===========================================================================

#[tokio::test]
async fn oauth_token_rejects_unsupported_grant_type() {
    let base = base_url().await;
    // grant_type dispatch only runs after client lookup succeeds, so this
    // needs a real client.
    let client_id = register_test_client(&base, &["https://example.com/callback"]).await;

    let resp = client()
        .post(format!("{base}/api/v1/oauth/token"))
        .form(&[("grant_type", "client_credentials"), ("client_id", &client_id)])
        .send()
        .await
        .expect("request should succeed at the transport level");

    assert_eq!(
        resp.status(),
        400,
        "unsupported grant_type should return 400"
    );
    let body: Value = resp.json().await.expect("should return JSON");
    assert_eq!(
        body,
        json!({"error": "Unsupported grant_type: client_credentials"}),
        "body must echo the exact unsupported grant_type value"
    );
}

#[tokio::test]
async fn oauth_token_rejects_unknown_client_id() {
    let base = base_url().await;

    let resp = client()
        .post(format!("{base}/api/v1/oauth/token"))
        .form(&[
            ("grant_type", "authorization_code"),
            ("client_id", "unknown-client-does-not-exist"),
            ("code", "irrelevant-because-client-lookup-fails-first"),
        ])
        .send()
        .await
        .expect("request should succeed at the transport level");

    assert_eq!(resp.status(), 400, "unknown client_id should return 400");
    let body: Value = resp.json().await.expect("should return JSON");
    assert_eq!(
        body,
        json!({"error": "Unknown client_id: unknown-client-does-not-exist"}),
        "body must match oauth_authorize's identical Unknown client_id shape \
         (both flow through the same lookup_active_client helper)"
    );
}

// ===========================================================================
// 4. register_client (oauth.rs:761)
// ===========================================================================

#[tokio::test]
async fn register_client_rejects_empty_redirect_uris() {
    let base = base_url().await;

    let resp = client()
        .post(format!("{base}/api/v1/oauth/register"))
        .json(&json!({ "redirect_uris": [] }))
        .send()
        .await
        .expect("request should succeed at the transport level");

    assert_eq!(
        resp.status(),
        400,
        "empty redirect_uris should return 400"
    );
    let body: Value = resp.json().await.expect("should return JSON");
    assert_eq!(
        body,
        json!({"error": "redirect_uris is required and must not be empty"}),
        "body must match the exact pre-KYO-401 error shape"
    );
}
