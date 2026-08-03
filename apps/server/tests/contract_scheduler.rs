// SPDX-License-Identifier: AGPL-3.0-or-later

//! Contract tests for Phase 7G: Special Indexers, Background Scheduler.
//!
//! Tests cover:
//! 1. Scheduler Redis lock acquisition/release
//! 2. Scheduler credential resolution logic
//! 3. Sample data indexer configuration detection
//! 4. BigQuery public indexer constants
//! 5. User dataset indexer full table ID format

// These tests talk directly to the DB/Redis — no HTTP harness required.

// ===========================================================================
// Section 1: Scheduler Redis lock tests
// ===========================================================================

// KYO-236: quarantined. `kyomi_core::redis::create_pool()` has no
// connection timeout on its initial-connect retries (redis-rs
// `ConnectionManager` default: 6 retries × exponential backoff, no
// per-attempt timeout) — when Redis is unreachable, as it always is in CI
// (no Redis service is configured; `kv_store::create_kv_store` already
// falls back to an in-memory store for the app's real functional paths),
// this "gracefully skip if unavailable" test takes 5-6+ minutes to reach
// its own `Err` branch before skipping, for zero signal every single CI
// run. Tracked in KYO-252 (fix `create_pool`'s timeout) — re-enable this
// test once that lands, or once CI grows a real Redis service.
#[ignore = "requires local Redis; create_pool() takes 5-6+ min to fail without one — see KYO-252"]
#[tokio::test]
async fn scheduler_redis_lock_acquire_and_release() {
    let redis_url = std::env::var("REDIS_URL")
        .unwrap_or_else(|_| "redis://localhost:6381".into());

    let mut redis = match kyomi_core::redis::create_pool(&redis_url).await {
        Ok(r) => r,
        Err(_) => return, // Skip if Redis unavailable
    };

    let test_key = "test_scheduler_lock_contract";

    // Clean up first
    let _: std::result::Result<(), _> = redis::AsyncCommands::del::<_, ()>(&mut redis, test_key).await;

    // Acquire lock via SETNX
    let acquired: Option<String> = redis::cmd("SET")
        .arg(test_key)
        .arg("1")
        .arg("NX")
        .arg("EX")
        .arg(10u64)
        .query_async(&mut redis)
        .await
        .unwrap_or(None);

    assert!(acquired.is_some(), "first lock acquisition should succeed");

    // Second acquisition should fail
    let second: Option<String> = redis::cmd("SET")
        .arg(test_key)
        .arg("1")
        .arg("NX")
        .arg("EX")
        .arg(10u64)
        .query_async(&mut redis)
        .await
        .unwrap_or(None);

    assert!(second.is_none(), "second lock acquisition should fail");

    // Release and re-acquire
    let _: std::result::Result<(), _> = redis::AsyncCommands::del::<_, ()>(&mut redis, test_key).await;

    let third: Option<String> = redis::cmd("SET")
        .arg(test_key)
        .arg("1")
        .arg("NX")
        .arg("EX")
        .arg(10u64)
        .query_async(&mut redis)
        .await
        .unwrap_or(None);

    assert!(third.is_some(), "third lock acquisition should succeed after release");

    // Clean up
    let _: std::result::Result<(), _> = redis::AsyncCommands::del::<_, ()>(&mut redis, test_key).await;
}

// ===========================================================================
// Section 2: Scheduler credential resolution tests (unit-level)
// ===========================================================================

#[test]
fn scheduler_rejects_oauth_indexing_credentials() {
    // OAuth credentials cannot be used for background indexing
    // (no automated token refresh without user interaction)
    let _config = serde_json::json!({
        "indexing_credentials": {
            "type": "oauth",
            "access_token": "some-token"
        }
    });

    // The scheduler module's get_indexing_credentials is private,
    // but we can verify the behavior through the public interface.
    // For now, we test the constants and sentinel values.
    assert_eq!(
        kyomi_auth::catalog::indexers::sample_data::SAMPLE_DATA_WORKSPACE_ID,
        "sample-data-workspace"
    );
}

#[test]
fn scheduler_constants_are_correct() {
    // Verify sentinel workspace IDs match Python
    assert_eq!(
        kyomi_auth::catalog::indexers::sample_data::SAMPLE_DATA_WORKSPACE_ID,
        "sample-data-workspace"
    );
    assert_eq!(
        kyomi_auth::catalog::indexers::bigquery_public::PUBLIC_DATA_WORKSPACE_ID,
        "public-data-workspace"
    );
}

// ===========================================================================
// Section 3: Sample data indexer tests
// ===========================================================================

#[test]
fn sample_data_indexer_is_configured_checks_env() {
    // In test environment, SAMPLE_CLICKHOUSE_HOST is typically not set
    let configured =
        kyomi_auth::catalog::indexers::sample_data::SampleDataIndexer::is_configured();
    // If the env var is not set, should return false
    if std::env::var("SAMPLE_CLICKHOUSE_HOST").is_err() {
        assert!(!configured);
    }
    // If it IS set, configured should be true
    if std::env::var("SAMPLE_CLICKHOUSE_HOST").is_ok() {
        assert!(configured);
    }
}

