// SPDX-License-Identifier: AGPL-3.0-or-later

//! Contract tests for the KYO-805 server-side billing gate.
//!
//! Covers the acceptance criteria from the ticket at the HTTP level:
//! - A representative REST route (`POST /api/v1/query-arrow`) 402s for a
//!   lapsed workspace.
//! - The dashboard list REST route returns JSON while active and 402s once
//!   the workspace lapses (rather than falling through to the SPA shell).
//! - MCP `initialize` 402s for a lapsed workspace (every MCP HTTP method
//!   takes a bare `AuthUser`, so this one call proves the shared mechanism).
//!   `tools/call` is also tested directly, without a session header —
//!   `handle_mcp_request` (`apps/server/src/routes/mcp.rs`) allows a missing
//!   `Mcp-Session-Id` for backwards compatibility, so it does NOT require an
//!   `initialize` call first; the gate must still fire because `AuthUser` is
//!   extracted before any method dispatch, `tools/call` included.
//! - Allowlisted server fns (billing settings, workspace switching, user/
//!   sidebar context) return 200 for a lapsed workspace.
//! - Active, trialing, and scheduled-cancellation workspaces are NOT gated.
//! - Self-hosted/personal mode is never gated, even with a `past_due` row.
//! - Fail-closed: a throwaway route with no per-route billing code, taking
//!   only a bare `AuthUser`, is gated by default.
//!
//! `AuthUser`-level unit tests (the extraction mechanism itself, including
//! the two DB-error fail-closed paths) live in
//! `crates/kyomi-auth/src/middleware.rs`'s own test module — this file
//! proves the mechanism is correctly wired into real HTTP routes, which is
//! an independent failure mode (see the module doc on
//! `kyomi_ui::server_fns::kyo_805_billing_gate_allowlist_tests` for the same
//! reasoning applied to server fns).

use serde_json::{json, Value};

use kyomi_test_harness::{cleanup_test_user, AuthContext};

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap()
}

async fn setup_auth_context(suffix: &str) -> Option<AuthContext> {
    kyomi_test_harness::setup_auth_context("Billing Gate Test User", "billgate", suffix).await
}

async fn set_subscription_status(ctx: &AuthContext, status: &str) {
    kyomi_core::db_execute!(
        &ctx.db,
        "UPDATE workspaces SET subscription_status = $1 WHERE workspace_id = $2",
        status,
        &ctx.workspace_id
    )
    .expect("should update subscription_status");
}

async fn set_scheduled_cancellation(ctx: &AuthContext) {
    let period_end = chrono::Utc::now() + chrono::Duration::days(5);
    kyomi_core::db_execute!(
        &ctx.db,
        "UPDATE workspaces SET subscription_status = 'cancelled', \
         stripe_subscription_id = 'sub_test_kyo805', subscription_period_end = $1 \
         WHERE workspace_id = $2",
        period_end,
        &ctx.workspace_id
    )
    .expect("should set scheduled cancellation");
}

// ===========================================================================
// A throwaway, test-only axum app for the fail-closed proof and the
// subscription-state matrix — deliberately independent of the shared
// harness server (whose `AppState.config.self_hosted` is fixed for the
// whole process), so the self-hosted case can be exercised at all, and so
// every case reads a clean 200/402 signal with no confounding from
// business logic unrelated to billing (e.g. "datasource not found").
// ===========================================================================

/// A route that does nothing except require a bare `AuthUser` — no
/// per-route billing code, no allow-lapsed opt-out. If the KYO-805 gate is
/// correctly wired into the `AuthUser` extractor itself, this route is
/// gated "for free"; if the gate ever regresses to something callers must
/// opt into instead of out of, this is the route that would silently start
/// passing lapsed requests through.
async fn probe_handler(_user: kyomi_auth::middleware::AuthUser) -> &'static str {
    "ok"
}

async fn start_probe_server(
    db: kyomi_core::DbPool,
    jwt_secret: String,
    self_hosted: bool,
) -> String {
    use axum::{routing::get, Router};

    let state = kyomi_auth::middleware::AuthState {
        jwt_secret,
        db,
        is_personal: false,
        self_hosted,
    };
    let app = Router::new()
        .route("/__kyo_805_fail_closed_probe", get(probe_handler))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind probe server");
    let addr = listener.local_addr().expect("local addr");
    tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("probe server exited with error");
    });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    format!("http://{addr}")
}

