// SPDX-License-Identifier: AGPL-3.0-or-later

use super::*;
use chrono::{Datelike, Timelike};

fn instant(value: &str) -> DateTime<Utc> {
    value.parse().unwrap()
}

fn next(schedule: &str, zone: &str, after: &str) -> DateTime<Utc> {
    calculate_next_run_in_timezone_after(schedule, Some(zone), instant(after)).unwrap()
}

#[test]
fn sydney_first_run_uses_offset_at_execution_with_dated_weekday_rollover() {
    let execution = next("0 9 * * 1", "Australia/Sydney", "2026-10-03T00:00:00Z");
    assert_eq!(execution, instant("2026-10-04T22:00:00Z"));
    assert_eq!(
        describe_execution(execution, Some("Australia/Sydney")).unwrap(),
        "2026-10-05 09:00 +11:00 Australia/Sydney / 2026-10-04 22:00 UTC"
    );
    assert_eq!(
        describe_cron_in_timezone("0 9 * * 1", Some("Australia/Sydney")),
        "Weekly on Monday at 9:00 AM Australia/Sydney"
    );
}

#[test]
fn weekly_wall_time_survives_both_sydney_seasonal_transitions() {
    for (start, expected) in [
        (
            "2026-09-26T00:00:00Z",
            [
                "2026-09-27T23:00:00Z",
                "2026-10-04T22:00:00Z",
                "2026-10-11T22:00:00Z",
            ],
        ),
        (
            "2027-03-27T00:00:00Z",
            [
                "2027-03-28T22:00:00Z",
                "2027-04-04T23:00:00Z",
                "2027-04-11T23:00:00Z",
            ],
        ),
    ] {
        let mut reference = instant(start);
        for expected in expected {
            reference = calculate_next_run_in_timezone_after(
                "0 9 * * 1",
                Some("Australia/Sydney"),
                reference,
            )
            .unwrap();
            assert_eq!(reference, instant(expected));
            let local = reference.with_timezone(&chrono_tz::Australia::Sydney);
            assert_eq!(local.weekday(), chrono::Weekday::Mon);
            assert_eq!((local.hour(), local.minute()), (9, 0));
        }
    }
}

#[test]
fn nonexistent_wall_time_is_skipped_instead_of_shifted() {
    assert_eq!(
        next("30 2 * * 0", "Australia/Sydney", "2026-10-03T00:00:00Z"),
        instant("2026-10-10T15:30:00Z")
    );
    assert_eq!(
        next("30 2 * * *", "America/New_York", "2026-03-08T00:00:00Z"),
        instant("2026-03-09T06:30:00Z")
    );
}

#[test]
fn repeated_wall_time_fires_only_at_first_occurrence_even_after_first_run() {
    let first = next("30 2 * * 0", "Australia/Sydney", "2026-04-04T00:00:00Z");
    assert_eq!(first, instant("2026-04-04T15:30:00Z"));
    for reference in [
        "2026-04-04T15:30:00Z",
        "2026-04-04T15:45:00Z",
        "2026-04-04T16:00:00Z",
        "2026-04-04T16:30:00Z",
    ] {
        assert_eq!(
            next("30 2 * * 0", "Australia/Sydney", reference),
            instant("2026-04-11T16:30:00Z")
        );
    }
    assert_eq!(
        next("30 1 * * *", "America/New_York", "2026-11-01T00:00:00Z"),
        instant("2026-11-01T05:30:00Z")
    );
    assert_eq!(
        next("30 1 * * *", "America/New_York", "2026-11-01T06:00:00Z"),
        instant("2026-11-02T06:30:00Z")
    );
}

#[test]
fn omitted_and_explicit_utc_keep_legacy_cron_and_invalid_zones_fail() {
    let after = instant("2026-10-03T00:00:00Z");
    for timezone in [None, Some("UTC")] {
        assert_eq!(
            calculate_next_run_in_timezone_after("0 23 * * 0", timezone, after).unwrap(),
            instant("2026-10-04T23:00:00Z")
        );
    }
    for timezone in ["+11:00", "11", "Sydney", "Australia/Unknown", ""] {
        assert!(calculate_next_run_in_timezone_after("0 9 * * 1", Some(timezone), after).is_err());
    }
}

