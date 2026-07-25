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

use crate::dashboard_service;
use crate::sync_log_service;
use crate::websocket::WebSocketManager;
use kyomi_types::sync::{SyncActionType, entity_types};

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
    pub created_by: String,
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
    kyomi_core::db_with_pool!(db, |p| {
        let mut q = sqlx::query_scalar::<_, i32>(sql);
        for b in binds {
            q = q.bind(*b);
        }
        q.fetch_optional(p).await
            .map_err(|e| kyomi_core::Error::Internal(format!("exists check failed: {e}")))
            .map(|opt| opt.is_some())
    })
}

// ─── Create collection ──────────────────────────────────────────────────────

/// Parameters for creating a new collection.
pub struct NewCollectionParams<'a> {
    pub db: &'a DbPool,
    pub workspace_id: &'a str,
    pub name: &'a str,
    pub description: Option<&'a str>,
    pub color: Option<&'a str>,
    pub is_public: bool,
    pub doc_type: &'a str,
    pub created_by: &'a str,
}

/// Create a new collection in a workspace.
pub async fn create_collection(
    params: NewCollectionParams<'_>,
) -> Result<kyomi_core::models::Collection> {
    let NewCollectionParams {
        db,
        workspace_id,
        name,
        description,
        color,
        is_public,
        doc_type,
        created_by,
    } = params;

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
        INSERT INTO collections (id, workspace_id, name, description, color, is_public, doc_type, created_by, created_at, updated_at)
        VALUES ($1, $2, $3, $4, $5, {bool_val}, $6, $7, {now_expr}, {now_expr})
        "#
    );

    db_execute!(db, &insert_sql, &id, workspace_id, name.trim(), &description, &color, doc_type, created_by)
        .map_err(|e| kyomi_core::Error::Internal(format!("failed to create collection: {e}")))?;

    // Fetch the created row
    let row = db_fetch_one!(
        db,
        kyomi_core::models::Collection,
        "SELECT id, workspace_id, created_by, name, description, color, is_public, created_at, updated_at FROM collections WHERE id = $1",
        &id
    )
    .map_err(|e| kyomi_core::Error::Internal(format!("failed to fetch created collection: {e}")))?;

    tracing::info!(collection_id = %row.id, "Created collection");
    Ok(row)
}

// ─── List collections ────────────────────────────────────────────────────────

