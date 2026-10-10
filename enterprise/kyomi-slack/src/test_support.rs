// SPDX-License-Identifier: LicenseRef-Alytic-Enterprise

//! Shared test fixtures for `kyomi-slack`'s unit tests.
//!
//! Originally lived only inside `routes::tests`; extracted (KYO-823) once
//! `billing_gate::tests` needed the same `test_pool()`/`insert_user()`/
//! `insert_workspace()`/`insert_workspace_user()` shape plus a handful of
//! Slack-specific fixtures (workspace billing fields, a Slack workspace
//! integration row, a platform user link) — a second near-identical copy of
//! these would have been the trigger this codebase's
//! `third-copy-of-test-helper-is-extraction-trigger` standard warns about.
//!
//! `kyomi_auth::test_support` is `#[cfg(test)]`-only inside `kyomi-auth`
//! and not visible to this crate, so it cannot be reused directly — this
//! module is `kyomi-slack`'s own equivalent.

use chrono::{DateTime, Utc};
use sqlx::sqlite::SqlitePoolOptions;

use kyomi_core::DbPool;

/// Build an in-memory SQLite pool with migrations applied.
///
/// Mirrors the `test_pool()` helper in `kyomi_auth::session` /
/// `workspace_service` — the established in-memory-sqlite pattern used
/// across the workspace's unit tests.
pub(crate) async fn test_pool() -> DbPool {
    let _ = kyomi_core::constants::load_with_fallback();

    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("connect in-memory sqlite");

    sqlx::query("PRAGMA foreign_keys=ON")
        .execute(&pool)
        .await
        .expect("enable foreign keys");

    sqlx::migrate!("../../apps/server/migrations-sqlite")
        .run(&pool)
        .await
        .expect("run sqlite migrations");

    DbPool::Sqlite(pool)
}

fn sqlite(pool: &DbPool) -> &sqlx::SqlitePool {
    match pool {
        DbPool::Sqlite(sq) => sq,
        _ => unreachable!("kyomi-slack tests only run against the sqlite pool"),
    }
}

/// Insert a user row with the given id.
pub(crate) async fn insert_user(pool: &DbPool, user_id: &str) {
    sqlx::query("INSERT INTO users (user_id, email) VALUES ($1, $2)")
        .bind(user_id)
        .bind(format!("{user_id}@test.local"))
        .execute(sqlite(pool))
        .await
        .expect("insert user");
}

/// Insert a workspace row owned by `owner_user_id`. Leaves every billing
/// column at its migration default (`subscription_status = 'active'`,
/// `stripe_subscription_id`/`subscription_period_end`/`trial_ends_at` all
/// NULL) — an unmodified row is therefore always billing-open, matching
/// `capability::billing_lapse_reason`'s `Active => None` arm.
pub(crate) async fn insert_workspace(pool: &DbPool, workspace_id: &str, owner_user_id: &str) {
    sqlx::query("INSERT INTO workspaces (workspace_id, name, owner_user_id) VALUES ($1, $2, $3)")
        .bind(workspace_id)
        .bind(format!("Workspace {workspace_id}"))
        .bind(owner_user_id)
        .execute(sqlite(pool))
        .await
        .expect("insert workspace");
}

/// Insert a `workspace_users` row with an explicit `active` flag.
pub(crate) async fn insert_workspace_user(
    pool: &DbPool,
    workspace_id: &str,
    user_id: &str,
    role: &str,
    active: bool,
) {
    sqlx::query(
        "INSERT INTO workspace_users (workspace_id, user_id, role, active) \
         VALUES ($1, $2, $3, $4)",
    )
    .bind(workspace_id)
    .bind(user_id)
    .bind(role)
    .bind(active)
    .execute(sqlite(pool))
    .await
    .expect("insert workspace_users row");
}