async fn assert_persistence_lifecycle(db: &DbPool, workspace: &str, user: &str) {
    let watch = create_watch(
        db,
        workspace,
        user,
        "Sydney Monday",
        "Report weekly revenue trends",
        "0 9 * * 1",
        Some("Australia/Sydney"),
        "report",
        None,
        None,
        None,
        false,
    )
    .await
    .unwrap();
    assert_eq!(watch.schedule, "0 9 * * 1");
    assert_eq!(watch.timezone.as_deref(), Some("Australia/Sydney"));
    let saved = get_watch(db, &watch.watch_id, workspace, user)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(saved.timezone, watch.timezone);
    assert_eq!(
        list_watches(db, workspace, user).await.unwrap()[0].timezone,
        watch.timezone
    );
    let edited = update_watch(
        db,
        &watch.watch_id,
        workspace,
        user,
        &WatchUpdate {
            name: Some("Still Sydney".into()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(edited.timezone, watch.timezone);
    assert_eq!(edited.next_run_at, watch.next_run_at);
    let paused = toggle_watch(db, &watch.watch_id, workspace, user, false)
        .await
        .unwrap();
    assert_eq!(paused.next_run_at, None);
    let edited = update_watch(
        db,
        &watch.watch_id,
        workspace,
        user,
        &WatchUpdate {
            schedule: Some("30 9 * * 1".into()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(edited.timezone, watch.timezone);
    assert_eq!(edited.next_run_at, None);
    let enabled = toggle_watch(db, &watch.watch_id, workspace, user, true)
        .await
        .unwrap();
    assert_eq!(enabled.timezone, watch.timezone);
    let local = enabled
        .next_run_at
        .unwrap()
        .with_timezone(&chrono_tz::Australia::Sydney);
    assert_eq!(
        (local.hour(), local.minute(), local.weekday()),
        (9, 30, chrono::Weekday::Mon)
    );
    let utc = update_watch(
        db,
        &watch.watch_id,
        workspace,
        user,
        &WatchUpdate {
            timezone: Some("UTC".into()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(utc.timezone.as_deref(), Some("UTC"));
    assert_eq!(
        (
            utc.next_run_at.unwrap().hour(),
            utc.next_run_at.unwrap().minute()
        ),
        (9, 30)
    );
    assert_ne!(utc.next_run_at, enabled.next_run_at);
    assert!(
        update_watch(
            db,
            &watch.watch_id,
            workspace,
            user,
            &WatchUpdate {
                timezone: Some("+11:00".into()),
                ..Default::default()
            }
        )
        .await
        .is_err()
    );
    assert_eq!(
        get_watch(db, &watch.watch_id, workspace, user)
            .await
            .unwrap()
            .unwrap()
            .timezone
            .as_deref(),
        Some("UTC")
    );
    assert!(
        create_watch(
            db,
            workspace,
            user,
            "Invalid zone",
            "Report weekly revenue trends",
            "0 9 * * 1",
            Some("Australia/Unknown"),
            "report",
            None,
            None,
            None,
            false
        )
        .await
        .is_err()
    );
    assert_eq!(list_watches(db, workspace, user).await.unwrap().len(), 1);
    let legacy = create_watch(
        db,
        workspace,
        user,
        "Legacy UTC",
        "Report weekly revenue trends",
        "0 23 * * 0",
        None,
        "report",
        None,
        None,
        None,
        false,
    )
    .await
    .unwrap();
    assert_eq!(legacy.timezone, None);
    assert_eq!(legacy.schedule, "0 23 * * 0");
    assert_eq!(legacy.next_run_at.unwrap().hour(), 23);
    assert_eq!(legacy.next_run_at.unwrap().weekday(), chrono::Weekday::Sun);
}

#[tokio::test]
async fn timezone_persistence_lifecycle_sqlite() {
    let db = crate::test_support::test_pool().await;
    let sq = crate::test_support::sqlite_pool(&db);
    crate::test_support::seed_user(sq, "timezone-user", "timezone@test.local").await;
    crate::test_support::seed_workspace(sq, "timezone-ws", "timezone-user").await;
    assert_persistence_lifecycle(&db, "timezone-ws", "timezone-user").await;
}

#[tokio::test]
async fn timezone_persistence_lifecycle_postgres() {
    let Some(db) =
        crate::test_pg::postgres_test_pool_or_skip("timezone_persistence_lifecycle_postgres").await
    else {
        return;
    };
    let user = crate::test_pg::unique_test_id("tz-user");
    let ws = crate::test_pg::unique_test_id("tz-ws");
    let pg = crate::test_pg::postgres_pool(&db);
    crate::test_pg::seed_user_pg(pg, &user, &format!("{user}@test.local")).await;
    crate::test_pg::seed_workspace_pg(pg, &ws, &user).await;
    assert_persistence_lifecycle(&db, &ws, &user).await;
    kyomi_core::db_execute!(&db, "DELETE FROM watches WHERE workspace_id = $1", &ws).unwrap();
    kyomi_core::db_execute!(&db, "DELETE FROM workspaces WHERE workspace_id = $1", &ws).unwrap();
    kyomi_core::db_execute!(&db, "DELETE FROM users WHERE user_id = $1", &user).unwrap();
}

async fn assert_additive_migration_preserves_legacy(db: &DbPool) {
    kyomi_core::db_execute!(
        db,
        "INSERT INTO watches (watch_id, schedule) VALUES ('legacy', '0 23 * * 0')"
    )
    .unwrap();
    let migration = if db.is_postgres() {
        include_str!("../../../apps/server/migrations/20261005000000_watch_schedule_timezone.sql")
    } else {
        include_str!("../../../apps/server/migrations-sqlite/00045_watch_schedule_timezone.sql")
    };
    kyomi_core::db_execute!(db, migration).unwrap();
    #[derive(sqlx::FromRow)]
    struct Row {
        schedule: String,
        timezone: Option<String>,
    }
    let legacy = kyomi_core::db_fetch_one!(
        db,
        Row,
        "SELECT schedule, timezone FROM watches WHERE watch_id = 'legacy'"
    )
    .unwrap();
    assert_eq!(legacy.schedule, "0 23 * * 0");
    assert_eq!(legacy.timezone, None);
    assert_eq!(
        calculate_next_run_in_timezone_after(
            &legacy.schedule,
            legacy.timezone.as_deref(),
            instant("2026-10-03T00:00:00Z")
        )
        .unwrap(),
        instant("2026-10-04T23:00:00Z")
    );
}

#[tokio::test]
async fn timezone_additive_migration_preserves_legacy_sqlite() {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    sqlx::query("CREATE TABLE watches (watch_id TEXT PRIMARY KEY, schedule TEXT NOT NULL)")
        .execute(&pool)
        .await
        .unwrap();
    assert_additive_migration_preserves_legacy(&DbPool::Sqlite(pool)).await;
}

#[tokio::test]
async fn timezone_additive_migration_preserves_legacy_postgres() {
    let Some(db) = crate::test_pg::postgres_test_pool_or_skip(
        "timezone_additive_migration_preserves_legacy_postgres",
    )
    .await
    else {
        return;
    };
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect_with((*db.pg_pool().connect_options()).clone())
        .await
        .unwrap();
    sqlx::query(
        "CREATE TEMPORARY TABLE watches (watch_id TEXT PRIMARY KEY, schedule TEXT NOT NULL)",
    )
    .execute(&pool)
    .await
    .unwrap();
    assert_additive_migration_preserves_legacy(&DbPool::Postgres(pool)).await;
}
