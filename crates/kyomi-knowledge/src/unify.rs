// SPDX-License-Identifier: AGPL-3.0-or-later

//! Post-migration helper: convert knowledge-file folders to collections.
//!
//! This runs after the SQL migration that adds `doc_type` to dashboards and
//! copies knowledge files into the dashboards table. It reads the folder tree
//! from the still-existing `knowledge_files` table, creates a collection for
//! each folder path, assigns migrated files to their corresponding collection,
//! and finally drops the `knowledge_files` table.
//!
//! Idempotent: if `knowledge_files` no longer exists, this is a no-op.

use std::collections::HashMap;

use kyomi_core::db::DbPool;
use kyomi_core::sql_compat;
use uuid::Uuid;

/// Resolve the full path for a folder by walking its parent chain.
///
/// Produces names like `"Revenue"`, `"Revenue / Q1"`, `"Revenue / Q1 / January"`.
/// Guards against cycles with a max depth of 50.
fn resolve_path(folder_id: &str, folder_map: &HashMap<String, (&str, Option<&str>)>) -> String {
    let mut parts = Vec::new();
    let mut current_id = Some(folder_id);
    let mut depth = 0;
    while let Some(id) = current_id {
        if depth > 50 {
            tracing::warn!(folder_id, "Folder chain exceeds 50 levels — truncating");
            break;
        }
        if let Some(&(name, parent)) = folder_map.get(id) {
            parts.push(name);
            current_id = parent;
        } else {
            break;
        }
        depth += 1;
    }
    parts.reverse();
    parts.join(" / ")
}

