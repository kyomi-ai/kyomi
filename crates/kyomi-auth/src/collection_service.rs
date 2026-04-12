// SPDX-License-Identifier: AGPL-3.0-or-later

//! Collection service — CRUD for dashboard collections.
//!
//! Ports Python's `routers/collections.py` business logic into a shared
//! service layer. Collections group dashboards for organization.
//!
//! Key design decisions:
//! - Free-function pattern (`&DbPool` first arg) matching other services
//! - Workspace-scoped: all operations filter by workspace_id
//! - CASCADE deletes on the junction table

use chrono::{DateTime, Utc};
use kyomi_core::sql_compat;
use kyomi_core::{db_execute, db_fetch_all, db_fetch_one, db_fetch_optional, db_fetch_scalar};
use kyomi_core::{DbPool, Result};
use serde::{Deserialize, Serialize};

// ─── Response types ──────────────────────────────────────────────────────────

/// A dashboard entry within a collection.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct DashboardInCollection {
    pub dashboard_id: String,
    pub title: String,
    pub position: i32,
    pub added_at: DateTime<Utc>,
}

/// A collection with its dashboard list.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollectionWithDashboards {
    pub id: String,
    pub workspace_id: String,
    pub name: String,
    pub description: Option<String>,
    pub color: Option<String>,
    pub is_public: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub dashboards: Vec<DashboardInCollection>,
}

/// Fields that can be updated on a collection.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct CollectionUpdates {
    pub name: Option<String>,
    pub description: Option<String>,
    pub color: Option<String>,
    pub is_public: Option<bool>,
}

/// Helper for collection-dashboard join rows (needs collection_id for grouping).
#[derive(Debug, Clone, sqlx::FromRow)]
struct CollectionDashboardRow {
    collection_id: String,
    dashboard_id: String,
    title: String,
    position: i32,
    added_at: DateTime<Utc>,
}

// ─── Validation ──────────────────────────────────────────────────────────────

fn validate_name(name: &str) -> Result<()> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(kyomi_core::Error::BadRequest(
            "Collection name must not be empty".into(),
        ));
    }
    if trimmed.len() > 255 {
        return Err(kyomi_core::Error::BadRequest(
            "Collection name must be at most 255 characters".into(),
        ));
    }
    Ok(())
}

fn validate_color(color: &str) -> Result<()> {
    // Must be #RRGGBB format
    if !color.starts_with('#') || color.len() != 7 {
        return Err(kyomi_core::Error::BadRequest(
            "Color must be in #RRGGBB format".into(),
        ));
    }
    if !color[1..].chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(kyomi_core::Error::BadRequest(
            "Color must be a valid hex color code".into(),
        ));
    }
    Ok(())
}

/// Check if a row exists. Returns true if the query returns at least one row.
async fn row_exists(db: &DbPool, sql: &str, binds: &[&str]) -> Result<bool> {
    match db {
        kyomi_core::db::DbPool::Postgres(pg) => {
            let mut q = sqlx::query_scalar::<_, i32>(sql);
            for b in binds {
                q = q.bind(*b);
            }
            Ok(q.fetch_optional(pg).await
                .map_err(|e| kyomi_core::Error::Internal(format!("exists check failed: {e}")))?
                .is_some())
        }
        kyomi_core::db::DbPool::Sqlite(sq) => {
            let mut q = sqlx::query_scalar::<_, i32>(sql);
            for b in binds {
                q = q.bind(*b);
            }
            Ok(q.fetch_optional(sq).await
                .map_err(|e| kyomi_core::Error::Internal(format!("exists check failed: {e}")))?
                .is_some())
        }
    }
}

// ─── Create collection ──────────────────────────────────────────────────────