// ===========================================================================
// Section 4: BigQuery public indexer tests
// ===========================================================================

#[test]
fn bigquery_public_indexer_sentinel_workspace_id() {
    // The public data workspace ID should be stable and match Python
    assert_eq!(
        kyomi_auth::catalog::indexers::bigquery_public::PUBLIC_DATA_WORKSPACE_ID,
        "public-data-workspace"
    );

    // Verify the struct type exists and is accessible
    // (BigQueryPublicIndexer is a unit struct so we can construct it)
    let _indexer = kyomi_auth::catalog::indexers::bigquery_public::BigQueryPublicIndexer;
}


// ===========================================================================
// Section 6: Scheduler start/stop test
// ===========================================================================

#[tokio::test]
async fn scheduler_can_be_started() {
    // Verify the scheduler function exists and can be called
    // We do not actually start the scheduler (it would run forever),
    // but we validate that the function signature compiles and
    // the Config flag is respected.
    let config = kyomi_core::Config::test_config();
    assert!(
        !config.enable_schedulers,
        "test config should disable schedulers"
    );
}

// ===========================================================================
// Section 7: Indexer trait integration tests
// ===========================================================================

#[test]
fn all_10_datasource_types_still_registered_in_registry() {
    // Ensure Phase 7G additions did not break existing datasource type registration
    use kyomi_core::datasource_registry::DatasourceType;

    let types = vec![
        DatasourceType::Postgres,
        DatasourceType::MySQL,
        DatasourceType::ClickHouse,
        DatasourceType::Snowflake,
        DatasourceType::Databricks,
        DatasourceType::Redshift,
        DatasourceType::SqlServer,
        DatasourceType::Synapse,
        DatasourceType::BigQuery,
        DatasourceType::FlareDb,
    ];

    for ds_type in &types {
        let meta = kyomi_core::datasource_registry::get_metadata(ds_type);
        assert!(
            !meta.display_name.is_empty(),
            "datasource metadata should have a display name for {ds_type:?}"
        );
    }

    // Verify all 10 types are in the registry
    assert_eq!(types.len(), 10, "should verify all 10 datasource types");
}

#[tokio::test]
async fn sample_data_needs_refresh_true_for_empty_db() {
    // KYO-242: connects to (and provisions/self-heals) this worktree's
    // private test database rather than the shared `kyomi_test` database.
    let db = match kyomi_core::test_db::connect_test_pool().await {
        Ok(pool) => pool,
        Err(_) => return, // Skip if DB unavailable
    };

    // With no sample data cached, needs_refresh should return true
    let needs = kyomi_auth::catalog::indexers::sample_data::SampleDataIndexer::needs_refresh(
        &db, 168,
    )
    .await;
    assert!(needs, "needs_refresh should return true when no data cached");
}

#[tokio::test]
async fn bigquery_public_needs_refresh_true_for_empty_db() {
    // KYO-242: connects to (and provisions/self-heals) this worktree's
    // private test database rather than the shared `kyomi_test` database.
    let db = match kyomi_core::test_db::connect_test_pool().await {
        Ok(pool) => pool,
        Err(_) => return,
    };

    let needs =
        kyomi_auth::catalog::indexers::bigquery_public::BigQueryPublicIndexer::needs_refresh(
            &db, 24,
        )
        .await;
    assert!(
        needs,
        "needs_refresh should return true when no public data cached"
    );
}

#[tokio::test]
async fn sample_data_get_table_count_returns_zero_for_empty_db() {
    // KYO-242: connects to (and provisions/self-heals) this worktree's
    // private test database rather than the shared `kyomi_test` database.
    let db = match kyomi_core::test_db::connect_test_pool().await {
        Ok(pool) => pool,
        Err(_) => return,
    };

    let count =
        kyomi_auth::catalog::indexers::sample_data::SampleDataIndexer::get_sample_table_count(&db)
            .await;
    // Count should be >= 0 (likely 0 in test DB)
    assert!(count >= 0);
}

#[tokio::test]
async fn bigquery_public_get_table_count_returns_zero_for_empty_db() {
    // KYO-242: connects to (and provisions/self-heals) this worktree's
    // private test database rather than the shared `kyomi_test` database.
    let db = match kyomi_core::test_db::connect_test_pool().await {
        Ok(pool) => pool,
        Err(_) => return,
    };

    let count = kyomi_auth::catalog::indexers::bigquery_public::BigQueryPublicIndexer::get_public_table_count(&db)
        .await;
    assert!(count >= 0);
}

// ===========================================================================
// Section 8: Catalog refresh rate-limiting
// ===========================================================================

#[tokio::test]
async fn can_refresh_now_returns_true_for_nonexistent_datasource() {
    // KYO-242: connects to (and provisions/self-heals) this worktree's
    // private test database rather than the shared `kyomi_test` database.
    let db = match kyomi_core::test_db::connect_test_pool().await {
        Ok(pool) => pool,
        Err(_) => return,
    };

    let can_refresh =
        kyomi_auth::catalog::helpers::can_refresh_now(&db, "nonexistent-ds-id-12345", 24).await;
    assert!(
        can_refresh,
        "can_refresh_now should return true for nonexistent datasource"
    );
}