/// Run the folder-to-collection migration and drop the old `knowledge_files` table.
///
/// Must be called after `sqlx::migrate!` has applied the SQL migration that
/// adds columns to dashboards and copies file rows. This function:
///
/// 1. Checks if `knowledge_files` still exists (idempotent guard).
/// 2. Reads all folders, builds path names by resolving parent chains.
/// 3. Creates a collection for each unique (workspace_id, folder_path),
///    skipping any that already exist (ON CONFLICT / INSERT OR IGNORE).
/// 4. Links files that had a `parent_id` to their folder's collection.
/// 5. Drops the `knowledge_files` table.
pub async fn migrate_folders_to_collections(db: &DbPool) -> kyomi_core::Result<()> {
    // Idempotent guard: check if knowledge_files table still exists.
    let table_exists = check_knowledge_files_exists(db).await?;
    if !table_exists {
        tracing::debug!("knowledge_files table does not exist — folder migration already complete");
        return Ok(());
    }

    let is_pg = db.is_postgres();

    // Step 1: Read all folders.
    #[derive(sqlx::FromRow, Debug)]
    struct FolderRow {
        id: String,
        workspace_id: String,
        parent_id: Option<String>,
        name: String,
        created_by: Option<String>,
    }

    let ct = |col: &str| sql_compat::cast_to_text(is_pg, col);
    let folder_sql = format!(
        "SELECT {id} AS id, {workspace_id} AS workspace_id, {parent_id} AS parent_id, name, created_by \
         FROM knowledge_files WHERE is_folder = {bool_true} \
         ORDER BY parent_id NULLS FIRST",
        id = ct("id"),
        workspace_id = ct("workspace_id"),
        parent_id = ct("parent_id"),
        bool_true = sql_compat::bool_true(is_pg),
    );
    let folders: Vec<FolderRow> = kyomi_core::db_fetch_all!(db, FolderRow, &folder_sql)?;

    if folders.is_empty() {
        tracing::info!("No knowledge-file folders to migrate — dropping knowledge_files table");
        drop_knowledge_files(db).await?;
        return Ok(());
    }

    // Step 2: Build folder path names by resolving parent chains.
    // Map folder_id -> (name, parent_id) for the resolve_path function
    let folder_map: HashMap<String, (&str, Option<&str>)> = folders
        .iter()
        .map(|f| {
            (
                f.id.clone(),
                (f.name.as_str(), f.parent_id.as_deref()),
            )
        })
        .collect();

    // Build (folder_id -> resolved_path) and collect unique (workspace_id, path) pairs
    let mut folder_paths: HashMap<String, String> = HashMap::new();
    // (workspace_id, path) -> collection_id
    let mut collection_ids: HashMap<(String, String), String> = HashMap::new();
    // (workspace_id, path) -> the created_by of whichever folder first
    // established this collection key, mirroring collection_ids' dedup so
    // both maps agree on which folder "owns" a given (workspace_id, path).
    let mut collection_created_by: HashMap<(String, String), Option<String>> = HashMap::new();

    for folder in &folders {
        let path = resolve_path(&folder.id, &folder_map);
        folder_paths.insert(folder.id.clone(), path.clone());
        let key = (folder.workspace_id.clone(), path);
        collection_ids.entry(key.clone()).or_insert_with(|| Uuid::new_v4().to_string());
        collection_created_by.entry(key).or_insert_with(|| folder.created_by.clone());
    }

    // Step 3: Create collections for each unique (workspace_id, folder_path).
    // Use ON CONFLICT / INSERT OR IGNORE to handle pre-existing collections
    // with the same name (e.g., a user had a "Revenue" collection AND folder).
    let now_expr = sql_compat::now(is_pg);
    let uuid_cast = |param: &str| sql_compat::cast_to_uuid(is_pg, param);

    // collections.created_by is NOT NULL with an FK to users(user_id) on
    // both backends (Postgres: 20260609000000_add_created_by_to_collections.sql;
    // SQLite: 00033_fix_collections_created_by_constraints.sql). A folder's
    // own created_by is nullable and carries no FK (see
    // apps/server/migrations-sqlite/00013_knowledge_files.sql), so it may be
    // NULL or may name a user that no longer exists — either would violate
    // the FK below. Resolve with the same three-step COALESCE fallback those
    // two migrations use for their own backfill, evaluated in SQL so the
    // insert stays one statement:
    //   1. the folder's own created_by ($4), but only if it still names a
    //      row in users — the inner SELECT returns no rows otherwise, so
    //      COALESCE falls through;
    //   2. else the earliest workspace_users member for this workspace
    //      ($2) by created_at;
    //   3. else the earliest user overall.
    // If all three yield nothing the FK rejects the row and this fails
    // loudly — which requires a database with zero users, and a database
    // with zero users cannot have had folders created in it.
    let created_by_expr = "COALESCE(\
        (SELECT u.user_id FROM users u WHERE u.user_id = $4), \
        (SELECT wu.user_id FROM workspace_users wu WHERE wu.workspace_id = $2 \
         ORDER BY wu.created_at ASC LIMIT 1), \
        (SELECT user_id FROM users ORDER BY created_at ASC LIMIT 1)\
    )";

    let insert_collection_sql = if is_pg {
        format!(
            "INSERT INTO collections (id, workspace_id, name, description, color, is_public, created_at, updated_at, created_by) \
             VALUES ({id}, $2, $3, NULL, NULL, {bool_false}, {now_expr}, {now_expr}, {created_by_expr}) \
             ON CONFLICT (workspace_id, name) DO NOTHING",
            id = uuid_cast("$1"),
            bool_false = sql_compat::bool_false(is_pg),
        )
    } else {
        format!(
            "INSERT OR IGNORE INTO collections (id, workspace_id, name, description, color, is_public, created_at, updated_at, created_by) \
             VALUES ({id}, $2, $3, NULL, NULL, {bool_false}, {now_expr}, {now_expr}, {created_by_expr})",
            id = uuid_cast("$1"),
            bool_false = sql_compat::bool_false(is_pg),
        )
    };

    for ((workspace_id, path), collection_id) in &collection_ids {
        let created_by =
            collection_created_by.get(&(workspace_id.clone(), path.clone())).cloned().flatten();
        kyomi_core::db_execute!(
            db,
            &insert_collection_sql,
            collection_id,
            workspace_id,
            path,
            created_by
        )?;
    }

    // Re-query actual collection IDs in case ON CONFLICT skipped any inserts
    // (a pre-existing collection has a different ID than the one we generated).
    let select_collection_sql = format!(
        "SELECT {id} AS id FROM collections WHERE workspace_id = $1 AND name = $2",
        id = sql_compat::cast_to_text(is_pg, "id"),
    );

    for ((workspace_id, path), collection_id) in collection_ids.iter_mut() {
        let actual_id: String = kyomi_core::db_fetch_scalar!(
            db,
            String,
            &select_collection_sql,
            workspace_id,
            path
        )?;
        *collection_id = actual_id;
    }

    tracing::info!(
        count = collection_ids.len(),
        "Created/resolved collections from knowledge-file folders"
    );

    // Step 4: Link files that had a parent_id to their folder's collection.
    // Files have been migrated to dashboards with dashboard_id = old knowledge_file.id.
    #[derive(sqlx::FromRow, Debug)]
    struct FileParentRow {
        id: String,
        workspace_id: String,
        parent_id: String,
    }

    let file_sql = format!(
        "SELECT {id} AS id, {workspace_id} AS workspace_id, {parent_id} AS parent_id \
         FROM knowledge_files WHERE is_folder = {bool_false} AND parent_id IS NOT NULL",
        id = ct("id"),
        workspace_id = ct("workspace_id"),
        parent_id = ct("parent_id"),
        bool_false = sql_compat::bool_false(is_pg),
    );
    let files_with_parents: Vec<FileParentRow> =
        kyomi_core::db_fetch_all!(db, FileParentRow, &file_sql)?;

    let insert_cd_sql = format!(
        "INSERT INTO collection_dashboards (collection_id, dashboard_id, position, added_at) \
         VALUES ({col_id}, $2, $3, {now_expr})",
        col_id = uuid_cast("$1"),
    );

    // Track position per collection so each starts at 0
    let mut position_counters: HashMap<String, i32> = HashMap::new();
    let mut linked = 0u64;

    for file in &files_with_parents {
        if let Some(path) = folder_paths.get(&file.parent_id) {
            let key = (file.workspace_id.clone(), path.clone());
            if let Some(collection_id) = collection_ids.get(&key) {
                let position = position_counters
                    .entry(collection_id.clone())
                    .or_insert(0);
                let pos = *position;
                kyomi_core::db_execute!(
                    db,
                    &insert_cd_sql,
                    collection_id,
                    &file.id,
                    &pos
                )?;
                *position += 1;
                linked += 1;
            }
        }
    }

    tracing::info!(linked, "Linked knowledge files to collections");

    // Step 5: Drop the old knowledge_files table.
    drop_knowledge_files(db).await?;

    Ok(())
}

