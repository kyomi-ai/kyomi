// SPDX-License-Identifier: AGPL-3.0-or-later

use super::*;
use crate::test_pg::{postgres_test_pool_or_skip, unique_test_id};

async fn assert_duplicate_names_and_id_scoping(db: &DbPool) {
    let workspace = unique_test_id("watch-names");
    let user_a = unique_test_id("watch-a");
    let user_b = unique_test_id("watch-b");
    for user in [&user_a, &user_b] {
        let email = format!("{user}@example.test");
        kyomi_core::db_execute!(
            db,
            "INSERT INTO users (user_id, email) VALUES ($1, $2)",
            user,
            &email
        )
        .expect("seed user");
    }
    kyomi_core::db_execute!(
        db,
        "INSERT INTO workspaces (workspace_id, name, owner_user_id) VALUES ($1, 'Watch names', $2)",
        &workspace,
        &user_a
    )
    .expect("seed workspace");
    for user in [&user_a, &user_b] {
        kyomi_core::db_execute!(db,
            "INSERT INTO workspace_users (workspace_id, user_id, role) VALUES ($1, $2, 'workspace_user')",
            &workspace, user
        ).expect("seed membership");
    }

    let mut watches = Vec::new();
    // Each user can reuse an identical name and a case variant, including a
    // name owned by the other user which their own search cannot reveal.
    for user in [&user_a, &user_b] {
        for name in ["Revenue Alert", "Revenue Alert", "revenue alert"] {
            let watch = create_watch(
                db,
                &workspace,
                user,
                name,
                "Check if revenue drops more than 10 percent",
                "0 9 * * *",
                "alert",
                None,
                None,
                None,
                false,
            )
            .await
            .expect("identical and case-variant names must be accepted");
            assert_eq!(watch.name, name);
            watches.push(watch);
        }
    }
    let ids: std::collections::HashSet<_> = watches.iter().map(|w| &w.watch_id).collect();
    assert_eq!(
        ids.len(),
        6,
        "duplicate labels must retain distinct watch identities"
    );

    update_watch(
        db,
        &watches[1].watch_id,
        &workspace,
        &user_a,
        &WatchUpdate {
            name: Some("A different label".into()),
            ..Default::default()
        },
    )
    .await
    .expect("give the renamed watch a distinct starting label");
    for name in ["Revenue Alert", "revenue alert"] {
        let updated = update_watch(
            db,
            &watches[1].watch_id,
            &workspace,
            &user_a,
            &WatchUpdate {
                name: Some(name.into()),
                ..Default::default()
            },
        )
        .await
        .expect("rename to an existing identical or case-variant name");
        assert_eq!(updated.name, name);
        assert_eq!(updated.watch_id, watches[1].watch_id);
    }
    assert!(
        get_watch(db, &watches[0].watch_id, &workspace, &user_b)
            .await
            .expect("get another user's watch")
            .is_none()
    );
    assert!(matches!(
        update_watch(
            db,
            &watches[0].watch_id,
            &workspace,
            &user_b,
            &WatchUpdate {
                name: Some("Changed label".into()),
                ..Default::default()
            }
        )
        .await,
        Err(kyomi_core::Error::NotFound(_))
    ));

    let expected: std::collections::HashSet<_> =
        watches[3..].iter().map(|w| w.watch_id.clone()).collect();
    let listed = list_watches(db, &workspace, &user_b).await.expect("list");
    assert_eq!(listed.len(), 3);
    assert_eq!(
        listed
            .into_iter()
            .map(|w| w.watch_id)
            .collect::<std::collections::HashSet<_>>(),
        expected
    );
    for query in [Some("revenue"), None] {
        let results = search_watches(db, &workspace, &user_b, query, 50)
            .await
            .expect("search");
        assert_eq!(results.len(), 3);
        assert_eq!(
            results
                .into_iter()
                .map(|w| w.watch_id)
                .collect::<std::collections::HashSet<_>>(),
            expected
        );
    }
    let synced = list_watches_for_sync(db, &workspace, &user_b)
        .await
        .expect("sync list");
    assert_eq!(synced.len(), 3);
    assert_eq!(
        count_watches_for_sync(db, &workspace, &user_b)
            .await
            .expect("sync count"),
        3
    );
    assert_eq!(
        synced
            .iter()
            .map(|w| w["watch_id"].as_str().expect("watch id").to_owned())
            .collect::<std::collections::HashSet<_>>(),
        expected
    );

    for watch in &watches {
        kyomi_core::db_execute!(db,
            "INSERT INTO watch_executions (watch_id, workspace_id, status, alert_triggered, started_at, completed_at, created_by) VALUES ($1, $2, 'success', $3, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP, $4)",
            &watch.watch_id, &workspace, true, &watch.created_by
        ).expect("seed triggered alert");
    }
    assert_eq!(
        get_unread_alerts_count(db, &workspace, &user_b)
            .await
            .expect("alert count"),
        3
    );
    let (alerts, total) = get_alerts_history(db, &workspace, None, 50, 0, false, &user_b)
        .await
        .expect("all own alerts");
    assert_eq!(total, 3);
    assert_eq!(alerts.len(), 3);
    assert_eq!(
        alerts
            .into_iter()
            .map(|a| a.watch_id.expect("watch id"))
            .collect::<std::collections::HashSet<_>>(),
        expected
    );
    for watch in [&watches[0], &watches[3], &watches[4]] {
        let (alerts, total) =
            get_alerts_history(db, &workspace, Some(&watch.watch_id), 50, 0, false, &user_b)
                .await
                .expect("filter alerts by watch id");
        let expected_count = i64::from(watch.created_by == user_b);
        assert_eq!(total, expected_count);
        assert_eq!(alerts.len() as i64, expected_count);
        for alert in alerts {
            assert_eq!(alert.watch_id.as_deref(), Some(watch.watch_id.as_str()));
        }
    }

    for table in ["watch_executions", "watches", "workspace_users", "sync_log"] {
        // Table names are fixed test-owned literals, never caller input.
        let sql = format!("DELETE FROM {table} WHERE workspace_id = $1");
        kyomi_core::db_execute!(db, &sql, &workspace).expect("cleanup watch fixture rows");
    }
    kyomi_core::db_execute!(
        db,
        "DELETE FROM workspaces WHERE workspace_id = $1",
        &workspace
    )
    .expect("cleanup workspace");
    for user in [&user_a, &user_b] {
        kyomi_core::db_execute!(db, "DELETE FROM users WHERE user_id = $1", user)
            .expect("cleanup user");
    }
}

#[tokio::test]
async fn sqlite_duplicate_names_keep_watch_id_scoping() {
    let db = crate::test_support::test_pool().await;
    assert_duplicate_names_and_id_scoping(&db).await;
}

#[tokio::test]
async fn postgres_duplicate_names_keep_watch_id_scoping() {
    let Some(db) =
        postgres_test_pool_or_skip("postgres_duplicate_names_keep_watch_id_scoping").await
    else {
        return;
    };
    assert_duplicate_names_and_id_scoping(&db).await;
}
