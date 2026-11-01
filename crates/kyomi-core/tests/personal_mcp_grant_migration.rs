// SPDX-License-Identifier: AGPL-3.0-or-later
//! Existing grants predate destination consent and may contain plaintext names.
use sqlx::{migrate::Migrator, sqlite::SqlitePoolOptions};
use std::{borrow::Cow, path::Path};

async fn migrations_through(version: i64) -> Migrator {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../apps/server/migrations-sqlite"
    );
    let full = Migrator::new(Path::new(path)).await.unwrap();
    let migrations = full
        .migrations
        .iter()
        .filter(|m| m.version <= version)
        .cloned()
        .collect();
    Migrator {
        migrations: Cow::Owned(migrations),
        ..full
    }
}

#[tokio::test]
async fn legacy_grants_are_removed_by_catalog_migration() {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    migrations_through(51).await.run(&pool).await.unwrap();
    for sql in [
        "INSERT INTO users (user_id,email) VALUES ('u','u@example.com')",
        "INSERT INTO workspaces (workspace_id,name,owner_user_id) VALUES ('w','w','u')",
        "INSERT INTO workspace_users (workspace_id,user_id,role,active) VALUES ('w','u','workspace_admin',TRUE)",
        "INSERT INTO user_mcp_connections (connection_id,owner_user_id,name,endpoint,auth_kind,status) VALUES ('c','u','c','https://example.com','none','ready')",
        "INSERT INTO user_mcp_workspace_enablements (connection_id,owner_user_id,workspace_id) VALUES ('c','u','w')",
        "INSERT INTO user_mcp_tool_grants (connection_id,workspace_id,tool_name,revision,schema_hash,generation) VALUES ('c','w','private-name','v','hash',1)",
    ] {
        sqlx::query(sql).execute(&pool).await.unwrap();
    }
    let before: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM user_mcp_tool_grants")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(before, 1);
    migrations_through(52).await.run(&pool).await.unwrap();
    let after: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM user_mcp_tool_grants")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(after, 0, "legacy grants must be removed before catalog use");
}