/// List all collections in a workspace with their dashboards.
///
/// Only returns collections that the user can see: their own (`created_by =
/// user_id`) or public (`is_public = true`).
///
/// When `doc_type` is `Some`, only collections whose own `doc_type` column
/// matches are returned. When `None`, all collections are returned.
pub async fn list_collections(
    db: &DbPool,
    workspace_id: &str,
    user_id: &str,
    doc_type: Option<&str>,
) -> Result<Vec<CollectionWithDashboards>> {
    let is_pg = db.is_postgres();
    let bool_true = sql_compat::bool_true(is_pg);

    // Fetch collections — optionally filtered by their own doc_type column
    let collections = if let Some(dt) = doc_type {
        let sql = format!(
            r#"
            SELECT id, workspace_id, created_by, name, description, color, is_public, created_at, updated_at
            FROM collections
            WHERE workspace_id = $1 AND doc_type = $2 AND (created_by = $3 OR is_public = {bool_true})
            ORDER BY created_at DESC
            "#
        );
        db_fetch_all!(
            db,
            kyomi_core::models::Collection,
            &sql,
            workspace_id,
            dt,
            user_id
        )
        .map_err(|e| kyomi_core::Error::Internal(format!("failed to list collections: {e}")))?
    } else {
        let sql = format!(
            r#"
            SELECT id, workspace_id, created_by, name, description, color, is_public, created_at, updated_at
            FROM collections
            WHERE workspace_id = $1 AND (created_by = $2 OR is_public = {bool_true})
            ORDER BY created_at DESC
            "#
        );
        db_fetch_all!(
            db,
            kyomi_core::models::Collection,
            &sql,
            workspace_id,
            user_id
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
                created_by: c.created_by,
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
///
/// Returns `None` if the collection does not exist in the workspace, or if the
/// user cannot see it (not the owner and not public).
pub async fn get_collection(
    db: &DbPool,
    collection_id: &str,
    workspace_id: &str,
    user_id: &str,
) -> Result<Option<CollectionWithDashboards>> {
    // Validate UUID format
    uuid::Uuid::parse_str(collection_id)
        .map_err(|e| kyomi_core::Error::BadRequest(format!("Invalid collection_id: {e}")))?;

    let is_pg = db.is_postgres();
    let bool_true = sql_compat::bool_true(is_pg);
    let sql = format!(
        r#"
        SELECT id, workspace_id, created_by, name, description, color, is_public, created_at, updated_at
        FROM collections
        WHERE id = $1 AND workspace_id = $2 AND (created_by = $3 OR is_public = {bool_true})
        "#
    );

    let collection: Option<kyomi_core::models::Collection> = db_fetch_optional!(
        db,
        kyomi_core::models::Collection,
        &sql,
        collection_id,
        workspace_id,
        user_id
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
        created_by: collection.created_by,
        name: collection.name,
        description: collection.description,
        color: collection.color,
        is_public: collection.is_public,
        created_at: collection.created_at,
        updated_at: collection.updated_at,
        dashboards,
    }))
}

// ─── Dashboard visibility transitions (KYO-238) ──────────────────────────────
//
// A dashboard's sync visibility is derived, not stored on the row —
// `dashboard_service::is_doc_publicly_visible` is `true` whenever the
// dashboard belongs to at least one public collection. Four mutations in
// this file can flip that derived value: adding/removing a dashboard from a
// collection, and `update_collection` changing `is_public`. None of them is
// a dashboard content edit, so none of the existing dashboard-mutation sync
// paths cover them — they need their own broadcast + sync_log writes,
// mirrored on `websocket::helpers::broadcast_dashboard_visibility_change`.

/// Reports a dashboard's overall sync-visibility transition caused by a
/// collection-membership or `is_public` mutation, when one actually
/// occurred (`was_visible != now_visible`). Callers use this to decide
/// whether to fire the matching live broadcast — the `sync_log` half is
/// already written by the time this is returned.
pub struct DashboardVisibilityTransition {
    pub dashboard_id: String,
    pub owner_user_id: String,
    pub now_public: bool,
}

/// Persist the `sync_log` row(s) for a dashboard visibility transition so an
/// offline member converges to the same state as the live broadcast on
/// their next delta sync (`sync_log_service::get_entries_since`).
///
/// Going public writes one `Update` row, `is_workspace_visible: true` —
/// every member's delta picks it up, including the owner (a harmless
/// idempotent re-apply of a snapshot they already have).
///
/// Going private writes two rows, **atomically, in one transaction** (via
/// `sync_log_service::write_sync_entries_in_transaction`), in order:
/// 1. `Delete`, `is_workspace_visible: true`. `true` is not a mistake for a
///    row about to go private: `get_entries_since` filters on
///    `is_workspace_visible OR owner_user_id = requester`, so this row has
///    to reach everyone who *had* visibility a moment ago, not the new
///    (narrower) audience — a `false` row here would be invisible to every
///    non-owner and the eviction would never reach an offline member.
/// 2. `Update`, `is_workspace_visible: false`, with the fresh snapshot,
///    scoped to the owner via `owner_user_id`. Applied *after* the Delete
///    in `sync_id` order, so the owner's delta ends with the dashboard
///    intact instead of evicted by row 1.
///
/// The transaction matters because these two rows are not independently
/// self-consistent the way every other `write_sync_entry` call site's
/// single row is: if only the Delete landed (a transient error between the
/// two sequential inserts is entirely plausible — a pool timeout, a
/// dropped connection), that row alone is workspace-visible and would
/// evict the *owner's own* cache on their own next delta, with no
/// compensating Update ever coming to fix it. Wrapping both in one
/// transaction restores the ordinary, recoverable failure mode: no rows at
/// all means "not converged yet", not "converged to the wrong state".
///
/// Mirrors the two-message shape of
/// `websocket::helpers::broadcast_dashboard_visibility_change`, persisted
/// instead of pushed live.
async fn write_visibility_sync_log(
    db: &DbPool,
    dashboard_id: &str,
    workspace_id: &str,
    owner_user_id: &str,
    entity_type: &'static str,
    now_public: bool,
) {
    // Never write an Update/Insert-type row with `data: None` — same
    // discipline as the live broadcast (KYO-218/KYO-245). If the snapshot
    // can't be fetched, skip the write entirely: a missing sync_log entry
    // means "not converged yet" (recoverable on the next mutation or a full
    // bootstrap), which is the only safe failure mode here. A DB error and
    // a genuinely-missing row are distinguished in logs via
    // `fetch_dashboard_snapshot`'s `Result` (KYO-245).
    let snapshot = match dashboard_service::fetch_dashboard_snapshot(db, dashboard_id, owner_user_id)
        .await
    {
        Ok(Some(snapshot)) => snapshot,
        Ok(None) => {
            tracing::warn!(
                dashboard_id,
                "visibility sync log: snapshot unavailable; skipping write"
            );
            return;
        }
        Err(e) => {
            tracing::error!(
                dashboard_id,
                error = %e,
                "visibility sync log: fetch failed; skipping write"
            );
            return;
        }
    };

    let update_entry = sync_log_service::SyncEntryParams {
        entity_type,
        entity_id: dashboard_id,
        workspace_id,
        action: SyncActionType::Update,
        data: Some(snapshot),
        owner_user_id: Some(owner_user_id),
        is_workspace_visible: now_public,
    };

    let result = if now_public {
        sync_log_service::write_sync_entries_in_transaction(
            db,
            std::slice::from_ref(&update_entry),
        )
        .await
    } else {
        let delete_entry = sync_log_service::SyncEntryParams {
            entity_type,
            entity_id: dashboard_id,
            workspace_id,
            action: SyncActionType::Delete,
            data: None,
            owner_user_id: Some(owner_user_id),
            is_workspace_visible: true,
        };
        sync_log_service::write_sync_entries_in_transaction(db, &[delete_entry, update_entry])
            .await
    };

    if let Err(e) = result {
        tracing::warn!(
            error = %e, dashboard_id,
            "failed to write visibility sync log entries"
        );
    }
}

/// Detect whether a dashboard's overall sync-visibility (visible via *any*
/// public collection membership) changed as a result of an add/remove-from-
/// collection mutation, and if so, persist the matching `sync_log` row(s).
///
/// `was_visible` must be captured by the caller *before* the mutation ran.
/// Returns `None` when visibility did not change — e.g. the dashboard
/// remained visible through a different public collection, or was never
/// visible at all — so the caller knows not to fire a live broadcast.
async fn record_dashboard_visibility_transition(
    db: &DbPool,
    dashboard_id: &str,
    workspace_id: &str,
    was_visible: bool,
) -> Result<Option<DashboardVisibilityTransition>> {
    let now_visible = dashboard_service::is_doc_publicly_visible(db, dashboard_id).await;
    if was_visible == now_visible {
        return Ok(None);
    }

    let dashboard = dashboard_service::get_dashboard_unchecked(db, dashboard_id, workspace_id)
        .await?
        .ok_or_else(|| {
            kyomi_core::Error::NotFound(format!("Dashboard {dashboard_id} not found"))
        })?;

    let entity_type = if dashboard.doc_type().is_knowledge() {
        entity_types::KNOWLEDGE
    } else {
        entity_types::DASHBOARD
    };

    write_visibility_sync_log(
        db,
        dashboard_id,
        workspace_id,
        &dashboard.user_id,
        entity_type,
        now_visible,
    )
    .await;

    Ok(Some(DashboardVisibilityTransition {
        dashboard_id: dashboard_id.to_string(),
        owner_user_id: dashboard.user_id,
        now_public: now_visible,
    }))
}

/// Fan a collection's `is_public` flip out to every dashboard it contains,
/// off the request path.
///
/// A collection can hold hundreds of dashboards. Computing the fan-out
/// synchronously inside `update_collection` would make a single toggle
/// block the response on up to N sequential sync_log writes and broadcasts.
/// This spawns the fan-out instead, matching
/// `dashboard_service::spawn_rechunk_document`'s existing precedent in this
/// crate for keeping a per-mutation side effect that scales with entity
/// count off the interactive request path — `update_collection` returns as
/// soon as the `collections` row itself is updated.
///
/// The membership query that decides *which* dashboards actually changed
/// visibility is a single bulk query, not one round trip per dashboard: a
/// dashboard already visible through a second public collection does not
/// need a broadcast just because this collection flipped, and this query
/// answers that for every dashboard in the collection at once.
///
/// Returns the `JoinHandle` so tests can await completion; production call
/// sites drop it (fire-and-forget, matching `spawn_rechunk_document`).
fn spawn_collection_visibility_fanout(
    db: DbPool,
    collection_id: String,
    workspace_id: String,
    old_is_public: bool,
    new_is_public: bool,
    ws_manager: Option<WebSocketManager>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        #[derive(sqlx::FromRow)]
        struct FanoutRow {
            dashboard_id: String,
            owner_user_id: String,
            doc_type: String,
            // 1/0, not a native bool column — see `is_publicly_shared` in
            // dashboard_service.rs for why CASE WHEN...THEN 1 ELSE 0 is
            // decoded as an integer rather than trusted as bool cross-db.
            visible_via_other_collections: i32,
        }

        let is_pg = db.is_postgres();
        let bool_true = sql_compat::bool_true(is_pg);
        let sql = format!(
            r#"
            SELECT cd.dashboard_id AS dashboard_id,
                   d.user_id AS owner_user_id,
                   d.doc_type AS doc_type,
                   CASE WHEN EXISTS (
                       SELECT 1 FROM collection_dashboards cd2
                       JOIN collections c2 ON cd2.collection_id = c2.id
                       WHERE cd2.dashboard_id = cd.dashboard_id
                         AND cd2.collection_id != $1
                         AND c2.is_public = {bool_true}
                   ) THEN 1 ELSE 0 END AS visible_via_other_collections
            FROM collection_dashboards cd
            JOIN dashboards d ON cd.dashboard_id = d.dashboard_id
            WHERE cd.collection_id = $1
            "#
        );

        let rows: Vec<FanoutRow> = match db_fetch_all!(db, FanoutRow, &sql, &collection_id) {
            Ok(rows) => rows,
            Err(e) => {
                tracing::error!(
                    collection_id = %collection_id, error = %e,
                    "visibility fan-out: failed to load collection membership"
                );
                return;
            }
        };

        for row in rows {
            let visible_via_other_collections = row.visible_via_other_collections != 0;
            let was_visible = visible_via_other_collections || old_is_public;
            let now_visible = visible_via_other_collections || new_is_public;
            if was_visible == now_visible {
                continue;
            }

            let entity_type = if row.doc_type == "knowledge" {
                entity_types::KNOWLEDGE
            } else {
                entity_types::DASHBOARD
            };

            write_visibility_sync_log(
                &db,
                &row.dashboard_id,
                &workspace_id,
                &row.owner_user_id,
                entity_type,
                now_visible,
            )
            .await;

            if let Some(ref manager) = ws_manager {
                crate::websocket::helpers::broadcast_dashboard_visibility_change(
                    &db,
                    manager,
                    &row.dashboard_id,
                    &workspace_id,
                    &row.owner_user_id,
                    now_visible,
                )
                .await;
            }
        }
    })
}