/// Check whether the `knowledge_files` table exists in the database.
async fn check_knowledge_files_exists(db: &DbPool) -> kyomi_core::Result<bool> {
    let sql = match db {
        DbPool::Postgres(_) => {
            "SELECT COUNT(*) FROM information_schema.tables \
             WHERE table_schema = 'public' AND table_name = 'knowledge_files'"
        }
        DbPool::Sqlite(_) => {
            "SELECT COUNT(*) FROM sqlite_master \
             WHERE type = 'table' AND name = 'knowledge_files'"
        }
    };
    let count: i64 = kyomi_core::db_fetch_scalar!(db, i64, sql)?;
    Ok(count > 0)
}

/// Drop the `knowledge_files` table and its indexes.
async fn drop_knowledge_files(db: &DbPool) -> kyomi_core::Result<()> {
    kyomi_core::db_execute!(db, "DROP TABLE IF EXISTS knowledge_files")?;
    tracing::info!("Dropped knowledge_files table");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_path_building() {
        // Verify path resolution logic with a mock folder structure
        // "Root" (id=1, parent=None)
        //   "Child" (id=2, parent=1)
        //     "Grandchild" (id=3, parent=2)

        let folder_map: HashMap<String, (&str, Option<&str>)> = HashMap::from([
            ("1".into(), ("Root", None)),
            ("2".into(), ("Child", Some("1"))),
            ("3".into(), ("Grandchild", Some("2"))),
        ]);

        assert_eq!(resolve_path("1", &folder_map), "Root");
        assert_eq!(resolve_path("2", &folder_map), "Root / Child");
        assert_eq!(resolve_path("3", &folder_map), "Root / Child / Grandchild");
    }

    // ─── migrate_folders_to_collections created_by regression (KYO-293) ─────
    //
    // The collections INSERT this function builds used to omit created_by
    // entirely. Against a post-00033 schema, collections.created_by is
    // `NOT NULL REFERENCES users(user_id)` on both backends — SQLite's
    // `INSERT OR IGNORE` silently drops the row on that constraint
    // violation (0 rows, no error), and the very next statement
    // (`SELECT id FROM collections WHERE workspace_id = $1 AND name = $2`)
    // then fails with `RowNotFound`, which propagates out of this function.
    // `DbPool::connect("sqlite::memory:")` runs the *real* embedded
    // migration chain (including 00033), so these tests exercise the exact
    // schema `apps/server/src/main.rs`'s boot-time call runs against.

    /// Extract the SQLite pool for direct fixture seeding — every test here
    /// connects via `DbPool::connect("sqlite::memory:")`, so this can never
    /// observe the Postgres arm.
    fn sqlite_pool(db: &DbPool) -> &sqlx::SqlitePool {
        match db {
            DbPool::Sqlite(pool) => pool,
            DbPool::Postgres(_) => unreachable!("sqlite::memory: URL must select the SQLite backend"),
        }
    }

    async fn seed_user(pool: &sqlx::SqlitePool, user_id: &str, created_at: &str) {
        sqlx::query("INSERT INTO users (user_id, email, created_at) VALUES ($1, $2, $3)")
            .bind(user_id)
            .bind(format!("{user_id}@example.com"))
            .bind(created_at)
            .execute(pool)
            .await
            .expect("seed user");
    }

    async fn seed_workspace(pool: &sqlx::SqlitePool, workspace_id: &str, owner_user_id: &str) {
        sqlx::query("INSERT INTO workspaces (workspace_id, name, owner_user_id) VALUES ($1, $2, $3)")
            .bind(workspace_id)
            .bind(format!("Workspace {workspace_id}"))
            .bind(owner_user_id)
            .execute(pool)
            .await
            .expect("seed workspace");
    }

    async fn seed_workspace_member(
        pool: &sqlx::SqlitePool,
        workspace_id: &str,
        user_id: &str,
        created_at: &str,
    ) {
        sqlx::query(
            "INSERT INTO workspace_users (workspace_id, user_id, role, created_at) \
             VALUES ($1, $2, 'member', $3)",
        )
        .bind(workspace_id)
        .bind(user_id)
        .bind(created_at)
        .execute(pool)
        .await
        .expect("seed workspace_users");
    }

    /// `created_by` is `Option<&str>` so callers can seed the NULL-owner
    /// fallback case directly.
    async fn seed_folder(
        pool: &sqlx::SqlitePool,
        id: &str,
        workspace_id: &str,
        name: &str,
        created_by: Option<&str>,
    ) {
        sqlx::query(
            "INSERT INTO knowledge_files (id, workspace_id, parent_id, name, is_folder, created_by) \
             VALUES ($1, $2, NULL, $3, 1, $4)",
        )
        .bind(id)
        .bind(workspace_id)
        .bind(name)
        .bind(created_by)
        .execute(pool)
        .await
        .expect("seed folder");
    }

    async fn collection_created_by_column(
        pool: &sqlx::SqlitePool,
        workspace_id: &str,
        name: &str,
    ) -> String {
        sqlx::query_scalar(
            "SELECT created_by FROM collections WHERE workspace_id = $1 AND name = $2",
        )
        .bind(workspace_id)
        .bind(name)
        .fetch_one(pool)
        .await
        .expect("fetch migrated collection's created_by")
    }

    /// The primary regression assertion: against a post-00033 schema, with
    /// a folder whose own `created_by` names a real user, the migration
    /// must succeed and the resulting collection's `created_by` must be
    /// that real user — not `RowNotFound`, and not an omitted column.
    #[tokio::test]
    async fn migrate_folders_to_collections_backfills_created_by_from_folder_owner() {
        let db = kyomi_core::db::DbPool::connect("sqlite::memory:")
            .await
            .expect("connect in-memory sqlite and run the full migration chain");
        let pool = sqlite_pool(&db);

        seed_user(pool, "user-owner", "2022-01-01T00:00:00Z").await;
        seed_workspace(pool, "ws-owner", "user-owner").await;
        seed_folder(pool, "folder-owner", "ws-owner", "Revenue", Some("user-owner")).await;

        let result = migrate_folders_to_collections(&db).await;
        assert!(result.is_ok(), "migration must succeed against a post-00033 schema: {result:?}");

        let created_by = collection_created_by_column(pool, "ws-owner", "Revenue").await;
        assert_eq!(
            created_by, "user-owner",
            "collection's created_by must be the folder's own owner when that owner still exists"
        );
    }

    /// Fallback step 2: a folder with no `created_by` (NULL, as
    /// `knowledge_files.created_by` allows) must fall back to the earliest
    /// `workspace_users` member for that workspace.
    #[tokio::test]
    async fn migrate_folders_to_collections_falls_back_when_folder_owner_is_null() {
        let db = kyomi_core::db::DbPool::connect("sqlite::memory:")
            .await
            .expect("connect in-memory sqlite and run the full migration chain");
        let pool = sqlite_pool(&db);

        seed_user(pool, "user-early", "2020-01-01T00:00:00Z").await;
        seed_user(pool, "user-late", "2023-01-01T00:00:00Z").await;
        seed_workspace(pool, "ws-null-owner", "user-early").await;
        seed_workspace_member(pool, "ws-null-owner", "user-early", "2020-01-01T00:00:00Z").await;
        seed_workspace_member(pool, "ws-null-owner", "user-late", "2023-01-01T00:00:00Z").await;
        seed_folder(pool, "folder-null-owner", "ws-null-owner", "Untitled", None).await;

        let result = migrate_folders_to_collections(&db).await;
        assert!(result.is_ok(), "migration must succeed when the folder has no owner: {result:?}");

        let created_by = collection_created_by_column(pool, "ws-null-owner", "Untitled").await;
        assert_eq!(
            created_by, "user-early",
            "a NULL folder owner must fall back to the earliest workspace_users member, \
             not the later one"
        );
    }

    /// Fallback step 2, other trigger: a folder whose `created_by` names a
    /// user that no longer exists (`knowledge_files.created_by` carries no
    /// FK) must not be honored — it must fall back exactly like the NULL
    /// case, not violate the new FK.
    #[tokio::test]
    async fn migrate_folders_to_collections_falls_back_when_folder_owner_does_not_exist() {
        let db = kyomi_core::db::DbPool::connect("sqlite::memory:")
            .await
            .expect("connect in-memory sqlite and run the full migration chain");
        let pool = sqlite_pool(&db);

        seed_user(pool, "user-member", "2021-01-01T00:00:00Z").await;
        seed_workspace(pool, "ws-ghost-owner", "user-member").await;
        seed_workspace_member(pool, "ws-ghost-owner", "user-member", "2021-01-01T00:00:00Z").await;
        seed_folder(
            pool,
            "folder-ghost-owner",
            "ws-ghost-owner",
            "Deleted Owner's Folder",
            Some("user-deleted-long-ago"),
        )
        .await;

        let result = migrate_folders_to_collections(&db).await;
        assert!(
            result.is_ok(),
            "migration must succeed when the folder's owner no longer exists: {result:?}"
        );

        let created_by =
            collection_created_by_column(pool, "ws-ghost-owner", "Deleted Owner's Folder").await;
        assert_eq!(
            created_by, "user-member",
            "a folder owner that no longer exists in users must not be honored \
             (would violate the new FK) — must fall back to the workspace member"
        );
    }
}
