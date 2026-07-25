// SPDX-License-Identifier: AGPL-3.0-or-later

//! Contract tests for `POST /api/v1/push/subscribe` endpoint validation.
//!
//! KYO-219 — `endpoint` used to be stored with no scheme/host validation, and
//! the server later POSTs to it carrying a VAPID-signed `Authorization`
//! header. These tests assert the route-level contract of the fix: rejected
//! endpoints return 4xx and write no row; a legitimate endpoint is accepted
//! and stored.
//!
//! Unit coverage for the shared predicate itself
//! (`kyomi_auth::push_service::validate_push_endpoint`) lives in
//! `crates/kyomi-auth/src/push_service.rs`; the egress re-validation lives in
//! `crates/kyomi-agent/src/web_push.rs`. This file only covers the HTTP
//! contract at the ingress route.

use serde_json::json;

use kyomi_test_harness::{cleanup_test_user, setup_auth_context};

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap()
}

fn auth_post(base: &str, path: &str, token: &str) -> reqwest::RequestBuilder {
    client()
        .post(format!("{base}{path}"))
        .header("origin", "http://localhost:5173")
        .header("content-type", "application/json")
        .header("cookie", format!("access_token={token}"))
}

fn subscribe_body(endpoint: &str) -> String {
    json!({
        "endpoint": endpoint,
        "p256dh": "BNvsCh-iTzovU3ujQD_THGIPeFKTMjZV0V4GN3IeN5otYKGgFtPEQ7IC0D0kGE8VPil54L_IcWUsIMIjpwa2bww",
        "auth": "BdoZ3UeQMCUdmCzzw6OuDg"
    })
    .to_string()
}

/// Count `push_subscriptions` rows for a user with a given endpoint.
async fn subscription_row_count(db: &kyomi_core::DbPool, user_id: &str, endpoint: &str) -> i64 {
    kyomi_core::db_fetch_scalar!(
        db,
        i64,
        "SELECT COUNT(*) FROM push_subscriptions WHERE user_id = $1 AND endpoint = $2",
        user_id,
        endpoint
    )
    .expect("count push_subscriptions rows")
}

// ===========================================================================
// Reject table (KYO-219)
// ===========================================================================

async fn assert_subscribe_rejected(email_prefix_suffix: &str, endpoint: &str) {
    let ctx = setup_auth_context("Push SSRF Test User", "push-ssrf", email_prefix_suffix).await;
    if ctx.is_none() {
        eprintln!(
            "SKIP: subscribe_rejects [{endpoint}] — requires Rust-backend mode"
        );
        return;
    }
    let ctx = ctx.unwrap();

    let resp = auth_post(&ctx.base_url, "/api/v1/push/subscribe", &ctx.access_token)
        .body(subscribe_body(endpoint))
        .send()
        .await
        .expect("subscribe request should succeed at the transport level");

    assert!(
        resp.status().is_client_error(),
        "endpoint {endpoint:?} should be rejected with a 4xx, got {}",
        resp.status()
    );

    let count = subscription_row_count(&ctx.db, &ctx.user_id, endpoint).await;
    assert_eq!(
        count, 0,
        "rejected endpoint {endpoint:?} must not be written to push_subscriptions"
    );

    cleanup_test_user(
        &ctx.db,
        &format!("push-ssrf-test-{email_prefix_suffix}@contract-test.local"),
    )
    .await;
}

#[tokio::test]
async fn subscribe_rejects_plain_http() {
    assert_subscribe_rejected("http", "http://fcm.googleapis.com/fcm/send/abc").await;
}

#[tokio::test]
async fn subscribe_rejects_loopback() {
    assert_subscribe_rejected("loopback", "https://127.0.0.1/x").await;
}

#[tokio::test]
async fn subscribe_rejects_localhost() {
    assert_subscribe_rejected("localhost", "https://localhost/x").await;
}

#[tokio::test]
async fn subscribe_rejects_cloud_metadata_address() {
    assert_subscribe_rejected(
        "metadata",
        "https://169.254.169.254/latest/meta-data/",
    )
    .await;
}

#[tokio::test]
async fn subscribe_rejects_rfc1918_address() {
    assert_subscribe_rejected("rfc1918", "https://10.0.0.1/x").await;
}

#[tokio::test]
async fn subscribe_rejects_ipv6_loopback() {
    assert_subscribe_rejected("ipv6loop", "https://[::1]/x").await;
}

#[tokio::test]
async fn subscribe_rejects_embedded_credentials() {
    assert_subscribe_rejected(
        "creds",
        "https://user:pass@fcm.googleapis.com/x",
    )
    .await;
}

#[tokio::test]
async fn subscribe_rejects_file_scheme() {
    assert_subscribe_rejected("file", "file:///etc/passwd").await;
}