/// Create a new collection in a workspace.
pub async fn create_collection(
    db: &DbPool,
    workspace_id: &str,
    name: &str,
    description: Option<&str>,
    color: Option<&str>,
    is_public: bool,
    doc_type: &str,
) -> Result<kyomi_core::models::Collection> {
    validate_name(name)?;
    if let Some(c) = color {
        validate_color(c)?;
    }

    let is_pg = db.is_postgres();
    let id = uuid::Uuid::new_v4().to_string();
    let now_expr = sql_compat::now(is_pg);
    let bool_val = if is_public {
        sql_compat::bool_true(is_pg)
    } else {
        sql_compat::bool_false(is_pg)
    };

    let insert_sql = format!(
        r#"
        INSERT INTO collections (id, workspace_id, name, description, color, is_public, doc_type, created_at, updated_at)
        VALUES ($1, $2, $3, $4, $5, {bool_val}, $6, {now_expr}, {now_expr})
        "#
    );

    db_execute!(db, &insert_sql, &id, workspace_id, name.trim(), &description, &color, doc_type)
        .map_err(|e| kyomi_core::Error::Internal(format!("failed to create collection: {e}")))?;

    // Fetch the created row
    let row = db_fetch_one!(
        db,
        kyomi_core::models::Collection,
        "SELECT id, workspace_id, name, description, color, is_public, created_at, updated_at FROM collections WHERE id = $1",
        &id
    )
    .map_err(|e| kyomi_core::Error::Internal(format!("failed to fetch created collection: {e}")))?;

    tracing::info!(collection_id = %row.id, "Created collection");
    Ok(row)
}

// ─── List collections ────────────────────────────────────────────────────────

/// List all collections in a workspace with their dashboards.
///
/// When `doc_type` is `Some`, only collections whose own `doc_type` column
/// matches are returned. When `None`, all collections are returned.
pub async fn list_collections(
    db: &DbPool,
    workspace_id: &str,
    doc_type: Option<&str>,
) -> Result<Vec<CollectionWithDashboards>> {
    // Fetch collections — optionally filtered by their own doc_type column
    let collections = if let Some(dt) = doc_type {
        db_fetch_all!(
            db,
            kyomi_core::models::Collection,
            r#"
            SELECT id, workspace_id, name, description, color, is_public, created_at, updated_at
            FROM collections
            WHERE workspace_id = $1 AND doc_type = $2
            ORDER BY created_at DESC
            "#,
            workspace_id,
            dt
        )
        .map_err(|e| kyomi_core::Error::Internal(format!("failed to list collections: {e}")))?
    } else {
        db_fetch_all!(
            db,
            kyomi_core::models::Collection,
            r#"
            SELECT id, workspace_id, name, description, color, is_public, created_at, updated_at
            FROM collections
            WHERE workspace_id = $1
            ORDER BY created_at DESC
            "#,
            workspace_id
        )
        .map_err(|e| kyomi_core::Error::Internal(format!("failed to list collections: {e}")))?
    };

    // Fetch all dashboards in collections for this workspace in one query
    let dashboard_rows = db_fetch_all!(
        db,
        CollectionDashboardRow,
        r#"
        SELECT cd.collection_id AS collection_id,
               cd.dashboard_id AS dashboard_id,
               cd.position, cd.added_at,
               d.title
        FROM collection_dashboards cd
        JOIN dashboards d ON cd.dashboard_id = d.dashboard_id
        JOIN collections c ON cd.collection_id = c.id
        WHERE c.workspace_id = $1
        ORDER BY cd.position ASC
        "#,
        workspace_id
    )
    .map_err(|e| {
        kyomi_core::Error::Internal(format!("failed to fetch collection dashboards: {e}"))
    })?;

    // Group dashboards by collection_id
    let mut dashboard_map: std::collections::HashMap<String, Vec<DashboardInCollection>> =
        std::collections::HashMap::new();
    for row in dashboard_rows {
        let entry = dashboard_map.entry(row.collection_id).or_default();
        entry.push(DashboardInCollection {
            dashboard_id: row.dashboard_id,
            title: row.title,
            position: row.position,
            added_at: row.added_at,
        });
    }

    let result = collections
        .into_iter()
        .map(|c| {
            let dashboards = dashboard_map.remove(&c.id).unwrap_or_default();
            CollectionWithDashboards {
                id: c.id,
                workspace_id: c.workspace_id,
                name: c.name,
                description: c.description,
                color: c.color,
                is_public: c.is_public,
                created_at: c.created_at,
                updated_at: c.updated_at,
                dashboards,
            }
        })
        .collect();

    Ok(result)
}

