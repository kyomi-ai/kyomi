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
    }

    let ct = |col: &str| sql_compat::cast_to_text(is_pg, col);
    let folder_sql = format!(
        "SELECT {id} AS id, {workspace_id} AS workspace_id, {parent_id} AS parent_id, name \
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

    for folder in &folders {
        let path = resolve_path(&folder.id, &folder_map);
        folder_paths.insert(folder.id.clone(), path.clone());
        collection_ids
            .entry((folder.workspace_id.clone(), path))
            .or_insert_with(|| Uuid::new_v4().to_string());
    }

    // Step 3: Create collections for each unique (workspace_id, folder_path).
    // Use ON CONFLICT / INSERT OR IGNORE to handle pre-existing collections
    // with the same name (e.g., a user had a "Revenue" collection AND folder).
    let now_expr = sql_compat::now(is_pg);
    let uuid_cast = |param: &str| sql_compat::cast_to_uuid(is_pg, param);

    let insert_collection_sql = if is_pg {
        format!(
            "INSERT INTO collections (id, workspace_id, name, description, color, is_public, created_at, updated_at) \
             VALUES ({id}, $2, $3, NULL, NULL, {bool_false}, {now_expr}, {now_expr}) \
             ON CONFLICT (workspace_id, name) DO NOTHING",
            id = uuid_cast("$1"),
            bool_false = sql_compat::bool_false(is_pg),
        )
    } else {
        format!(
            "INSERT OR IGNORE INTO collections (id, workspace_id, name, description, color, is_public, created_at, updated_at) \
             VALUES ({id}, $2, $3, NULL, NULL, {bool_false}, {now_expr}, {now_expr})",
            id = uuid_cast("$1"),
            bool_false = sql_compat::bool_false(is_pg),
        )
    };

    for ((workspace_id, path), collection_id) in &collection_ids {
        kyomi_core::db_execute!(
            db,
            &insert_collection_sql,
            collection_id,
            workspace_id,
            path
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
}