#[tokio::test]
async fn subscribe_rejects_suffix_boundary_bypass() {
    assert_subscribe_rejected(
        "boundary",
        "https://evilgoogleapis.com/fcm/send/abc",
    )
    .await;
}

#[tokio::test]
async fn subscribe_rejects_unrecognized_host() {
    assert_subscribe_rejected("unrecognized", "https://attacker.example/collect").await;
}

// ===========================================================================
// Accept table (KYO-219) — real browser push-service URL shapes
// ===========================================================================

#[tokio::test]
async fn subscribe_accepts_real_fcm_endpoint() {
    let ctx = setup_auth_context("Push SSRF Test User", "push-ssrf", "fcm-accept").await;
    if ctx.is_none() {
        eprintln!("SKIP: subscribe_accepts_real_fcm_endpoint — requires Rust-backend mode");
        return;
    }
    let ctx = ctx.unwrap();
    let endpoint = "https://fcm.googleapis.com/fcm/send/dGVzdC1zdWJzY3JpcHRpb24taWQ";

    let resp = auth_post(&ctx.base_url, "/api/v1/push/subscribe", &ctx.access_token)
        .body(subscribe_body(endpoint))
        .send()
        .await
        .expect("subscribe request should succeed");

    assert_eq!(
        resp.status(),
        200,
        "a real FCM endpoint should be accepted"
    );

    let count = subscription_row_count(&ctx.db, &ctx.user_id, endpoint).await;
    assert_eq!(count, 1, "accepted endpoint should be stored exactly once");

    cleanup_test_user(&ctx.db, "push-ssrf-test-fcm-accept@contract-test.local").await;
}

#[tokio::test]
async fn subscribe_accepts_real_mozilla_autopush_endpoint() {
    let ctx = setup_auth_context("Push SSRF Test User", "push-ssrf", "moz-accept").await;
    if ctx.is_none() {
        eprintln!(
            "SKIP: subscribe_accepts_real_mozilla_autopush_endpoint — requires Rust-backend mode"
        );
        return;
    }
    let ctx = ctx.unwrap();
    let endpoint =
        "https://updates.push.services.mozilla.com/wpush/v2/gAAAAABtest-token-value";

    let resp = auth_post(&ctx.base_url, "/api/v1/push/subscribe", &ctx.access_token)
        .body(subscribe_body(endpoint))
        .send()
        .await
        .expect("subscribe request should succeed");

    assert_eq!(
        resp.status(),
        200,
        "a real Mozilla autopush endpoint should be accepted"
    );

    let count = subscription_row_count(&ctx.db, &ctx.user_id, endpoint).await;
    assert_eq!(count, 1, "accepted endpoint should be stored exactly once");

    cleanup_test_user(&ctx.db, "push-ssrf-test-moz-accept@contract-test.local").await;
}

// ===========================================================================
// Migration purge logic (KYO-219) — LIVE Postgres, not just the SQLite unit
// test in crates/kyomi-auth/src/push_service.rs.
// ===========================================================================
//
// This migration's host-extraction logic has been wrong THREE times in
// review, each time empirically proven against a live database, not just
// theorized. `url::Url` implements the WHATWG URL Standard, not RFC 3986 —
// hand-written SQL cannot be proven equivalent to it, which is why this SQL
// migration is now a coarse best-effort first pass and NOT the authoritative
// purge (see the migration file's own comments). The authoritative purge is
// the Rust startup sweep, `kyomi_auth::push_service::purge_invalid_subscriptions`,
// which calls `validate_push_endpoint` directly and runs on every boot.
//
//   1. `endpoint ILIKE 'https://%.googleapis.com/%'` matches the suffix
//      appearing ANYWHERE in the string, including the path. A row with
//      real host `attacker.example` and endpoint
//      `https://attacker.example/.googleapis.com/x` survived.
//   2. Bounding the authority only at `/` let a URL with no `/` but a
//      crafted query string or fragment survive instead:
//      `https://evil.com?x=y.googleapis.com` and
//      `https://evil.com#y.googleapis.com` both extracted a "host" that
//      still ended in `.googleapis.com`.
//   3. WHATWG (unlike RFC 3986) treats `\` as equivalent to `/` for special
//      schemes including `https`. `https://evil.com\.googleapis.com/x` has
//      real host `evil.com` per `url::Url`, but bounding the authority only
//      at `/`, `?`, `#` (not also `\`) extracted a "host" that still ended
//      in `.googleapis.com`.
//
// The current migration bounds the authority at the first of `/`, `?`, `#`,
// OR `\`, and this test locks in all three bypass classes plus the userinfo
// case (`user@host`, both orderings) so they can't regress a fourth time.
// Everything here runs inside a transaction that is rolled back at the end,
// so this test never permanently mutates the shared dev database — it
// proves the migration file's actual DELETE statement against real Postgres
// semantics (ILIKE, `substring(... from ...)`, `USING`) without touching any
// other row in `push_subscriptions`.