async fn probe(base: &str, token: &str) -> reqwest::Response {
    client()
        .get(format!("{base}/__kyo_805_fail_closed_probe"))
        .header("cookie", format!("access_token={token}"))
        .send()
        .await
        .expect("probe request should succeed")
}

// ===========================================================================
// Fail-closed: a bare `AuthUser` with no per-route code is gated
// ===========================================================================

#[tokio::test]
async fn fail_closed_bare_authuser_route_returns_402_for_lapsed_workspace() {
    let ctx = setup_auth_context("fail-closed-probe").await;
    if ctx.is_none() {
        eprintln!("SKIP: fail_closed_bare_authuser_route_returns_402_for_lapsed_workspace — requires Rust-backend mode");
        return;
    }
    let ctx = ctx.unwrap();
    set_subscription_status(&ctx, "past_due").await;

    let base = start_probe_server(ctx.db.clone(), ctx.jwt_secret.clone(), false).await;
    let resp = probe(&base, &ctx.access_token).await;

    assert_eq!(
        resp.status(),
        402,
        "a route taking a bare AuthUser, with zero per-route billing code, must be gated by default"
    );
    let body: Value = resp.json().await.expect("should return JSON");
    assert_eq!(body["error"], "payment_required");

    cleanup_test_user(&ctx.db, "billgate-test-fail-closed-probe@contract-test.local").await;
}

// ===========================================================================
// Subscription-state matrix
// ===========================================================================

#[tokio::test]
async fn active_workspace_is_not_gated() {
    let ctx = setup_auth_context("active-state").await;
    if ctx.is_none() {
        eprintln!("SKIP: active_workspace_is_not_gated — requires Rust-backend mode");
        return;
    }
    let ctx = ctx.unwrap();
    // setup_auth_context's created workspace defaults to 'active' — no
    // explicit set_subscription_status call needed.

    let base = start_probe_server(ctx.db.clone(), ctx.jwt_secret.clone(), false).await;
    let resp = probe(&base, &ctx.access_token).await;
    assert_eq!(resp.status(), 200, "an active SaaS workspace must not be gated");

    cleanup_test_user(&ctx.db, "billgate-test-active-state@contract-test.local").await;
}

#[tokio::test]
async fn trialing_workspace_is_not_gated() {
    let ctx = setup_auth_context("trialing-state").await;
    if ctx.is_none() {
        eprintln!("SKIP: trialing_workspace_is_not_gated — requires Rust-backend mode");
        return;
    }
    let ctx = ctx.unwrap();
    let future_trial_end = chrono::Utc::now() + chrono::Duration::days(14);
    kyomi_core::db_execute!(
        &ctx.db,
        "UPDATE workspaces SET subscription_status = 'trialing', trial_ends_at = $1 \
         WHERE workspace_id = $2",
        future_trial_end,
        &ctx.workspace_id
    )
    .expect("should set trialing state");

    let base = start_probe_server(ctx.db.clone(), ctx.jwt_secret.clone(), false).await;
    let resp = probe(&base, &ctx.access_token).await;
    assert_eq!(resp.status(), 200, "a workspace still inside its trial must not be gated");

    cleanup_test_user(&ctx.db, "billgate-test-trialing-state@contract-test.local").await;
}

#[tokio::test]
async fn scheduled_cancellation_in_grace_period_is_not_gated() {
    let ctx = setup_auth_context("sched-cancel").await;
    if ctx.is_none() {
        eprintln!("SKIP: scheduled_cancellation_in_grace_period_is_not_gated — requires Rust-backend mode");
        return;
    }
    let ctx = ctx.unwrap();
    set_scheduled_cancellation(&ctx).await;

    let base = start_probe_server(ctx.db.clone(), ctx.jwt_secret.clone(), false).await;
    let resp = probe(&base, &ctx.access_token).await;
    assert_eq!(
        resp.status(),
        200,
        "a scheduled cancellation (cancel_at_period_end) still inside its paid-up period must not be gated"
    );

    cleanup_test_user(&ctx.db, "billgate-test-sched-cancel@contract-test.local").await;
}

