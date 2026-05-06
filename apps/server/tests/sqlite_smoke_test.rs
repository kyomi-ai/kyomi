// SPDX-License-Identifier: AGPL-3.0-or-later

//! SQLite CRUD smoke test — verifies that the in-memory SQLite backend can run
//! migrations and perform basic create / read / update / delete / scalar
//! operations through the `DbPool` dispatch macros.

use kyomi_core::db::DbPool;
use kyomi_core::{db_execute, db_fetch_all, db_fetch_one, db_fetch_optional, db_fetch_scalar};

// ---------------------------------------------------------------------------
// Row types for query_as
// ---------------------------------------------------------------------------

#[derive(Debug, sqlx::FromRow)]
struct UserRow {
    user_id: String,
    email: String,
    name: Option<String>,
    active: i32,
}

#[derive(Debug, sqlx::FromRow)]
struct WorkspaceRow {
    workspace_id: String,
    name: Option<String>,
    owner_user_id: String,
    subscription_tier: String,
}

#[derive(Debug, sqlx::FromRow)]
struct WorkspaceUserRow {
    workspace_id: String,
    user_id: String,
    role: String,
}

#[derive(Debug, sqlx::FromRow)]
struct DashboardRow {
    dashboard_id: String,
    user_id: String,
    workspace_id: String,
    title: String,
    content: String,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn sqlite_crud_smoke_test() {
    // 1. Connect to in-memory SQLite — migrations run automatically.
    let db = DbPool::connect("sqlite::memory:").await.expect("failed to connect to SQLite");
    assert!(db.is_sqlite(), "expected SQLite pool");

    let now = chrono::Utc::now().to_rfc3339();
    let user_id = uuid::Uuid::new_v4().to_string();
    let workspace_id = uuid::Uuid::new_v4().to_string();

    // 2. Create a user.
    db_execute!(
        db,
        "INSERT INTO users (user_id, email, name, created_at, updated_at, active, verified)
         VALUES ($1, $2, $3, $4, $5, 1, 0)",
        &user_id,
        "smoke@test.local",
        "Smoke Tester",
        &now,
        &now
    )
    .expect("insert user");

    // 3. Query back the user.
    let user: UserRow = db_fetch_one!(
        db,
        UserRow,
        "SELECT user_id, email, name, active FROM users WHERE user_id = $1",
        &user_id
    )
    .expect("fetch user");
    assert_eq!(user.user_id, user_id);
    assert_eq!(user.email, "smoke@test.local");
    assert_eq!(user.name.as_deref(), Some("Smoke Tester"));
    assert_eq!(user.active, 1);

    // 3b. Update the user name.
    db_execute!(
        db,
        "UPDATE users SET name = $1 WHERE user_id = $2",
        "Updated Name",
        &user_id
    )
    .expect("update user");

    let updated: UserRow = db_fetch_one!(
        db,
        UserRow,
        "SELECT user_id, email, name, active FROM users WHERE user_id = $1",
        &user_id
    )
    .expect("fetch updated user");
    assert_eq!(updated.name.as_deref(), Some("Updated Name"));

    // 4. Create a workspace.
    db_execute!(
        db,
        "INSERT INTO workspaces (workspace_id, name, owner_user_id, created_at, updated_at, subscription_tier, subscription_status)
         VALUES ($1, $2, $3, $4, $5, 'free', 'active')",
        &workspace_id,
        "Test Workspace",
        &user_id,
        &now,
        &now
    )
    .expect("insert workspace");

    // 5. Create a workspace_user.
    db_execute!(
        db,
        "INSERT INTO workspace_users (workspace_id, user_id, role, active, created_at)
         VALUES ($1, $2, 'admin', 1, $3)",
        &workspace_id,
        &user_id,
        &now
    )
    .expect("insert workspace_user");

    // 6. Query workspace.
    let ws: WorkspaceRow = db_fetch_one!(
        db,
        WorkspaceRow,
        "SELECT workspace_id, name, owner_user_id, subscription_tier FROM workspaces WHERE workspace_id = $1",
        &workspace_id
    )
    .expect("fetch workspace");
    assert_eq!(ws.workspace_id, workspace_id);
    assert_eq!(ws.name.as_deref(), Some("Test Workspace"));
    assert_eq!(ws.owner_user_id, user_id);
    assert_eq!(ws.subscription_tier, "free");

    // 7. Query workspace_user.
    let wu: WorkspaceUserRow = db_fetch_one!(
        db,
        WorkspaceUserRow,
        "SELECT workspace_id, user_id, role FROM workspace_users WHERE workspace_id = $1 AND user_id = $2",
        &workspace_id,
        &user_id
    )
    .expect("fetch workspace_user");
    assert_eq!(wu.workspace_id, workspace_id);
    assert_eq!(wu.user_id, user_id);
    assert_eq!(wu.role, "admin");

    // 8. Create a dashboard.
    let dashboard_id = uuid::Uuid::new_v4().to_string();
    db_execute!(
        db,
        "INSERT INTO dashboards (dashboard_id, user_id, workspace_id, title, content, created_at, updated_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7)",
        &dashboard_id,
        &user_id,
        &workspace_id,
        "Smoke Dashboard",
        "## Hello\nSome content",
        &now,
        &now
    )
    .expect("insert dashboard");

    // 9. Query dashboard back.
    let dash: DashboardRow = db_fetch_one!(
        db,
        DashboardRow,
        "SELECT dashboard_id, user_id, workspace_id, title, content FROM dashboards WHERE dashboard_id = $1",
        &dashboard_id
    )
    .expect("fetch dashboard");
    assert_eq!(dash.dashboard_id, dashboard_id);
    assert_eq!(dash.title, "Smoke Dashboard");
    assert_eq!(dash.content, "## Hello\nSome content");
    assert_eq!(dash.user_id, user_id);
    assert_eq!(dash.workspace_id, workspace_id);

    // 10. Delete the dashboard.
    let result = db_execute!(
        db,
        "DELETE FROM dashboards WHERE dashboard_id = $1",
        &dashboard_id
    )
    .expect("delete dashboard");
    assert_eq!(result.rows_affected(), 1);

    // 11. Verify deletion.
    let gone = db_fetch_optional!(
        db,
        DashboardRow,
        "SELECT dashboard_id, user_id, workspace_id, title, content FROM dashboards WHERE dashboard_id = $1",
        &dashboard_id
    )
    .expect("fetch deleted dashboard");
    assert!(gone.is_none(), "dashboard should have been deleted");

    // 12. Verify db_fetch_scalar works.
    let count: i64 = db_fetch_scalar!(
        db,
        i64,
        "SELECT COUNT(*) FROM users"
    )
    .expect("count users");
    assert_eq!(count, 1);

    // 13. Verify db_fetch_all works too.
    let all_users: Vec<UserRow> = db_fetch_all!(
        db,
        UserRow,
        "SELECT user_id, email, name, active FROM users"
    )
    .expect("fetch all users");
    assert_eq!(all_users.len(), 1);
    assert_eq!(all_users[0].email, "smoke@test.local");
}
