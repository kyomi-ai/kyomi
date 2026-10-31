// SPDX-License-Identifier: AGPL-3.0-or-later

use super::{
    MIGRATOR, seed_membership, seed_user, seed_user_with_active, seed_workspace,
    seed_workspace_with_name, sqlite_pool, test_pool,
};

#[tokio::test]
async fn migrated_pool_enforces_foreign_keys() {
    let db = test_pool().await;
    let sq = sqlite_pool(&db);
    assert_eq!(sq.options().get_max_connections(), 1);
    let enabled: i64 = sqlx::query_scalar("PRAGMA foreign_keys")
        .fetch_one(sq)
        .await
        .expect("read foreign key setting");
    assert_eq!(enabled, 1);

    let migrations: Vec<(i64, bool)> =
        sqlx::query_as("SELECT version, success FROM _sqlx_migrations ORDER BY version")
            .fetch_all(sq)
            .await
            .expect("read migration history");
    let expected: Vec<(i64, bool)> = MIGRATOR
        .iter()
        .map(|migration| (migration.version, true))
        .collect();
    assert_eq!(migrations, expected);

    let error = sqlx::query(
        "INSERT INTO workspaces (workspace_id, name, owner_user_id) VALUES ('orphan', 'Orphan', 'missing')",
    ).execute(sq).await.expect_err("missing owner must violate foreign key");
    assert!(
        error
            .as_database_error()
            .expect("database error")
            .is_foreign_key_violation()
    );

    seed_user(sq, "owner", "owner@test.local").await;
    seed_workspace(sq, "ws", "owner").await;
    let error = sqlx::query(
        "INSERT INTO workspace_users (workspace_id, user_id, role, active) VALUES ('ws', 'missing', 'user', 1)",
    ).execute(sq).await.expect_err("missing member must violate foreign key");
    assert!(
        error
            .as_database_error()
            .expect("database error")
            .is_foreign_key_violation()
    );
    let error = sqlx::query(
        "INSERT INTO workspace_users (workspace_id, user_id, role, active) VALUES ('missing', 'owner', 'user', 1)",
    ).execute(sq).await.expect_err("missing workspace must violate foreign key");
    assert!(
        error
            .as_database_error()
            .expect("database error")
            .is_foreign_key_violation()
    );
}

#[tokio::test]
async fn generic_seeds_preserve_identity_active_flags_names_and_roles() {
    let db = test_pool().await;
    let sq = sqlite_pool(&db);
    seed_user(sq, "owner", "owner@test.local").await;
    seed_user_with_active(sq, "inactive", "inactive@test.local", false).await;
    seed_user_with_active(sq, "active", "active@test.local", true).await;
    seed_workspace(sq, "ws", "owner").await;
    seed_workspace_with_name(sq, "named", "owner", "Workspace").await;
    seed_membership(sq, "ws", "owner", "workspace_admin", true).await;
    seed_membership(sq, "ws", "inactive", "user", false).await;

    let users: Vec<(String, String, bool)> =
        sqlx::query_as("SELECT user_id, email, active FROM users ORDER BY user_id")
            .fetch_all(sq)
            .await
            .expect("read users");
    assert_eq!(
        users,
        vec![
            ("active".into(), "active@test.local".into(), true),
            ("inactive".into(), "inactive@test.local".into(), false),
            ("owner".into(), "owner@test.local".into(), true),
        ]
    );
    let workspaces: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT workspace_id, name, owner_user_id FROM workspaces ORDER BY workspace_id",
    )
    .fetch_all(sq)
    .await
    .expect("read workspaces");
    assert_eq!(
        workspaces,
        vec![
            ("named".into(), "Workspace".into(), "owner".into()),
            ("ws".into(), "Workspace ws".into(), "owner".into()),
        ]
    );
    let memberships: Vec<(String, String, String, bool)> = sqlx::query_as(
        "SELECT workspace_id, user_id, role, active FROM workspace_users ORDER BY user_id",
    )
    .fetch_all(sq)
    .await
    .expect("read memberships");
    assert_eq!(
        memberships,
        vec![
            ("ws".into(), "inactive".into(), "user".into(), false),
            ("ws".into(), "owner".into(), "workspace_admin".into(), true),
        ]
    );

    let other_db = test_pool().await;
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users")
        .fetch_one(sqlite_pool(&other_db))
        .await
        .expect("read isolated users");
    assert_eq!(count, 0, "each test pool must own an isolated database");
}