const POSTGRES_PURGE_MIGRATION_SQL: &str =
    include_str!("../migrations/20260725010000_purge_invalid_push_endpoints.sql");

#[tokio::test]
async fn postgres_purge_migration_rejects_authority_smuggling_bypasses_on_live_db() {
    let ctx = setup_auth_context("Push SSRF Migration Test User", "push-ssrf-mig", "pg-purge").await;
    if ctx.is_none() {
        eprintln!(
            "SKIP: postgres_purge_migration_rejects_authority_smuggling_bypasses_on_live_db — requires Rust-backend mode"
        );
        return;
    }
    let ctx = ctx.unwrap();

    let kyomi_core::db::DbPool::Postgres(pg) = &ctx.db else {
        eprintln!(
            "SKIP: postgres_purge_migration_rejects_authority_smuggling_bypasses_on_live_db — requires a live Postgres pool (got SQLite)"
        );
        cleanup_test_user(&ctx.db, "push-ssrf-mig-test-pg-purge@contract-test.local").await;
        return;
    };

    let mut tx = pg.begin().await.expect("begin transaction");

    let good = [
        "https://fcm.googleapis.com/fcm/send/abc",
        "https://updates.push.services.mozilla.com/wpush/v2/abc",
        "https://googleapis.com/fcm/send/abc",
    ];
    let bad = [
        "http://fcm.googleapis.com/fcm/send/abc",
        "https://127.0.0.1/x",
        "https://user:pass@fcm.googleapis.com/x",
        "https://evilgoogleapis.com/fcm/send/abc",
        // Path-smuggling (recurrence #1): real host is attacker.example, the
        // allowlisted suffix is smuggled into the PATH. An unanchored
        // `LIKE '%.googleapis.com/%'` matches this even though the host
        // never touches googleapis.com.
        "https://attacker.example/.googleapis.com/x",
        // Query/fragment-smuggling (recurrence #2): real host is evil.com,
        // the suffix is smuggled into the query string / fragment. An
        // authority extraction bounded only by "/" (not also "?" and "#")
        // extracts "evil.com?x=y.googleapis.com" / "evil.com#y.googleapis.com"
        // as the "host", which still ends in the allowed suffix.
        "https://evil.com?x=y.googleapis.com",
        "https://evil.com#y.googleapis.com",
        // Userinfo, both orderings — caught by the separate embedded-
        // credentials check, not by host-suffix matching, but locked in
        // here so a future refactor of the host-suffix logic alone can't
        // silently reopen this.
        "https://evil.com@fcm.googleapis.com/x",
        "https://fcm.googleapis.com@evil.com/x",
        // Backslash-smuggling (recurrence #3): WHATWG treats "\" as
        // equivalent to "/" for the https scheme — real host per `url::Url`
        // is evil.com, but bounding the authority only at "/", "?", "#"
        // (not also "\") extracted "evil.com\.googleapis.com" as the
        // "host", which still ends in the allowed suffix.
        "https://evil.com\\.googleapis.com/x",
    ];

    for (i, endpoint) in good.iter().chain(bad.iter()).enumerate() {
        sqlx::query(
            "INSERT INTO push_subscriptions (user_id, endpoint, p256dh, auth) VALUES ($1, $2, 'p', 'a')",
        )
        .bind(&ctx.user_id)
        .bind(*endpoint)
        .execute(&mut *tx)
        .await
        .unwrap_or_else(|e| panic!("seed subscription {i} ({endpoint}): {e}"));
    }

    // Run the exact statement shipped in the migration file — not a
    // hand-rewritten approximation — against real Postgres.
    sqlx::query(POSTGRES_PURGE_MIGRATION_SQL)
        .execute(&mut *tx)
        .await
        .expect("run purge statement on live Postgres");

    let remaining: Vec<String> = sqlx::query_scalar(
        "SELECT endpoint FROM push_subscriptions WHERE user_id = $1",
    )
    .bind(&ctx.user_id)
    .fetch_all(&mut *tx)
    .await
    .expect("fetch remaining endpoints");

    for endpoint in &good {
        assert!(
            remaining.contains(&endpoint.to_string()),
            "legitimate endpoint {endpoint} was incorrectly purged on live Postgres"
        );
    }
    for endpoint in &bad {
        assert!(
            !remaining.contains(&endpoint.to_string()),
            "malicious/smuggled endpoint {endpoint} survived the LIVE POSTGRES purge migration"
        );
    }
    assert_eq!(
        remaining.len(),
        good.len(),
        "unexpected row count after live Postgres purge: {remaining:?}"
    );

    // Never commit — leaves the shared dev database untouched.
    tx.rollback().await.expect("rollback transaction");

    cleanup_test_user(&ctx.db, "push-ssrf-mig-test-pg-purge@contract-test.local").await;
}
