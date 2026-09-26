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

use serde_json::{Value, json};

use kyomi_test_harness::{AuthContext, base_url, setup_auth_context};

const VERIFIER: &str = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
const CHALLENGE: &str = "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM";

// ===========================================================================
// Test infrastructure
// ===========================================================================

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("OAuth contract value")
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
            ("code_challenge", CHALLENGE),
            ("code_challenge_method", "S256"),
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
            ("code_challenge", CHALLENGE),
            ("code_challenge_method", "S256"),
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
        .form(&[
            ("grant_type", "client_credentials"),
            ("client_id", &client_id),
        ])
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

    assert_eq!(resp.status(), 400, "empty redirect_uris should return 400");
    let body: Value = resp.json().await.expect("should return JSON");
    assert_eq!(
        body,
        json!({"error": "redirect_uris is required and must not be empty"}),
        "body must match the exact pre-KYO-401 error shape"
    );
}

fn hidden_value(page: &str, name: &str) -> String {
    let marker = format!("name=\"{name}\" value=\"");
    page.split(&marker)
        .nth(1)
        .expect("consent form field")
        .split('"')
        .next()
        .expect("OAuth contract value")
        .to_owned()
}

async fn consent_page(ctx: &AuthContext, client_id: &str) -> (String, String, String) {
    let resp = client()
        .get(format!("{}/api/v1/oauth/authorize", ctx.base_url))
        .header("cookie", format!("access_token={}", ctx.access_token))
        .query(&[
            ("client_id", client_id),
            ("redirect_uri", "https://example.com/callback?existing=1"),
            ("response_type", "code"),
            ("code_challenge", CHALLENGE),
            ("code_challenge_method", "S256"),
            ("state", "client state & more"),
        ])
        .send()
        .await
        .expect("OAuth contract value");
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.headers()["cache-control"], "no-store");
    let csp = resp.headers()["content-security-policy"]
        .to_str()
        .expect("consent CSP");
    assert!(csp.contains("style-src 'self'"));
    assert!(csp.contains("frame-ancestors 'none'"));
    let consent_cookie = resp.headers()["set-cookie"]
        .to_str()
        .expect("consent cookie")
        .split(';')
        .next()
        .expect("consent cookie pair")
        .to_owned();
    let page = resp.text().await.expect("OAuth contract value");
    assert!(page.contains("Allow MCP access?"));
    assert!(page.contains("kyomi_full_logo_white.svg"));
    assert!(!page.contains("code="));
    (
        hidden_value(&page, "transaction"),
        hidden_value(&page, "csrf"),
        consent_cookie,
    )
}

async fn oauth_context(suffix: &str) -> Option<(AuthContext, String)> {
    let ctx = setup_auth_context("OAuth Test User", "oauth", suffix).await?;
    let id =
        register_test_client(&ctx.base_url, &["https://example.com/callback?existing=1"]).await;
    Some((ctx, id))
}

#[tokio::test]
async fn logged_in_get_requires_post_consent_and_pkce_to_issue_code() {
    let Some((ctx, id)) = oauth_context("consent-allow").await else {
        return;
    };
    let (transaction, csrf, consent_cookie) = consent_page(&ctx, &id).await;
    let resp = client()
        .post(format!("{}/api/v1/oauth/authorize", ctx.base_url))
        .header(
            "cookie",
            format!("access_token={}; {consent_cookie}", ctx.access_token),
        )
        .form(&[
            ("transaction", transaction.as_str()),
            ("csrf", csrf.as_str()),
            ("decision", "allow"),
        ])
        .send()
        .await
        .expect("OAuth contract value");
    assert_eq!(resp.status(), 303);
    let location = resp.headers()["location"]
        .to_str()
        .expect("OAuth contract value");
    let url = url::Url::parse(location).expect("OAuth contract value");
    assert_eq!(
        url.query_pairs()
            .find(|(key, _)| key == "existing")
            .expect("OAuth contract value")
            .1,
        "1"
    );
    assert_eq!(
        url.query_pairs()
            .find(|(key, _)| key == "state")
            .expect("OAuth contract value")
            .1,
        "client state & more"
    );
    let code = url
        .query_pairs()
        .find(|(key, _)| key == "code")
        .expect("OAuth contract value")
        .1
        .to_string();
    let replay = client()
        .post(format!("{}/api/v1/oauth/authorize", ctx.base_url))
        .header(
            "cookie",
            format!("access_token={}; {consent_cookie}", ctx.access_token),
        )
        .form(&[
            ("transaction", transaction.as_str()),
            ("csrf", csrf.as_str()),
            ("decision", "allow"),
        ])
        .send()
        .await
        .expect("OAuth contract value");
    assert_eq!(replay.status(), 400);
    let wrong_pkce = client()
        .post(format!("{}/api/v1/oauth/token", ctx.base_url))
        .form(&[
            ("grant_type", "authorization_code"),
            ("client_id", id.as_str()),
            ("redirect_uri", "https://example.com/callback?existing=1"),
            ("code", code.as_str()),
            (
                "code_verifier",
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            ),
        ])
        .send()
        .await
        .expect("OAuth contract value");
    assert_eq!(wrong_pkce.status(), 400);
    assert_eq!(
        wrong_pkce
            .json::<Value>()
            .await
            .expect("OAuth contract value")["error"],
        "invalid_grant: PKCE verification failed"
    );
}