// ─── Get collection ──────────────────────────────────────────────────────────

/// Get a single collection with its dashboards.
pub async fn get_collection(
    db: &DbPool,
    collection_id: &str,
    workspace_id: &str,
) -> Result<Option<CollectionWithDashboards>> {
    // Validate UUID format
    uuid::Uuid::parse_str(collection_id)
        .map_err(|e| kyomi_core::Error::BadRequest(format!("Invalid collection_id: {e}")))?;

    let collection: Option<kyomi_core::models::Collection> = db_fetch_optional!(
        db,
        kyomi_core::models::Collection,
        r#"
        SELECT id, workspace_id, name, description, color, is_public, created_at, updated_at
        FROM collections
        WHERE id = $1 AND workspace_id = $2
        "#,
        collection_id,
        workspace_id
    )
    .map_err(|e| kyomi_core::Error::Internal(format!("failed to get collection: {e}")))?;

    let collection = match collection {
        Some(c) => c,
        None => return Ok(None),
    };

    let dashboards = db_fetch_all!(
        db,
        DashboardInCollection,
        r#"
        SELECT cd.dashboard_id AS dashboard_id,
               cd.position, cd.added_at, d.title
        FROM collection_dashboards cd
        JOIN dashboards d ON cd.dashboard_id = d.dashboard_id
        WHERE cd.collection_id = $1
        ORDER BY cd.position ASC
        "#,
        collection_id
    )
    .map_err(|e| {
        kyomi_core::Error::Internal(format!("failed to fetch collection dashboards: {e}"))
    })?;

    Ok(Some(CollectionWithDashboards {
        id: collection.id,
        workspace_id: collection.workspace_id,
        name: collection.name,
        description: collection.description,
        color: collection.color,
        is_public: collection.is_public,
        created_at: collection.created_at,
        updated_at: collection.updated_at,
        dashboards,
    }))
}

// ─── Update collection ──────────────────────────────────────────────────────

/// Partial update of a collection.
pub async fn update_collection(
    db: &DbPool,
    collection_id: &str,
    workspace_id: &str,
    updates: &CollectionUpdates,
) -> Result<bool> {
    // Validate if provided
    if let Some(ref name) = updates.name {
        validate_name(name)?;
    }
    if let Some(ref color) = updates.color {
        validate_color(color)?;
    }

    // Build dynamic UPDATE
    let mut set_parts: Vec<String> = Vec::new();
    let mut param_idx = 3u32; // $1 = collection_id, $2 = workspace_id

    if updates.name.is_some() {
        set_parts.push(format!("name = ${param_idx}"));
        param_idx += 1;
    }
    if updates.description.is_some() {
        set_parts.push(format!("description = ${param_idx}"));
        param_idx += 1;
    }
    if updates.color.is_some() {
        set_parts.push(format!("color = ${param_idx}"));
        param_idx += 1;
    }
    if updates.is_public.is_some() {
        set_parts.push(format!("is_public = ${param_idx}"));
        param_idx += 1;
    }

    // Always update updated_at
    set_parts.push(format!("updated_at = ${param_idx}"));

    if set_parts.len() <= 1 {
        // Only updated_at — no actual changes
        return Ok(false);
    }

    let sql = format!(
        "UPDATE collections SET {} WHERE id = $1 AND workspace_id = $2",
        set_parts.join(", ")
    );

    let now = chrono::Utc::now();
    // Dynamic SQL with variable bind count — use match pool directly
    let result = match db {
        kyomi_core::db::DbPool::Postgres(pg) => {
            let mut query = sqlx::query(&sql).bind(collection_id).bind(workspace_id);
            if let Some(ref name) = updates.name {
                query = query.bind(name.trim());
            }
            if let Some(ref description) = updates.description {
                query = query.bind(description);
            }
            if let Some(ref color) = updates.color {
                query = query.bind(color);
            }
            if let Some(is_public) = updates.is_public {
                query = query.bind(is_public);
            }
            query = query.bind(now);
            query.execute(pg).await.map(kyomi_core::db::DbQueryResult::from_pg)
        }
        kyomi_core::db::DbPool::Sqlite(sq) => {
            let mut query = sqlx::query(&sql).bind(collection_id).bind(workspace_id);
            if let Some(ref name) = updates.name {
                query = query.bind(name.trim());
            }
            if let Some(ref description) = updates.description {
                query = query.bind(description);
            }
            if let Some(ref color) = updates.color {
                query = query.bind(color);
            }
            if let Some(is_public) = updates.is_public {
                query = query.bind(is_public);
            }
            query = query.bind(now);
            query.execute(sq).await.map(kyomi_core::db::DbQueryResult::from_sqlite)
        }
    }
    .map_err(|e| {
        kyomi_core::Error::Internal(format!("failed to update collection: {e}"))
    })?;

    if result.rows_affected() == 0 {
        return Err(kyomi_core::Error::NotFound(format!(
            "Collection {collection_id} not found"
        )));
    }

    tracing::info!(collection_id = %collection_id, "Updated collection");
    Ok(true)
}

