// SPDX-License-Identifier: AGPL-3.0-or-later

//! Regression tests for KYO-106 — Stripe webhook `subscription_period_start` /
//! `subscription_period_end` bind typing.
//!
//! # The bug
//!
//! `handle_subscription_event` in `apps/server/src/routes/billing.rs` used to
//! bind `Option<&str>` (produced by `DateTime::to_rfc3339()`) to the
//! `timestamp with time zone` columns `subscription_period_start` and
//! `subscription_period_end`. Postgres rejects this with
//! `column "subscription_period_start" is of type timestamp with time zone but
//! expression is of type text`. SQLite silently coerces, which is why it never
//! surfaced locally — only in prod SaaS (Postgres) where real users were
//! blocked on bundle / subscription purchases.
//!
//! # What these tests lock in
//!
//! 1. `webhook_update_rejects_text_bind_for_timestamptz_column` — the
//!    production bug. Binding the RFC3339 string to the timestamptz column
//!    MUST fail with the specific Postgres error. This guards against a
//!    reintroduction.
//! 2. `webhook_update_accepts_datetime_bind_for_timestamptz_column` — the
//!    fix. Binding `Option<DateTime<Utc>>` MUST succeed and the row MUST be
//!    readable as a `DateTime<Utc>` equal to what was written.
//!
//! Both tests use the same `UPDATE workspaces … SET subscription_period_start
//! = $N, subscription_period_end = $N+1 …` statement that
//! `handle_subscription_event` issues in production.

use chrono::{DateTime, TimeZone, Utc};
use kyomi_test_harness::{cleanup_test_user, setup_auth_context};
use sqlx::Row;

/// The exact UPDATE statement emitted by
/// `handle_subscription_event` on `customer.subscription.updated`
/// (`apps/server/src/routes/billing.rs`). Kept in sync with production.
const WEBHOOK_UPDATE_SQL: &str = "UPDATE workspaces SET \
     subscription_tier = $1, \
     subscription_status = $2, \
     billing_cycle = $3, \
     subscription_period_start = $4, \
     subscription_period_end = $5, \
     user_limit = $6 \
     WHERE workspace_id = $7";

/// Skip-or-unwrap a Postgres pool from the test harness. Returns `None` if
/// tests are running against SQLite (which doesn't reproduce the bug).
async fn pg_pool_or_skip(
    prefix: &str,
    suffix: &str,
) -> Option<(kyomi_test_harness::AuthContext, sqlx::PgPool)> {
    let ctx = setup_auth_context("KYO-106 Regression", prefix, suffix).await?;
    match &ctx.db {
        kyomi_core::db::DbPool::Postgres(pg) => {
            let pg = pg.clone();
            Some((ctx, pg))
        }
        kyomi_core::db::DbPool::Sqlite(_) => {
            eprintln!(
                "SKIP: KYO-106 regression test requires Postgres (SQLite silently coerces \
                 text → timestamptz and cannot reproduce the bug)"
            );
            // Cleanup then bail.
            cleanup_test_user(
                &ctx.db,
                &format!("{prefix}-test-{suffix}@contract-test.local"),
            )
            .await;
            None
        }
    }
}

/// Reproduces the exact production bug: binding the RFC3339 string form of a
/// timestamp (what `DateTime::to_rfc3339()` produces) to the `timestamptz`
/// columns MUST be rejected by Postgres.
///
/// The error message is asserted explicitly so that if a future change
/// silently starts accepting text binds (e.g. via a cast in the SQL) this
/// test will surface it rather than pass trivially.
#[tokio::test]
async fn webhook_update_rejects_text_bind_for_timestamptz_column() {
    let Some((ctx, pg)) = pg_pool_or_skip("kyo106-red", "text-bind").await else {
        return;
    };

    // Reproduce the exact transformation production used to perform.
    let period_start: DateTime<Utc> = Utc.with_ymd_and_hms(2026, 4, 1, 0, 0, 0).unwrap();
    let period_end: DateTime<Utc> = Utc.with_ymd_and_hms(2026, 5, 1, 0, 0, 0).unwrap();
    let period_start_str: Option<String> = Some(period_start).map(|dt| dt.to_rfc3339());
    let period_end_str: Option<String> = Some(period_end).map(|dt| dt.to_rfc3339());

    let result = sqlx::query(WEBHOOK_UPDATE_SQL)
        .bind("cloud")
        .bind("active")
        .bind(Some("monthly"))
        .bind(period_start_str.as_deref())
        .bind(period_end_str.as_deref())
        .bind(Some(5_i32))
        .bind(&ctx.workspace_id)
        .execute(&pg)
        .await;

    let err = result.expect_err(
        "binding Option<&str> to a timestamptz column must fail — if this succeeds the \
         KYO-106 regression guard is broken",
    );
    let err_str = err.to_string();
    assert!(
        err_str.contains("subscription_period_start")
            && err_str.contains("timestamp with time zone")
            && err_str.contains("text"),
        "expected Postgres to reject text bind for timestamptz column, got: {err_str}"
    );

    cleanup_test_user(&ctx.db, "kyo106-red-test-text-bind@contract-test.local").await;
}

/// Demonstrates the fix: binding `Option<DateTime<Utc>>` directly to the
/// timestamptz columns succeeds, the row is updated, and the values round-trip
/// as expected.
#[tokio::test]
async fn webhook_update_accepts_datetime_bind_for_timestamptz_column() {
    let Some((ctx, pg)) = pg_pool_or_skip("kyo106-green", "dt-bind").await else {
        return;
    };

    let period_start: DateTime<Utc> = Utc.with_ymd_and_hms(2026, 4, 1, 12, 34, 56).unwrap();
    let period_end: DateTime<Utc> = Utc.with_ymd_and_hms(2026, 5, 1, 12, 34, 56).unwrap();

    // Note: we bind Option<DateTime<Utc>> directly — no to_rfc3339() in sight.
    sqlx::query(WEBHOOK_UPDATE_SQL)
        .bind("cloud")
        .bind("active")
        .bind(Some("monthly"))
        .bind(Some(period_start))
        .bind(Some(period_end))
        .bind(Some(5_i32))
        .bind(&ctx.workspace_id)
        .execute(&pg)
        .await
        .expect("DateTime<Utc> bind to timestamptz must succeed after KYO-106 fix");

    // Read back and confirm the timestamps round-tripped intact, plus the
    // other fields the webhook handler updates.
    let row = sqlx::query(
        "SELECT subscription_tier, subscription_status, billing_cycle, \
                subscription_period_start, subscription_period_end, user_limit \
         FROM workspaces WHERE workspace_id = $1",
    )
    .bind(&ctx.workspace_id)
    .fetch_one(&pg)
    .await
    .expect("workspace row must exist");

    let tier: String = row.get("subscription_tier");
    let status: String = row.get("subscription_status");
    let billing_cycle: Option<String> = row.get("billing_cycle");
    let start: Option<DateTime<Utc>> = row.get("subscription_period_start");
    let end: Option<DateTime<Utc>> = row.get("subscription_period_end");
    let user_limit: Option<i32> = row.get("user_limit");

    assert_eq!(tier, "cloud");
    assert_eq!(status, "active");
    assert_eq!(billing_cycle.as_deref(), Some("monthly"));
    assert_eq!(start, Some(period_start));
    assert_eq!(end, Some(period_end));
    assert_eq!(user_limit, Some(5));

    cleanup_test_user(&ctx.db, "kyo106-green-test-dt-bind@contract-test.local").await;
}