// ─── Update collection ──────────────────────────────────────────────────────

/// Partial update of a collection.
///
/// `ws_manager` is used only when `updates.is_public` actually changes the
/// collection's stored value — see `spawn_collection_visibility_fanout`.
pub async fn update_collection(
    db: &DbPool,
    collection_id: &str,
    workspace_id: &str,
    updates: &CollectionUpdates,
    ws_manager: Option<&WebSocketManager>,
) -> Result<bool> {
    // Validate if provided
    if let Some(ref name) = updates.name {
        validate_name(name)?;
    }
    if let Some(ref color) = updates.color {
        validate_color(color)?;
    }

    // Capture the pre-update is_public value when a transition might occur.
    // `db_fetch_optional!` (not `_scalar!`, which uses `fetch_one`) so a
    // nonexistent collection_id falls through to the existing
    // rows_affected() == 0 / NotFound handling below, unchanged.
    let old_is_public: Option<bool> = if updates.is_public.is_some() {
        db_fetch_optional!(
            db,
            (bool,),
            "SELECT is_public FROM collections WHERE id = $1 AND workspace_id = $2",
            collection_id,
            workspace_id
        )
        .map_err(|e| {
            kyomi_core::Error::Internal(format!("failed to read collection visibility: {e}"))
        })?
        .map(|(v,)| v)
    } else {
        None
    };

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
    // Dynamic SQL with variable bind count — identical for both backends.
    let rows_affected = kyomi_core::db_with_pool!(db, |p| {
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
        query.execute(p).await.map(|r| r.rows_affected())
    })
    .map_err(|e| {
        kyomi_core::Error::Internal(format!("failed to update collection: {e}"))
    })?;

    if rows_affected == 0 {
        return Err(kyomi_core::Error::NotFound(format!(
            "Collection {collection_id} not found"
        )));
    }

    // Fan out the visibility transition to every dashboard in the
    // collection, off the request path (see
    // `spawn_collection_visibility_fanout`). Only a genuine transition —
    // the caller may have re-sent the current value, which is not one.
    if let (Some(new_is_public), Some(old_is_public)) = (updates.is_public, old_is_public)
        && new_is_public != old_is_public
    {
        spawn_collection_visibility_fanout(
            db.clone(),
            collection_id.to_string(),
            workspace_id.to_string(),
            old_is_public,
            new_is_public,
            ws_manager.cloned(),
        );
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
///
/// Returns `Some(DashboardVisibilityTransition)` when this add changed the
/// dashboard's overall sync visibility (it was private and this is its
/// first public collection) — the caller fires the matching live broadcast
/// via `websocket::helpers::broadcast_dashboard_visibility_change`. The
/// `sync_log` half is already written before this returns.
pub async fn add_dashboard(
    db: &DbPool,
    collection_id: &str,
    dashboard_id: &str,
    workspace_id: &str,
    user_id: &str,
    position: Option<i32>,
) -> Result<Option<DashboardVisibilityTransition>> {
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

    // Verify the requesting user owns the dashboard
    let is_owner = row_exists(
        db,
        "SELECT 1 FROM dashboards WHERE dashboard_id = $1 AND user_id = $2",
        &[dashboard_id, user_id],
    )
    .await?;

    if !is_owner {
        return Err(kyomi_core::Error::Forbidden(
            "Only the document owner can add it to a collection".into(),
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

    // Capture visibility before the insert so the post-insert comparison in
    // `record_dashboard_visibility_transition` can tell a genuine
    // private->public transition apart from "already visible via another
    // public collection".
    let was_visible = dashboard_service::is_doc_publicly_visible(db, dashboard_id).await;

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

    record_dashboard_visibility_transition(db, dashboard_id, workspace_id, was_visible).await
}

// ─── Remove dashboard from collection ────────────────────────────────────────

/// Remove a dashboard from a collection.
///
/// Returns `Some(DashboardVisibilityTransition)` when this removal changed
/// the dashboard's overall sync visibility (it had no other public
/// collection membership left) — the caller fires the matching live
/// broadcast via `websocket::helpers::broadcast_dashboard_visibility_change`.
/// The `sync_log` half is already written before this returns.
pub async fn remove_dashboard(
    db: &DbPool,
    collection_id: &str,
    dashboard_id: &str,
    workspace_id: &str,
) -> Result<Option<DashboardVisibilityTransition>> {
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

    // Capture visibility before the delete — see the matching comment in
    // `add_dashboard`.
    let was_visible = dashboard_service::is_doc_publicly_visible(db, dashboard_id).await;

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

    record_dashboard_visibility_transition(db, dashboard_id, workspace_id, was_visible).await
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

    // ─── Integration tests (async, in-memory SQLite) ─────────────────────────
    //
    // Regression coverage for KYO-200: `doc_type` was added to `collections`
    // on Postgres (20260410000001_collection_doc_type.sql) but had no SQLite
    // counterpart until migrations-sqlite/00026_collection_doc_type.sql. Both
    // `create_collection` (unconditional `doc_type` insert) and
    // `list_collections` (`doc_type` filter) failed with
    // "no such column: doc_type" on SQLite before that migration existed.

    use sqlx::sqlite::SqlitePoolOptions;

    async fn test_pool() -> DbPool {
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

    fn sqlite_pool(db: &DbPool) -> &sqlx::SqlitePool {
        match db {
            DbPool::Sqlite(sq) => sq,
            _ => panic!("test requires sqlite pool"),
        }
    }

    async fn seed_user(sq: &sqlx::SqlitePool, user_id: &str, email: &str) {
        sqlx::query("INSERT INTO users (user_id, email) VALUES ($1, $2)")
            .bind(user_id)
            .bind(email)
            .execute(sq)
            .await
            .expect("insert user");
    }

    async fn seed_workspace(sq: &sqlx::SqlitePool, workspace_id: &str, owner_user_id: &str) {
        sqlx::query(
            "INSERT INTO workspaces (workspace_id, name, owner_user_id) VALUES ($1, $2, $3)",
        )
        .bind(workspace_id)
        .bind(format!("Workspace {workspace_id}"))
        .bind(owner_user_id)
        .execute(sq)
        .await
        .expect("insert workspace");
    }

    #[tokio::test]
    async fn create_and_list_collections_filters_by_doc_type_on_sqlite() {
        let db = test_pool().await;
        let sq = sqlite_pool(&db);

        seed_user(sq, "user-1", "user1@example.com").await;
        seed_workspace(sq, "ws-1", "user-1").await;

        let dashboard_collection = create_collection(NewCollectionParams {
            db: &db,
            workspace_id: "ws-1",
            name: "Dashboards Folder",
            description: None,
            color: None,
            is_public: false,
            doc_type: "dashboard",
            created_by: "user-1",
        })
        .await
        .expect("create dashboard collection on sqlite");

        let knowledge_collection = create_collection(NewCollectionParams {
            db: &db,
            workspace_id: "ws-1",
            name: "Knowledge Folder",
            description: None,
            color: None,
            is_public: false,
            doc_type: "knowledge",
            created_by: "user-1",
        })
        .await
        .expect("create knowledge collection on sqlite");

        let dashboards_only = list_collections(&db, "ws-1", "user-1", Some("dashboard"))
            .await
            .expect("list dashboard-scoped collections on sqlite");
        assert_eq!(dashboards_only.len(), 1);
        assert_eq!(dashboards_only[0].id, dashboard_collection.id);

        let knowledge_only = list_collections(&db, "ws-1", "user-1", Some("knowledge"))
            .await
            .expect("list knowledge-scoped collections on sqlite");
        assert_eq!(knowledge_only.len(), 1);
        assert_eq!(knowledge_only[0].id, knowledge_collection.id);

        let unfiltered = list_collections(&db, "ws-1", "user-1", None)
            .await
            .expect("list all collections on sqlite");
        assert_eq!(unfiltered.len(), 2);
    }

    // ─── Visibility transitions (KYO-238) ────────────────────────────────────
    //
    // A dashboard's sync visibility is derived from collection membership,
    // not stored on the row. These tests lock in both the live-broadcast
    // routing (`websocket::helpers::broadcast_dashboard_visibility_change`)
    // and the `sync_log` writes that let a member who was offline across the
    // transition converge to the same state via
    // `sync_log_service::get_entries_since` on their next delta sync.

    async fn create_test_dashboard(
        db: &DbPool,
        owner: &str,
        workspace_id: &str,
        title: &str,
    ) -> String {
        dashboard_service::create_dashboard(
            db,
            owner,
            workspace_id,
            title,
            "# content",
            kyomi_core::models::DocType::Dashboard,
            None,
        )
        .await
        .expect("create dashboard")
    }

    /// Seeds users, workspace, *and* `workspace_users` membership rows —
    /// `WebSocketManager::broadcast_to_workspace` queries `workspace_users`
    /// to resolve who to deliver to, so a member missing from that table
    /// silently never receives a broadcast (indistinguishable from a
    /// correctly-scoped exclusion without this).
    async fn seed_two_users_one_workspace(sq: &sqlx::SqlitePool) {
        seed_user(sq, "user-a", "a@test.local").await;
        seed_user(sq, "user-b", "b@test.local").await;
        seed_workspace(sq, "ws-1", "user-a").await;

        sqlx::query(
            "INSERT INTO workspace_users (workspace_id, user_id, role, active) \
             VALUES ('ws-1', 'user-a', 'workspace_admin', 1)",
        )
        .execute(sq)
        .await
        .expect("insert workspace_users user-a");
        sqlx::query(
            "INSERT INTO workspace_users (workspace_id, user_id, role, active) \
             VALUES ('ws-1', 'user-b', 'user', 1)",
        )
        .execute(sq)
        .await
        .expect("insert workspace_users user-b");
    }

    // ── Live broadcast routing ───────────────────────────────────────────

    #[tokio::test]
    async fn add_dashboard_to_public_collection_pushes_update_to_non_owner_only() {
        let db = test_pool().await;
        let sq = sqlite_pool(&db);
        seed_two_users_one_workspace(sq).await;

        let dashboard_id = create_test_dashboard(&db, "user-a", "ws-1", "Private Dash").await;
        let collection = create_collection(NewCollectionParams {
            db: &db,
            workspace_id: "ws-1",
            name: "Public Folder",
            description: None,
            color: None,
            is_public: true,
            doc_type: "dashboard",
            created_by: "user-a",
        })
        .await
        .expect("create public collection");

        let manager = crate::websocket::WebSocketManager::new(None, db.clone());
        let (_conn_a, mut rx_a) = manager.connect("user-a").expect("connect user-a");
        let (_conn_b, mut rx_b) = manager.connect("user-b").expect("connect user-b");
        rx_a.try_recv().expect("heartbeat for user-a");
        rx_b.try_recv().expect("heartbeat for user-b");

        let transition = add_dashboard(&db, &collection.id, &dashboard_id, "ws-1", "user-a", None)
            .await
            .expect("add_dashboard")
            .expect("adding to a public collection must report a visibility transition");
        assert!(transition.now_public);
        assert_eq!(transition.owner_user_id, "user-a");

        crate::websocket::helpers::broadcast_dashboard_visibility_change(
            &db, &manager, &dashboard_id, "ws-1", "user-a", true,
        )
        .await;

        let msg_b: serde_json::Value = serde_json::from_str(
            &rx_b
                .try_recv()
                .expect("non-owner should receive the visibility broadcast"),
        )
        .expect("valid JSON");
        assert_eq!(msg_b["data"]["action"], "update");
        assert_eq!(msg_b["data"]["entity_id"], dashboard_id);
        assert!(
            !msg_b["data"]["data"].is_null(),
            "non-owner's update must carry the snapshot: {msg_b}"
        );

        let result_a = rx_a.try_recv();
        assert!(
            result_a.is_err(),
            "owner already has it — must not receive an extra broadcast, got: {result_a:?}"
        );
    }

    #[tokio::test]
    async fn remove_dashboard_evicts_non_owner_and_updates_owner() {
        let db = test_pool().await;
        let sq = sqlite_pool(&db);
        seed_two_users_one_workspace(sq).await;

        let dashboard_id = create_test_dashboard(&db, "user-a", "ws-1", "Shared Dash").await;
        let collection = create_collection(NewCollectionParams {
            db: &db,
            workspace_id: "ws-1",
            name: "Public Folder",
            description: None,
            color: None,
            is_public: true,
            doc_type: "dashboard",
            created_by: "user-a",
        })
        .await
        .expect("create public collection");

        add_dashboard(&db, &collection.id, &dashboard_id, "ws-1", "user-a", None)
            .await
            .expect("add_dashboard");

        let manager = crate::websocket::WebSocketManager::new(None, db.clone());
        let (_conn_a, mut rx_a) = manager.connect("user-a").expect("connect user-a");
        let (_conn_b, mut rx_b) = manager.connect("user-b").expect("connect user-b");
        rx_a.try_recv().expect("heartbeat for user-a");
        rx_b.try_recv().expect("heartbeat for user-b");

        let transition = remove_dashboard(&db, &collection.id, &dashboard_id, "ws-1")
            .await
            .expect("remove_dashboard")
            .expect("removing from the only public collection must report a transition");
        assert!(!transition.now_public);

        crate::websocket::helpers::broadcast_dashboard_visibility_change(
            &db, &manager, &dashboard_id, "ws-1", "user-a", false,
        )
        .await;

        let msg_b: serde_json::Value = serde_json::from_str(
            &rx_b
                .try_recv()
                .expect("non-owner should receive the eviction broadcast"),
        )
        .expect("valid JSON");
        assert_eq!(msg_b["data"]["action"], "delete");
        assert!(msg_b["data"]["data"].is_null());

        let msg_a: serde_json::Value = serde_json::from_str(
            &rx_a
                .try_recv()
                .expect("owner should receive a refreshed snapshot, not be evicted"),
        )
        .expect("valid JSON");
        assert_eq!(msg_a["data"]["action"], "update");
        assert!(
            !msg_a["data"]["data"].is_null(),
            "owner's update must carry the snapshot: {msg_a}"
        );

        assert!(
            rx_a.try_recv().is_err(),
            "owner must not also receive a Delete"
        );
    }

    #[tokio::test]
    async fn collection_visibility_fanout_reaches_every_dashboard() {
        let db = test_pool().await;
        let sq = sqlite_pool(&db);
        seed_two_users_one_workspace(sq).await;

        let d1 = create_test_dashboard(&db, "user-a", "ws-1", "Dash One").await;
        let d2 = create_test_dashboard(&db, "user-a", "ws-1", "Dash Two").await;
        let collection = create_collection(NewCollectionParams {
            db: &db,
            workspace_id: "ws-1",
            name: "Folder",
            description: None,
            color: None,
            is_public: false,
            doc_type: "dashboard",
            created_by: "user-a",
        })
        .await
        .expect("create collection");

        assert!(
            add_dashboard(&db, &collection.id, &d1, "ws-1", "user-a", None)
                .await
                .expect("add d1")
                .is_none(),
            "collection is still private — adding must not report a transition yet"
        );
        assert!(
            add_dashboard(&db, &collection.id, &d2, "ws-1", "user-a", None)
                .await
                .expect("add d2")
                .is_none()
        );

        let manager = crate::websocket::WebSocketManager::new(None, db.clone());
        let (_conn_a, mut rx_a) = manager.connect("user-a").expect("connect user-a");
        let (_conn_b, mut rx_b) = manager.connect("user-b").expect("connect user-b");
        rx_a.try_recv().expect("heartbeat for user-a");
        rx_b.try_recv().expect("heartbeat for user-b");

        // Exercises the same fan-out `update_collection` spawns in
        // production; called directly (it's private to this module) so the
        // test can await the `JoinHandle` instead of racing a detached task.
        spawn_collection_visibility_fanout(
            db.clone(),
            collection.id.clone(),
            "ws-1".to_string(),
            false,
            true,
            Some(manager.clone()),
        )
        .await
        .expect("fan-out task must not panic");

        let mut seen = std::collections::HashSet::new();
        for _ in 0..2 {
            let msg: serde_json::Value = serde_json::from_str(
                &rx_b
                    .try_recv()
                    .expect("non-owner should receive a broadcast per dashboard"),
            )
            .expect("valid JSON");
            assert_eq!(msg["data"]["action"], "update");
            seen.insert(msg["data"]["entity_id"].as_str().unwrap().to_string());
        }
        assert_eq!(seen, std::collections::HashSet::from([d1, d2]));
        assert!(
            rx_b.try_recv().is_err(),
            "no more than one broadcast per dashboard expected"
        );
        assert!(
            rx_a.try_recv().is_err(),
            "owner already has both — must not receive fan-out broadcasts"
        );
    }

    #[tokio::test]
    async fn update_collection_is_public_same_value_is_not_a_transition() {
        let db = test_pool().await;
        let sq = sqlite_pool(&db);
        seed_user(sq, "user-a", "a@test.local").await;
        seed_workspace(sq, "ws-1", "user-a").await;

        let d1 = create_test_dashboard(&db, "user-a", "ws-1", "Dash One").await;
        let collection = create_collection(NewCollectionParams {
            db: &db,
            workspace_id: "ws-1",
            name: "Folder",
            description: None,
            color: None,
            is_public: true,
            doc_type: "dashboard",
            created_by: "user-a",
        })
        .await
        .expect("create collection");
        add_dashboard(&db, &collection.id, &d1, "ws-1", "user-a", None)
            .await
            .expect("add d1");

        let cursor = sync_log_service::get_latest_sync_id(&db, "ws-1")
            .await
            .expect("cursor");

        let updates = CollectionUpdates {
            name: Some("Renamed Folder".to_string()),
            is_public: Some(true), // same as the stored value — not a transition
            ..Default::default()
        };
        update_collection(&db, &collection.id, "ws-1", &updates, None)
            .await
            .expect("update_collection");

        let entries = sync_log_service::get_entries_since(&db, "ws-1", cursor, "user-a", 50)
            .await
            .expect("get_entries_since");
        assert!(
            entries.is_empty(),
            "re-sending the current is_public value must not write any new \
             sync_log entries: {entries:?}"
        );
    }

    // ── Offline convergence via the delta path ───────────────────────────
    //
    // A member who is offline (no WebSocket connection) across the
    // transition never receives the live broadcast at all. These tests
    // seed a delta cursor *before* the transition, run the mutation with no
    // WebSocketManager involved, and assert `get_entries_since` alone —
    // the same call the server makes for a `sync_delta` request — brings
    // the member to the correct state.

    #[tokio::test]
    async fn offline_non_owner_converges_via_delta_after_going_public() {
        let db = test_pool().await;
        let sq = sqlite_pool(&db);
        seed_two_users_one_workspace(sq).await;

        let dashboard_id = create_test_dashboard(&db, "user-a", "ws-1", "Private Dash").await;
        let collection = create_collection(NewCollectionParams {
            db: &db,
            workspace_id: "ws-1",
            name: "Public Folder",
            description: None,
            color: None,
            is_public: true,
            doc_type: "dashboard",
            created_by: "user-a",
        })
        .await
        .expect("create public collection");

        // user-b's last-synced cursor, captured before the transition —
        // simulates being offline while it happens.
        let cursor = sync_log_service::get_latest_sync_id(&db, "ws-1")
            .await
            .expect("cursor");

        add_dashboard(&db, &collection.id, &dashboard_id, "ws-1", "user-a", None)
            .await
            .expect("add_dashboard")
            .expect("transition expected");
        // Deliberately no call to broadcast_dashboard_visibility_change —
        // user-b was offline and never received a live push.

        let entries = sync_log_service::get_entries_since(&db, "ws-1", cursor, "user-b", 50)
            .await
            .expect("get_entries_since for user-b");

        let entry = entries
            .iter()
            .find(|e| e.entity_id == dashboard_id)
            .expect("offline non-owner's delta must include the newly-public dashboard");
        assert!(matches!(entry.action, SyncActionType::Update));
        assert!(
            entry.data.is_some(),
            "delta entry must carry the snapshot so the client can render it: {entry:?}"
        );
    }

    #[tokio::test]
    async fn offline_members_converge_via_delta_after_going_private() {
        let db = test_pool().await;
        let sq = sqlite_pool(&db);
        seed_two_users_one_workspace(sq).await;

        let dashboard_id = create_test_dashboard(&db, "user-a", "ws-1", "Shared Dash").await;
        let collection = create_collection(NewCollectionParams {
            db: &db,
            workspace_id: "ws-1",
            name: "Public Folder",
            description: None,
            color: None,
            is_public: true,
            doc_type: "dashboard",
            created_by: "user-a",
        })
        .await
        .expect("create public collection");
        add_dashboard(&db, &collection.id, &dashboard_id, "ws-1", "user-a", None)
            .await
            .expect("add_dashboard");

        let cursor = sync_log_service::get_latest_sync_id(&db, "ws-1")
            .await
            .expect("cursor");

        remove_dashboard(&db, &collection.id, &dashboard_id, "ws-1")
            .await
            .expect("remove_dashboard")
            .expect("transition expected");
        // No live broadcast fired — both members were offline.

        let non_owner_entries =
            sync_log_service::get_entries_since(&db, "ws-1", cursor, "user-b", 50)
                .await
                .expect("get_entries_since for user-b");
        assert_eq!(
            non_owner_entries.len(),
            1,
            "offline non-owner must see exactly the eviction, nothing else: {non_owner_entries:?}"
        );
        assert!(matches!(non_owner_entries[0].action, SyncActionType::Delete));
        assert_eq!(non_owner_entries[0].entity_id, dashboard_id);

        let owner_entries = sync_log_service::get_entries_since(&db, "ws-1", cursor, "user-a", 50)
            .await
            .expect("get_entries_since for user-a");
        assert_eq!(
            owner_entries.len(),
            2,
            "offline owner's delta must contain both the eviction row and the \
             follow-up snapshot that restores it: {owner_entries:?}"
        );
        assert!(matches!(owner_entries[0].action, SyncActionType::Delete));
        assert!(matches!(owner_entries[1].action, SyncActionType::Update));
        assert!(
            owner_entries[1].data.is_some(),
            "owner's restoring update must carry the fresh snapshot: {owner_entries:?}"
        );
        assert!(
            owner_entries[0].sync_id < owner_entries[1].sync_id,
            "Delete must be applied before Update, or the owner ends up evicted too"
        );
    }

    // ─── Live broadcast — content sync (KYO-245) ─────────────────────────────
    //
    // `websocket::helpers::broadcast_dashboard_sync` (a content edit, not a
    // visibility transition — see the KYO-238 tests above for that) had the
    // same defect KYO-218 fixed for watches: a DB error and a genuinely
    // absent dashboard both collapsed to `data: None` in `Insert`/`Update`
    // sync actions, which is the wire protocol's `Delete` signal. Unlike the
    // watch case this fans out workspace-wide for public docs, and its
    // `entity_type` fallback meant a knowledge doc could be broadcast
    // mislabeled `dashboard` on the same failure path.

    async fn create_test_knowledge_doc(
        db: &DbPool,
        owner: &str,
        workspace_id: &str,
        title: &str,
    ) -> String {
        dashboard_service::create_dashboard(
            db,
            owner,
            workspace_id,
            title,
            "# knowledge content",
            kyomi_core::models::DocType::Knowledge,
            None,
        )
        .await
        .expect("create knowledge doc")
    }

    /// KYO-245 regression guard: an Upsert broadcast for a dashboard that
    /// exists must carry the full payload — `data` must be non-null and
    /// `entity_type` must be resolved from the real snapshot.
    #[tokio::test]
    async fn broadcast_dashboard_sync_upsert_includes_full_payload() {
        let db = test_pool().await;
        let sq = sqlite_pool(&db);
        seed_two_users_one_workspace(sq).await;

        let dashboard_id = create_test_dashboard(&db, "user-a", "ws-1", "A's Dashboard").await;

        let manager = crate::websocket::WebSocketManager::new(None, db.clone());
        let (_conn_a, mut rx_a) = manager.connect("user-a").expect("connect user-a");
        rx_a.try_recv().expect("heartbeat for user-a");

        crate::websocket::helpers::broadcast_dashboard_sync(
            &db,
            &manager,
            &dashboard_id,
            "ws-1",
            SyncActionType::Insert,
            "user-a",
        )
        .await;

        let msg: serde_json::Value = serde_json::from_str(
            &rx_a
                .try_recv()
                .expect("owner should receive the sync_action broadcast"),
        )
        .expect("valid JSON");
        assert_eq!(msg["data"]["action"], "insert");
        assert_eq!(msg["data"]["entity_type"], entity_types::DASHBOARD);
        assert!(
            !msg["data"]["data"].is_null(),
            "Upsert broadcast for an existing dashboard must not have a null payload: {msg}"
        );
    }

    /// KYO-245 regression guard: an Upsert broadcast for a dashboard that
    /// does NOT exist (deleted between the mutation and the broadcast, or a
    /// bad ID) must send no message at all rather than an Upsert with
    /// `data: None` — a payload-less Upsert is wire-indistinguishable from a
    /// Delete, and this broadcast fans out to the whole workspace for
    /// public docs.
    ///
    /// This is the load-bearing test: against the pre-fix code, the missing
    /// snapshot still produced a broadcast (with `data: null`), so this
    /// assertion fails without the fix.
    #[tokio::test]
    async fn broadcast_dashboard_sync_upsert_for_missing_dashboard_sends_nothing() {
        let db = test_pool().await;
        let sq = sqlite_pool(&db);
        seed_two_users_one_workspace(sq).await;

        let manager = crate::websocket::WebSocketManager::new(None, db.clone());
        let (_conn_a, mut rx_a) = manager.connect("user-a").expect("connect user-a");
        rx_a.try_recv().expect("heartbeat for user-a");

        crate::websocket::helpers::broadcast_dashboard_sync(
            &db,
            &manager,
            "dashboard-does-not-exist",
            "ws-1",
            SyncActionType::Insert,
            "user-a",
        )
        .await;

        let result_a = rx_a.try_recv();
        assert!(
            result_a.is_err(),
            "no message should be sent for an Upsert of a nonexistent dashboard, got: {result_a:?}"
        );
    }

    /// Regression guard for the fix itself: a Delete broadcast must still
    /// carry `data: None`. The fix must only suppress the broadcast for
    /// non-Delete actions when the fetch fails or the row is missing — it
    /// must not touch the Delete branch.
    #[tokio::test]
    async fn broadcast_dashboard_sync_delete_still_sends_null_payload() {
        let db = test_pool().await;
        let sq = sqlite_pool(&db);
        seed_two_users_one_workspace(sq).await;

        let dashboard_id =
            create_test_dashboard(&db, "user-a", "ws-1", "A's Dashboard To Delete").await;

        let manager = crate::websocket::WebSocketManager::new(None, db.clone());
        let (_conn_a, mut rx_a) = manager.connect("user-a").expect("connect user-a");
        rx_a.try_recv().expect("heartbeat for user-a");

        crate::websocket::helpers::broadcast_dashboard_sync(
            &db,
            &manager,
            &dashboard_id,
            "ws-1",
            SyncActionType::Delete,
            "user-a",
        )
        .await;

        let msg: serde_json::Value = serde_json::from_str(
            &rx_a
                .try_recv()
                .expect("owner should receive the Delete sync_action broadcast"),
        )
        .expect("valid JSON");
        assert_eq!(msg["data"]["action"], "delete");
        assert_eq!(
            msg["data"]["data"],
            serde_json::Value::Null,
            "Delete broadcasts must still carry data: None: {msg}"
        );
    }

    /// KYO-245's misrouting-specific guard: if a knowledge doc's row
    /// disappears between the mutation and the broadcast (simulating the
    /// same "unavailable snapshot" condition as the DB-error path, since
    /// neither is distinguishable from the fetch's caller side), the
    /// broadcast must be suppressed entirely — never sent with a
    /// defaulted `entity_type: dashboard`, which would misroute a
    /// knowledge doc to the dashboard cache store on every connected
    /// client.
    ///
    /// This is the load-bearing test for the misrouting defect: against
    /// the pre-fix code, this still sent a message with
    /// `entity_type: "dashboard"` for what was a knowledge document.
    #[tokio::test]
    async fn broadcast_dashboard_sync_missing_knowledge_doc_is_never_mislabeled_dashboard() {
        let db = test_pool().await;
        let sq = sqlite_pool(&db);
        seed_two_users_one_workspace(sq).await;

        let doc_id = create_test_knowledge_doc(&db, "user-a", "ws-1", "A's Knowledge Doc").await;

        // Simulate the row disappearing out from under the broadcast (a
        // concurrent delete, or the same "unavailable" condition a DB error
        // would produce) — fetch_dashboard_snapshot can't tell these apart
        // from its caller's side, which is exactly the bug this ticket
        // fixes.
        sqlx::query("DELETE FROM dashboards WHERE dashboard_id = $1")
            .bind(&doc_id)
            .execute(sq)
            .await
            .expect("simulate concurrent delete of the knowledge doc row");

        let manager = crate::websocket::WebSocketManager::new(None, db.clone());
        let (_conn_a, mut rx_a) = manager.connect("user-a").expect("connect user-a");
        rx_a.try_recv().expect("heartbeat for user-a");

        crate::websocket::helpers::broadcast_dashboard_sync(
            &db,
            &manager,
            &doc_id,
            "ws-1",
            SyncActionType::Insert,
            "user-a",
        )
        .await;

        let result_a = rx_a.try_recv();
        assert!(
            result_a.is_err(),
            "no message — let alone one mislabeled entity_type: dashboard — should be sent for \
             a knowledge doc whose snapshot is unavailable, got: {result_a:?}"
        );
    }

    /// KYO-245: `create_dashboard`'s `write_sync_entry` call must produce a
    /// `sync_log` row with a non-null `data` payload when the snapshot
    /// fetches without error — locking in the persisted-log half of the fix
    /// (the live-broadcast half is covered by the tests above). The
    /// durable path is where a null-payload row is worse than the live one:
    /// it replays to every future delta consumer instead of failing once.
    #[tokio::test]
    async fn create_dashboard_sync_log_entry_has_non_null_data() {
        let db = test_pool().await;
        let sq = sqlite_pool(&db);
        seed_two_users_one_workspace(sq).await;

        let dashboard_id =
            create_test_dashboard(&db, "user-a", "ws-1", "A's Sync Log Dashboard").await;

        let entries = sync_log_service::get_entries_since(&db, "ws-1", 0, "user-a", 50)
            .await
            .expect("get_entries_since");

        let entry = entries
            .iter()
            .find(|e| e.entity_id == dashboard_id)
            .expect("sync_log should contain an entry for the created dashboard");

        assert!(
            entry.data.is_some(),
            "sync_log row for a dashboard that fetched successfully must have non-null data: {entry:?}"
        );
    }
}