// ─── Delete collection ──────────────────────────────────────────────────────

/// Delete a collection. CASCADE removes junction rows.
pub async fn delete_collection(
    db: &DbPool,
    collection_id: &str,
    workspace_id: &str,
) -> Result<bool> {
    uuid::Uuid::parse_str(collection_id)
        .map_err(|e| kyomi_core::Error::BadRequest(format!("Invalid collection_id: {e}")))?;

    let result = db_execute!(
        db,
        "DELETE FROM collections WHERE id = $1 AND workspace_id = $2",
        collection_id,
        workspace_id
    )
    .map_err(|e| kyomi_core::Error::Internal(format!("failed to delete collection: {e}")))?;

    if result.rows_affected() == 0 {
        return Err(kyomi_core::Error::NotFound(format!(
            "Collection {collection_id} not found"
        )));
    }

    tracing::info!(collection_id = %collection_id, "Deleted collection");
    Ok(true)
}

// ─── Add dashboard to collection ─────────────────────────────────────────────

/// Add a dashboard to a collection.
///
/// If position is None, appends to end.
/// Returns error if the dashboard is already in the collection.
pub async fn add_dashboard(
    db: &DbPool,
    collection_id: &str,
    dashboard_id: &str,
    workspace_id: &str,
    position: Option<i32>,
) -> Result<()> {
    uuid::Uuid::parse_str(collection_id)
        .map_err(|e| kyomi_core::Error::BadRequest(format!("Invalid collection_id: {e}")))?;

    // Verify collection exists and belongs to workspace
    let exists = row_exists(
        db,
        "SELECT 1 FROM collections WHERE id = $1 AND workspace_id = $2",
        &[collection_id, workspace_id],
    )
    .await?;

    if !exists {
        return Err(kyomi_core::Error::NotFound(
            "Collection not found".into(),
        ));
    }

    // Verify dashboard exists
    let dash_exists = row_exists(
        db,
        "SELECT 1 FROM dashboards WHERE dashboard_id = $1",
        &[dashboard_id],
    )
    .await?;

    if !dash_exists {
        return Err(kyomi_core::Error::NotFound(
            "Dashboard not found".into(),
        ));
    }

    // Check if already in collection
    let already_exists = row_exists(
        db,
        "SELECT 1 FROM collection_dashboards WHERE collection_id = $1 AND dashboard_id = $2",
        &[collection_id, dashboard_id],
    )
    .await?;

    if already_exists {
        return Err(kyomi_core::Error::BadRequest(
            "Dashboard already in collection".into(),
        ));
    }

    // Determine position
    let pos = match position {
        Some(p) => p,
        None => {
            // Append to end
            let count: i64 = db_fetch_scalar!(
                db,
                i64,
                "SELECT COUNT(*) FROM collection_dashboards WHERE collection_id = $1",
                collection_id
            )
            .map_err(|e| {
                kyomi_core::Error::Internal(format!("failed to count dashboards: {e}"))
            })?;
            count as i32
        }
    };

    let is_pg = db.is_postgres();
    let now_expr = sql_compat::now(is_pg);
    let sql = format!(
        r#"
        INSERT INTO collection_dashboards (collection_id, dashboard_id, position, added_at)
        VALUES ($1, $2, $3, {now_expr})
        "#
    );

    db_execute!(db, &sql, collection_id, dashboard_id, &pos)
        .map_err(|e| kyomi_core::Error::Internal(format!("failed to add dashboard to collection: {e}")))?;

    tracing::info!(
        collection_id = %collection_id,
        dashboard_id = %dashboard_id,
        "Added dashboard to collection"
    );
    Ok(())
}