#[tokio::test]
async fn past_due_workspace_is_gated() {
    let ctx = setup_auth_context("past-due-state").await;
    if ctx.is_none() {
        eprintln!("SKIP: past_due_workspace_is_gated — requires Rust-backend mode");
        return;
    }
    let ctx = ctx.unwrap();
    set_subscription_status(&ctx, "past_due").await;

    let base = start_probe_server(ctx.db.clone(), ctx.jwt_secret.clone(), false).await;
    let resp = probe(&base, &ctx.access_token).await;
    assert_eq!(resp.status(), 402);

    cleanup_test_user(&ctx.db, "billgate-test-past-due-state@contract-test.local").await;
}

// ===========================================================================
// Self-hosted / personal mode: never gated, even with a past_due row
// ===========================================================================

#[tokio::test]
async fn self_hosted_mode_never_gates_even_with_past_due_workspace() {
    let ctx = setup_auth_context("self-hosted-mode").await;
    if ctx.is_none() {
        eprintln!("SKIP: self_hosted_mode_never_gates_even_with_past_due_workspace — requires Rust-backend mode");
        return;
    }
    let ctx = ctx.unwrap();
    set_subscription_status(&ctx, "past_due").await;

    let base = start_probe_server(ctx.db.clone(), ctx.jwt_secret.clone(), true).await;
    let resp = probe(&base, &ctx.access_token).await;
    assert_eq!(
        resp.status(),
        200,
        "self-hosted/personal mode must never 402, even with a past_due workspace row"
    );

    cleanup_test_user(&ctx.db, "billgate-test-self-hosted-mode@contract-test.local").await;
}

// ===========================================================================
// A representative data REST route: POST /api/v1/query-arrow
// ===========================================================================

#[tokio::test]
async fn dashboard_list_returns_json_when_active_and_402_when_lapsed() {
    let ctx = setup_auth_context("dashboard-list").await;
    if ctx.is_none() {
        eprintln!("SKIP: dashboard_list_returns_json_when_active_and_402_when_lapsed — requires Rust-backend mode");
        return;
    }
    let ctx = ctx.unwrap();
    let url = format!("{}/api/v1/dashboards", ctx.base_url);
    let cookie = format!("access_token={}", ctx.access_token);

    let active_resp = client()
        .get(&url)
        .header("cookie", &cookie)
        .send()
        .await
        .expect("dashboard list request should succeed at the transport level");
    assert_eq!(active_resp.status(), 200, "active workspace should be able to list dashboards");
    assert_eq!(
        active_resp.headers()[reqwest::header::CONTENT_TYPE],
        "application/json",
        "dashboard list must be an API response rather than SPA HTML"
    );
    let dashboards: Value = active_resp.json().await.expect("dashboard list should be JSON");
    assert!(dashboards.is_array(), "dashboard list should be a JSON array");

    set_subscription_status(&ctx, "past_due").await;
    let lapsed_resp = client()
        .get(&url)
        .header("cookie", &cookie)
        .send()
        .await
        .expect("lapsed dashboard list request should succeed at the transport level");
    assert_eq!(lapsed_resp.status(), 402, "lapsed workspace must be gated before listing dashboards");
    let body: Value = lapsed_resp.json().await.expect("billing error should be JSON");
    assert_eq!(body["error"], "payment_required");

    cleanup_test_user(&ctx.db, "billgate-test-dashboard-list@contract-test.local").await;
}