/// Set every billing-relevant column on a workspace row in one call, so a
/// test's intent (e.g. "cancelled, scheduled, still in its paid-up grace
/// period") is legible at the call site instead of split across several
/// `UPDATE`s. Mirrors the field set `capability::billing_lapse_reason`
/// actually reads — nothing more.
pub(crate) async fn set_workspace_billing(
    pool: &DbPool,
    workspace_id: &str,
    subscription_status: &str,
    stripe_subscription_id: Option<&str>,
    subscription_period_end: Option<DateTime<Utc>>,
    trial_ends_at: Option<DateTime<Utc>>,
) {
    sqlx::query(
        "UPDATE workspaces SET \
            subscription_status = $1, \
            stripe_subscription_id = $2, \
            subscription_period_end = $3, \
            trial_ends_at = $4 \
         WHERE workspace_id = $5",
    )
    .bind(subscription_status)
    .bind(stripe_subscription_id)
    .bind(subscription_period_end)
    .bind(trial_ends_at)
    .bind(workspace_id)
    .execute(sqlite(pool))
    .await
    .expect("update workspace billing fields");
}

/// Insert a `workspace_integrations` row for the Slack platform, with the
/// config JSON shape `resolve_slack_context`/`lookup_workspace_by_team_id`
/// expect: `{"team_id": ..., "bot_token": <already-encrypted>}`.
pub(crate) async fn insert_slack_workspace_integration(
    pool: &DbPool,
    workspace_id: &str,
    team_id: &str,
    encrypted_bot_token: &str,
) {
    let config = serde_json::json!({
        "team_id": team_id,
        "team_name": "Test Team",
        "bot_token": encrypted_bot_token,
        "bot_user_id": "UBOT123",
    });
    sqlx::query(
        "INSERT INTO workspace_integrations (id, workspace_id, platform_type, config) \
         VALUES ($1, $2, 'slack', $3)",
    )
    .bind(uuid::Uuid::new_v4().to_string())
    .bind(workspace_id)
    .bind(config.to_string())
    .execute(sqlite(pool))
    .await
    .expect("insert workspace_integrations row");
}

/// Insert a `platform_user_links` row linking a Slack user id to a Kyomi
/// user within a workspace — what `resolve_platform_user`/
/// `resolve_slack_context` look up to translate `slack_user_id` into a
/// Kyomi `user_id`.
pub(crate) async fn insert_platform_user_link(
    pool: &DbPool,
    workspace_id: &str,
    user_id: &str,
    slack_user_id: &str,
) {
    sqlx::query(
        "INSERT INTO platform_user_links \
            (id, workspace_id, user_id, platform_type, platform_user_id) \
         VALUES ($1, $2, $3, 'slack', $4)",
    )
    .bind(uuid::Uuid::new_v4().to_string())
    .bind(workspace_id)
    .bind(user_id)
    .bind(slack_user_id)
    .execute(sqlite(pool))
    .await
    .expect("insert platform_user_links row");
}

/// Count `chat_sessions` rows for a workspace — used to prove the KYO-823
/// billing gate stops the pipeline *before* session creation, not merely
/// before the agent call.
pub(crate) async fn count_chat_sessions_for_workspace(pool: &DbPool, workspace_id: &str) -> i64 {
    #[derive(sqlx::FromRow)]
    struct CountRow {
        n: i64,
    }
    let row: CountRow =
        sqlx::query_as("SELECT COUNT(*) as n FROM chat_sessions WHERE workspace_id = $1")
            .bind(workspace_id)
            .fetch_one(sqlite(pool))
            .await
            .expect("count chat_sessions");
    row.n
}

/// Count `chat_messages` rows across all sessions belonging to a workspace
/// — used alongside [`count_chat_sessions_for_workspace`] by the slash
/// command / interaction tests, which have no single session to filter on.
pub(crate) async fn count_chat_messages_for_workspace(pool: &DbPool, workspace_id: &str) -> i64 {
    #[derive(sqlx::FromRow)]
    struct CountRow {
        n: i64,
    }
    let row: CountRow = sqlx::query_as(
        "SELECT COUNT(*) as n FROM chat_messages \
         WHERE session_id IN (SELECT session_id FROM chat_sessions WHERE workspace_id = $1)",
    )
    .bind(workspace_id)
    .fetch_one(sqlite(pool))
    .await
    .expect("count chat_messages");
    row.n
}