#[tokio::test]
async fn consent_denial_csrf_and_browser_binding() {
    let Some((ctx, id)) = oauth_context("consent-deny").await else {
        return;
    };
    let (transaction, csrf, consent_cookie) = consent_page(&ctx, &id).await;
    let denied = client()
        .post(format!("{}/api/v1/oauth/authorize", ctx.base_url))
        .header(
            "cookie",
            format!("access_token={}; {consent_cookie}", ctx.access_token),
        )
        .form(&[
            ("transaction", transaction.as_str()),
            ("csrf", csrf.as_str()),
            ("decision", "deny"),
        ])
        .send()
        .await
        .expect("OAuth contract value");
    assert_eq!(denied.status(), 303);
    let location = denied.headers()["location"]
        .to_str()
        .expect("OAuth contract value");
    assert!(location.contains("error=access_denied"));
    assert!(!location.contains("code="));

    let (transaction, _, consent_cookie) = consent_page(&ctx, &id).await;
    let csrf_failure = client()
        .post(format!("{}/api/v1/oauth/authorize", ctx.base_url))
        .header(
            "cookie",
            format!("access_token={}; {consent_cookie}", ctx.access_token),
        )
        .form(&[
            ("transaction", transaction.as_str()),
            ("csrf", "bad"),
            ("decision", "allow"),
        ])
        .send()
        .await
        .expect("OAuth contract value");
    assert_eq!(csrf_failure.status(), 403);

    let (transaction, csrf, consent_cookie) = consent_page(&ctx, &id).await;
    // A pending page must survive the middleware rotating the access JWT.
    let refresh_token = kyomi_auth::jwt::create_refresh_token();
    let refresh_hash = kyomi_auth::token_service::hash_refresh_token(&refresh_token);
    let device = kyomi_auth::token_service::DeviceInfo {
        user_agent: None,
        ip_address: None,
        country_code: None,
        oauth_client_id: None,
    };
    kyomi_auth::token_service::store_refresh_token(
        &ctx.db,
        &ctx.user_id,
        &refresh_hash,
        chrono::Utc::now() + chrono::Duration::days(1),
        &device,
        &kyomi_auth::token_service::generate_family_id(),
    )
    .await
    .expect("store refresh fixture");
    let expired_access = kyomi_auth::jwt::create_access_token_str(
        &ctx.user_id,
        &ctx.jwt_secret,
        -1,
        Default::default(),
    )
    .expect("expired access fixture");
    let refreshed_session = client()
        .post(format!("{}/api/v1/oauth/authorize", ctx.base_url))
        .header(
            "cookie",
            format!(
                "access_token={expired_access}; refresh_token={refresh_token}; {consent_cookie}"
            ),
        )
        .form(&[
            ("transaction", transaction.as_str()),
            ("csrf", csrf.as_str()),
            ("decision", "allow"),
        ])
        .send()
        .await
        .expect("consent after access refresh");
    assert_eq!(
        refreshed_session.status(),
        303,
        "a refreshed access JWT preserves consent"
    );
    assert!(
        refreshed_session
            .headers()
            .get_all("set-cookie")
            .iter()
            .any(|value| {
                value
                    .to_str()
                    .is_ok_and(|cookie| cookie.starts_with("access_token="))
            })
    );

    let (transaction, csrf, _) = consent_page(&ctx, &id).await;
    let wrong_browser = client()
        .post(format!("{}/api/v1/oauth/authorize", ctx.base_url))
        .header("cookie", format!("access_token={}", ctx.access_token))
        .form(&[
            ("transaction", transaction.as_str()),
            ("csrf", csrf.as_str()),
            ("decision", "allow"),
        ])
        .send()
        .await
        .expect("wrong-browser request");
    assert_eq!(wrong_browser.status(), 403);
}

#[tokio::test]
async fn login_continuation_only_renders_consent() {
    let Some((ctx, id)) = oauth_context("consent-login").await else {
        return;
    };
    let initial = client()
        .get(format!("{}/api/v1/oauth/authorize", ctx.base_url))
        .query(&[
            ("client_id", id.as_str()),
            ("redirect_uri", "https://example.com/callback?existing=1"),
            ("code_challenge", CHALLENGE),
            ("code_challenge_method", "S256"),
        ])
        .send()
        .await
        .expect("OAuth contract value");
    assert_eq!(initial.status(), 303);
    let login = url::Url::parse(
        initial.headers()["location"]
            .to_str()
            .expect("OAuth contract value"),
    )
    .expect("OAuth contract value");
    let state = login
        .query_pairs()
        .find(|(key, _)| key == "oauth_continue")
        .expect("OAuth contract value")
        .1
        .to_string();
    let continued = client()
        .get(format!("{}/api/v1/oauth/authorize/continue", ctx.base_url))
        .header("cookie", format!("access_token={}", ctx.access_token))
        .query(&[("state", state.as_str())])
        .send()
        .await
        .expect("OAuth contract value");
    assert_eq!(continued.status(), 200);
    assert_eq!(continued.headers()["cache-control"], "no-store");
    let csp = continued.headers()["content-security-policy"]
        .to_str()
        .expect("continuation consent CSP");
    assert!(csp.contains("style-src 'self'"));
    assert!(csp.contains("img-src 'self'"));
    assert!(csp.contains("frame-ancestors 'none'"));
    let page = continued.text().await.expect("OAuth contract value");
    assert!(page.contains("Allow MCP access?"));
    assert!(page.contains("kyomi_full_logo_white.svg"));
    assert!(page.contains("fonts.googleapis.com"));
    assert!(!page.contains("code="));
}