#[tokio::test]
async fn query_arrow_returns_402_for_lapsed_workspace() {
    let ctx = setup_auth_context("query-arrow").await;
    if ctx.is_none() {
        eprintln!("SKIP: query_arrow_returns_402_for_lapsed_workspace — requires Rust-backend mode");
        return;
    }
    let ctx = ctx.unwrap();
    set_subscription_status(&ctx, "past_due").await;

    let resp = client()
        .post(format!("{}/api/v1/query-arrow", ctx.base_url))
        .header("origin", "http://localhost:5173")
        .header("content-type", "application/json")
        .header("cookie", format!("access_token={}", ctx.access_token))
        .body(json!({"datasource_slug": "does-not-matter", "sql": "SELECT 1"}).to_string())
        .send()
        .await
        .expect("query-arrow request should succeed at the transport level");

    // `AuthUser` is extracted before the request body is even parsed, so
    // this 402 fires regardless of whether "does-not-matter" is a real
    // datasource — the gate is upstream of any query-execution logic.
    assert_eq!(
        resp.status(),
        402,
        "a data REST route taking a bare AuthUser must 402 for a lapsed workspace"
    );
    let body: Value = resp.json().await.expect("should return JSON");
    assert_eq!(body["error"], "payment_required");

    cleanup_test_user(&ctx.db, "billgate-test-query-arrow@contract-test.local").await;
}

// ===========================================================================
// MCP: initialize 402s for a lapsed workspace
// ===========================================================================

#[tokio::test]
async fn mcp_initialize_returns_402_for_lapsed_workspace() {
    let ctx = setup_auth_context("mcp-init").await;
    if ctx.is_none() {
        eprintln!("SKIP: mcp_initialize_returns_402_for_lapsed_workspace — requires Rust-backend mode");
        return;
    }
    let ctx = ctx.unwrap();
    set_subscription_status(&ctx, "past_due").await;

    let resp = client()
        .post(format!("{}/mcp", ctx.base_url))
        .header("origin", "http://localhost:5173")
        .header("content-type", "application/json")
        .header("cookie", format!("access_token={}", ctx.access_token))
        .body(
            json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize"
            })
            .to_string(),
        )
        .send()
        .await
        .expect("MCP request should succeed at the transport level");

    assert_eq!(
        resp.status(),
        402,
        "MCP's POST /mcp handler takes a bare AuthUser (handle_mcp_request) — a lapsed \
         workspace must 402 before any JSON-RPC method dispatch, including initialize"
    );

    cleanup_test_user(&ctx.db, "billgate-test-mcp-init@contract-test.local").await;
}

#[tokio::test]
async fn mcp_tools_call_without_session_header_returns_402_for_lapsed_workspace() {
    let ctx = setup_auth_context("mcp-tools-call").await;
    if ctx.is_none() {
        eprintln!("SKIP: mcp_tools_call_without_session_header_returns_402_for_lapsed_workspace — requires Rust-backend mode");
        return;
    }
    let ctx = ctx.unwrap();
    set_subscription_status(&ctx, "past_due").await;

    let resp = client()
        .post(format!("{}/mcp", ctx.base_url))
        .header("origin", "http://localhost:5173")
        .header("content-type", "application/json")
        .header("cookie", format!("access_token={}", ctx.access_token))
        // Deliberately no Mcp-Session-Id header and no prior `initialize`
        // call: `handle_mcp_request` allows a missing session header for
        // backwards compatibility (see its own doc comment), so `tools/call`
        // reaches its dispatch arm regardless. `AuthUser` is still extracted
        // before that dispatch, so the gate must fire the same as it does
        // for every other method.
        .body(
            json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "tools/call",
                "params": {"name": "does-not-matter", "arguments": {}}
            })
            .to_string(),
        )
        .send()
        .await
        .expect("MCP request should succeed at the transport level");

    assert_eq!(
        resp.status(),
        402,
        "tools/call with no Mcp-Session-Id header and no prior initialize call must still \
         402 for a lapsed workspace — AuthUser extraction happens before method dispatch"
    );
    let body: Value = resp.json().await.expect("should return JSON");
    assert_eq!(body["error"], "payment_required");

    cleanup_test_user(&ctx.db, "billgate-test-mcp-tools-call@contract-test.local").await;
}

// ===========================================================================
// Allowlisted server fns return 200 for a lapsed workspace
// ===========================================================================
//
// Hit via the real server fn HTTP route (`<T as ServerFn>::PATH`, which
// resolves the macro-generated path — including its compile-time hash —
// rather than guessing at it), matching the PostUrl codec both functions
// use: POST, `application/x-www-form-urlencoded`, empty body (neither
// takes any arguments).

fn server_fn_url<T: leptos::server_fn::ServerFn>(base: &str) -> String {
    format!("{base}{}", T::PATH)
}