// ─── Remove dashboard from collection ────────────────────────────────────────

/// Remove a dashboard from a collection.
pub async fn remove_dashboard(
    db: &DbPool,
    collection_id: &str,
    dashboard_id: &str,
    workspace_id: &str,
) -> Result<()> {
    uuid::Uuid::parse_str(collection_id)
        .map_err(|e| kyomi_core::Error::BadRequest(format!("Invalid collection_id: {e}")))?;

    // Verify collection exists and belongs to workspace
    let exists = row_exists(
        db,
        "SELECT 1 FROM collections WHERE id = $1 AND workspace_id = $2",
        &[collection_id, workspace_id],
    )
    .await?;

    if !exists {
        return Err(kyomi_core::Error::NotFound(
            "Collection not found".into(),
        ));
    }

    let result = db_execute!(
        db,
        "DELETE FROM collection_dashboards WHERE collection_id = $1 AND dashboard_id = $2",
        collection_id,
        dashboard_id
    )
    .map_err(|e| {
        kyomi_core::Error::Internal(format!(
            "failed to remove dashboard from collection: {e}"
        ))
    })?;

    if result.rows_affected() == 0 {
        return Err(kyomi_core::Error::NotFound(
            "Dashboard not found in collection".into(),
        ));
    }

    tracing::info!(
        collection_id = %collection_id,
        dashboard_id = %dashboard_id,
        "Removed dashboard from collection"
    );
    Ok(())
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_name_empty_fails() {
        assert!(validate_name("").is_err());
        assert!(validate_name("   ").is_err());
    }

    #[test]
    fn validate_name_too_long_fails() {
        let long = "x".repeat(256);
        assert!(validate_name(&long).is_err());
    }

    #[test]
    fn validate_name_valid() {
        assert!(validate_name("My Collection").is_ok());
        assert!(validate_name("x".repeat(255).as_str()).is_ok());
        assert!(validate_name("a").is_ok());
    }

    #[test]
    fn validate_color_valid_hex() {
        assert!(validate_color("#FF0000").is_ok());
        assert!(validate_color("#00ff00").is_ok());
        assert!(validate_color("#123abc").is_ok());
    }

    #[test]
    fn validate_color_invalid() {
        assert!(validate_color("FF0000").is_err()); // missing #
        assert!(validate_color("#FFF").is_err()); // too short
        assert!(validate_color("#GGGGGG").is_err()); // invalid hex chars
        assert!(validate_color("#FF00001").is_err()); // too long
    }

    #[test]
    fn collection_updates_default() {
        let updates = CollectionUpdates::default();
        assert!(updates.name.is_none());
        assert!(updates.description.is_none());
        assert!(updates.color.is_none());
        assert!(updates.is_public.is_none());
    }

    #[test]
    fn dashboard_in_collection_serializes() {
        let d = DashboardInCollection {
            dashboard_id: "dash-1".into(),
            title: "Test".into(),
            position: 0,
            added_at: chrono::Utc::now(),
        };
        let json = serde_json::to_value(&d).unwrap();
        assert_eq!(json["dashboard_id"], "dash-1");
        assert_eq!(json["position"], 0);
    }
}