#[tokio::test]
async fn registration_rejects_unsafe_callbacks_and_authorize_requires_s256() {
    let base = base_url().await;
    for uri in [
        "javascript:alert(1)",
        "https://user:pass@example.com/callback",
        "https://example.com/callback#fragment",
        "http://example.com/callback",
    ] {
        let resp = client()
            .post(format!("{base}/api/v1/oauth/register"))
            .json(&json!({"redirect_uris": [uri]}))
            .send()
            .await
            .expect("OAuth contract value");
        assert_eq!(resp.status(), 400, "{uri}");
    }
    let id = register_test_client(&base, &["http://127.0.0.1:8000/callback"]).await;
    let resp = client()
        .get(format!("{base}/api/v1/oauth/authorize"))
        .query(&[
            ("client_id", id.as_str()),
            ("redirect_uri", "http://127.0.0.1:8000/callback"),
        ])
        .send()
        .await
        .expect("OAuth contract value");
    assert_eq!(resp.status(), 400);
    assert_eq!(
        resp.json::<Value>().await.expect("OAuth contract value")["error"],
        "PKCE S256 code_challenge required"
    );
}

#[tokio::test]
async fn code_exchange_requires_exact_redirect_and_correct_pkce() {
    let Some((ctx, id)) = oauth_context("token-valid").await else {
        return;
    };
    let (transaction, csrf, consent_cookie) = consent_page(&ctx, &id).await;
    let approved = client()
        .post(format!("{}/api/v1/oauth/authorize", ctx.base_url))
        .header(
            "cookie",
            format!("access_token={}; {consent_cookie}", ctx.access_token),
        )
        .form(&[
            ("transaction", transaction.as_str()),
            ("csrf", csrf.as_str()),
            ("decision", "allow"),
        ])
        .send()
        .await
        .expect("OAuth contract value");
    assert_eq!(approved.status(), 303);
    let location = url::Url::parse(
        approved.headers()["location"]
            .to_str()
            .expect("OAuth contract value"),
    )
    .expect("OAuth contract value");
    let code = location
        .query_pairs()
        .find(|(key, _)| key == "code")
        .expect("OAuth contract value")
        .1
        .to_string();
    let missing_redirect = client()
        .post(format!("{}/api/v1/oauth/token", ctx.base_url))
        .form(&[
            ("grant_type", "authorization_code"),
            ("client_id", id.as_str()),
            ("code", code.as_str()),
            ("code_verifier", VERIFIER),
        ])
        .send()
        .await
        .expect("OAuth contract value");
    assert_eq!(missing_redirect.status(), 400);
    assert_eq!(
        missing_redirect
            .json::<Value>()
            .await
            .expect("OAuth contract value")["error"],
        "redirect_uri required"
    );
    let exchanged = client()
        .post(format!("{}/api/v1/oauth/token", ctx.base_url))
        .form(&[
            ("grant_type", "authorization_code"),
            ("client_id", id.as_str()),
            ("redirect_uri", "https://example.com/callback?existing=1"),
            ("code", code.as_str()),
            ("code_verifier", VERIFIER),
        ])
        .send()
        .await
        .expect("OAuth contract value");
    assert_eq!(exchanged.status(), 200);
    let tokens: Value = exchanged.json().await.expect("OAuth contract value");
    assert!(tokens["access_token"].as_str().is_some());
    assert!(tokens["refresh_token"].as_str().is_some());
}

#[tokio::test]
async fn different_user_cannot_approve_another_users_transaction() {
    let Some((ctx, id)) = oauth_context("wrong-user-owner").await else {
        return;
    };
    let Some(other) = setup_auth_context("Other User", "oauth", "wrong-user-other").await else {
        return;
    };
    let (transaction, csrf, consent_cookie) = consent_page(&ctx, &id).await;
    let response = client()
        .post(format!("{}/api/v1/oauth/authorize", ctx.base_url))
        .header(
            "cookie",
            format!("access_token={}; {consent_cookie}", other.access_token),
        )
        .form(&[
            ("transaction", transaction.as_str()),
            ("csrf", csrf.as_str()),
            ("decision", "allow"),
        ])
        .send()
        .await
        .expect("OAuth contract value");
    assert_eq!(response.status(), 403);
}