#[tokio::test]
async fn get_user_context_returns_200_for_lapsed_workspace() {
    let ctx = setup_auth_context("get-user-context").await;
    if ctx.is_none() {
        eprintln!("SKIP: get_user_context_returns_200_for_lapsed_workspace — requires Rust-backend mode");
        return;
    }
    let ctx = ctx.unwrap();
    set_subscription_status(&ctx, "past_due").await;

    let url = server_fn_url::<kyomi_ui::server_fns::context::GetUserContext>(&ctx.base_url);
    let resp = client()
        .post(url)
        .header("origin", "http://localhost:5173")
        .header("content-type", "application/x-www-form-urlencoded")
        .header("cookie", format!("access_token={}", ctx.access_token))
        .body("")
        .send()
        .await
        .expect("get_user_context request should succeed at the transport level");

    assert_eq!(
        resp.status(),
        200,
        "get_user_context is allowlisted (KYO-805) — it must keep working for a lapsed workspace"
    );

    cleanup_test_user(&ctx.db, "billgate-test-get-user-context@contract-test.local").await;
}

#[tokio::test]
async fn get_sidebar_user_returns_200_for_lapsed_workspace_and_reports_billing_lapsed() {
    let ctx = setup_auth_context("get-sidebar-user").await;
    if ctx.is_none() {
        eprintln!("SKIP: get_sidebar_user_returns_200_for_lapsed_workspace_and_reports_billing_lapsed — requires Rust-backend mode");
        return;
    }
    let ctx = ctx.unwrap();
    set_subscription_status(&ctx, "past_due").await;

    let url = server_fn_url::<kyomi_ui::server_fns::sidebar::GetSidebarUser>(&ctx.base_url);
    let resp = client()
        .post(url)
        .header("origin", "http://localhost:5173")
        .header("content-type", "application/x-www-form-urlencoded")
        .header("cookie", format!("access_token={}", ctx.access_token))
        .body("")
        .send()
        .await
        .expect("get_sidebar_user request should succeed at the transport level");

    assert_eq!(
        resp.status(),
        200,
        "get_sidebar_user is allowlisted (KYO-805) — it must keep working for a lapsed \
         workspace, since it's how the client learns billing_lapsed at all"
    );
    let body: Value = resp.json().await.expect("should return JSON");
    assert_eq!(
        body["billing_lapsed"], true,
        "get_sidebar_user must report billing_lapsed=true for a past_due workspace"
    );

    cleanup_test_user(&ctx.db, "billgate-test-get-sidebar-user@contract-test.local").await;
}

// ===========================================================================
// A gated (non-allowlisted) server fn 402s for a lapsed workspace
// ===========================================================================
//
// The `kyo_805_billing_gate_allowlist_tests` source-inspection tests in
// `crates/kyomi-ui/src/server_fns/mod.rs` prove which extractor each
// function *calls* — they never run `extract_auth()`'s `PaymentRequired`
// branch, and they can't see whether `ResponseOptions::set_status` actually
// reaches the HTTP response leptos_axum sends. This test exercises the real
// HTTP path end to end for `get_recent_sessions` (sidebar.rs) — deliberately
// NOT on the KYO-805 allowlist, so it must be gated like any other ordinary
// server fn — with both the negative (lapsed → 402) and positive (active →
// 200) cases on the same workspace.

#[tokio::test]
async fn get_recent_sessions_returns_402_for_lapsed_workspace_and_200_for_active() {
    let ctx = setup_auth_context("get-recent-sessions").await;
    if ctx.is_none() {
        eprintln!("SKIP: get_recent_sessions_returns_402_for_lapsed_workspace_and_200_for_active — requires Rust-backend mode");
        return;
    }
    let ctx = ctx.unwrap();

    let url = server_fn_url::<kyomi_ui::server_fns::sidebar::GetRecentSessions>(&ctx.base_url);

    // Positive control: setup_auth_context's workspace defaults to 'active'
    // — get_recent_sessions must behave completely normally.
    let active_resp = client()
        .post(&url)
        .header("origin", "http://localhost:5173")
        .header("content-type", "application/x-www-form-urlencoded")
        .header("cookie", format!("access_token={}", ctx.access_token))
        .body("")
        .send()
        .await
        .expect("get_recent_sessions request should succeed at the transport level");
    assert_eq!(
        active_resp.status(),
        200,
        "get_recent_sessions is not allowlisted — it must still return 200 for an active workspace"
    );

    // Now lapse the same workspace and confirm the gate fires over real HTTP.
    set_subscription_status(&ctx, "past_due").await;

    let lapsed_resp = client()
        .post(&url)
        .header("origin", "http://localhost:5173")
        .header("content-type", "application/x-www-form-urlencoded")
        .header("cookie", format!("access_token={}", ctx.access_token))
        .body("")
        .send()
        .await
        .expect("get_recent_sessions request should succeed at the transport level");

    assert_eq!(
        lapsed_resp.status(),
        402,
        "get_recent_sessions is NOT on the KYO-805 allowlist — a lapsed workspace must 402, \
         proving extract_auth()'s PaymentRequired branch and ResponseOptions::set_status reach \
         the real HTTP response, not just that the source calls extract_auth()"
    );
    let body = lapsed_resp.text().await.expect("should read response body");
    assert!(
        body.contains(kyomi_core::PAYMENT_REQUIRED_CODE),
        "the 402 response body must carry the payment_required marker, got: {body:?}"
    );

    cleanup_test_user(&ctx.db, "billgate-test-get-recent-sessions@contract-test.local").await;
}

// ===========================================================================
// Allowlisted billing / workspace-switching server fns return 200 for a
// lapsed workspace
// ===========================================================================

#[tokio::test]
async fn get_subscription_info_returns_200_for_lapsed_workspace() {
    let ctx = setup_auth_context("get-subscription-info").await;
    if ctx.is_none() {
        eprintln!("SKIP: get_subscription_info_returns_200_for_lapsed_workspace — requires Rust-backend mode");
        return;
    }
    let ctx = ctx.unwrap();
    set_subscription_status(&ctx, "past_due").await;

    // `setup_auth_context` makes the caller the workspace's owner
    // (`create_workspace_for_user`), which is what `get_subscription_info`'s
    // `require_workspace_owner` gate needs — a non-owner would 403 here for
    // an unrelated reason before the billing gate is even reached.
    let url = server_fn_url::<kyomi_ui::server_fns::billing::GetSubscriptionInfo>(&ctx.base_url);
    let resp = client()
        .post(url)
        .header("origin", "http://localhost:5173")
        .header("content-type", "application/x-www-form-urlencoded")
        .header("cookie", format!("access_token={}", ctx.access_token))
        .body("")
        .send()
        .await
        .expect("get_subscription_info request should succeed at the transport level");

    assert_eq!(
        resp.status(),
        200,
        "get_subscription_info is allowlisted (KYO-805) — a lapsed workspace's owner must \
         still be able to view billing info in order to fix it"
    );

    cleanup_test_user(&ctx.db, "billgate-test-get-subscription-info@contract-test.local").await;
}

#[tokio::test]
async fn list_my_workspaces_returns_200_for_lapsed_workspace() {
    let ctx = setup_auth_context("list-my-workspaces").await;
    if ctx.is_none() {
        eprintln!("SKIP: list_my_workspaces_returns_200_for_lapsed_workspace — requires Rust-backend mode");
        return;
    }
    let ctx = ctx.unwrap();
    set_subscription_status(&ctx, "past_due").await;

    let url = server_fn_url::<kyomi_ui::server_fns::workspace::ListMyWorkspaces>(&ctx.base_url);
    let resp = client()
        .post(url)
        .header("origin", "http://localhost:5173")
        .header("content-type", "application/x-www-form-urlencoded")
        .header("cookie", format!("access_token={}", ctx.access_token))
        .body("")
        .send()
        .await
        .expect("list_my_workspaces request should succeed at the transport level");

    assert_eq!(
        resp.status(),
        200,
        "list_my_workspaces is allowlisted (KYO-805) — a member of several workspaces must \
         not be trapped in a lapsed one, and this is how they find the others to switch to"
    );

    cleanup_test_user(&ctx.db, "billgate-test-list-my-workspaces@contract-test.local").await;
}
