// SPDX-License-Identifier: AGPL-3.0-or-later

//! Server functions for collection CRUD operations.
//!
//! These replace the REST API calls for collections:
//! - `GET    /collections`                                  -> `list_collections()`
//! - `POST   /collections`                                  -> `create_collection()`
//! - `PATCH  /collections/{collection_id}`                  -> `update_collection()`
//! - `DELETE /collections/{collection_id}`                  -> `delete_collection()`
//! - `POST   /collections/{collection_id}/dashboards`       -> `add_dashboard_to_collection()`
//! - `DELETE /collections/{collection_id}/dashboards/{id}`  -> `remove_dashboard_from_collection()`
//!
//! Calls the same service-layer code as `apps/server/src/routes/collections.rs`.

use leptos::prelude::*;
use serde::{Deserialize, Serialize};

#[cfg(feature = "ssr")]
use super::{AuthenticatedContext, IntoServerFnError};

// ─── Types ──────────────────────────────────────────────────────────────────

/// A dashboard entry within a collection.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CollectionDashboardView {
    pub dashboard_id: String,
    pub title: String,
    pub position: i32,
    pub added_at: String,
}

/// A collection returned from the server.
///
/// Matches the JSON shape returned by `GET /collections` and other endpoints.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CollectionItem {
    pub collection_id: String,
    pub workspace_id: String,
    pub created_by: String,
    pub name: String,
    pub description: Option<String>,
    pub color: Option<String>,
    pub is_public: bool,
    pub dashboards: Vec<CollectionDashboardView>,
    pub dashboard_count: i64,
    pub created_at: String,
    pub updated_at: String,
}

// ─── Conversion helpers (SSR only) ──────────────────────────────────────────

/// Convert a `CollectionWithDashboards` to a `CollectionItem`.
#[cfg(feature = "ssr")]
fn to_collection_item(
    coll: &kyomi_auth::collection_service::CollectionWithDashboards,
) -> CollectionItem {
    CollectionItem {
        collection_id: coll.id.clone(),
        workspace_id: coll.workspace_id.clone(),
        created_by: coll.created_by.clone(),
        name: coll.name.clone(),
        description: coll.description.clone(),
        color: coll.color.clone(),
        is_public: coll.is_public,
        dashboards: coll
            .dashboards
            .iter()
            .map(|d| CollectionDashboardView {
                dashboard_id: d.dashboard_id.clone(),
                title: d.title.clone(),
                position: d.position,
                added_at: d.added_at.to_rfc3339(),
            })
            .collect(),
        dashboard_count: coll.dashboards.len() as i64,
        created_at: coll.created_at.to_rfc3339(),
        updated_at: coll.updated_at.to_rfc3339(),
    }
}

/// Convert a bare `Collection` model to a `CollectionItem`.
#[cfg(feature = "ssr")]
fn bare_to_collection_item(coll: &kyomi_core::models::Collection) -> CollectionItem {
    CollectionItem {
        collection_id: coll.id.clone(),
        workspace_id: coll.workspace_id.clone(),
        created_by: coll.created_by.clone(),
        name: coll.name.clone(),
        description: coll.description.clone(),
        color: coll.color.clone(),
        is_public: coll.is_public,
        dashboards: vec![],
        dashboard_count: 0,
        created_at: coll.created_at.to_rfc3339(),
        updated_at: coll.updated_at.to_rfc3339(),
    }
}

// ─── Server Functions ───────────────────────────────────────────────────────

/// List all collections for the current workspace with dashboard counts.
///
/// When `doc_type` is `Some`, only collections containing at least one
/// document of that type are returned (e.g. `"dashboard"` or `"knowledge"`).
#[server(prefix = "/leptos-api")]
pub async fn list_collections(
    doc_type: Option<String>,
) -> Result<Vec<CollectionItem>, ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;

    let collections = kyomi_auth::collection_service::list_collections(
        ac.db(),
        &ac.ws_id,
        &ac.auth.user_id,
        doc_type.as_deref(),
    )
    .await
    .into_sfn()?;

    Ok(collections.iter().map(to_collection_item).collect())
}

/// Create a new collection in the current workspace.
#[server(prefix = "/leptos-api")]
pub async fn create_collection(
    name: String,
    description: Option<String>,
    color: Option<String>,
    is_public: Option<bool>,
    doc_type: Option<String>,
) -> Result<CollectionItem, ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;

    let dt = doc_type.as_deref().unwrap_or("dashboard");

    let collection = kyomi_auth::collection_service::create_collection(
        kyomi_auth::collection_service::NewCollectionParams {
            db: ac.db(),
            workspace_id: &ac.ws_id,
            name: &name,
            description: description.as_deref(),
            color: color.as_deref(),
            is_public: is_public.unwrap_or(false),
            doc_type: dt,
            created_by: &ac.auth.user_id,
        },
    )
    .await
    .into_sfn()?;

    Ok(bare_to_collection_item(&collection))
}

/// Update an existing collection.
#[server(prefix = "/leptos-api")]
pub async fn update_collection(
    collection_id: String,
    name: Option<String>,
    description: Option<String>,
    color: Option<String>,
    is_public: Option<bool>,
) -> Result<CollectionItem, ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;

    let updates = kyomi_auth::collection_service::CollectionUpdates {
        name,
        description,
        color,
        is_public,
    };

    kyomi_auth::collection_service::update_collection(
        ac.db(),
        &collection_id,
        &ac.ws_id,
        &updates,
    )
    .await
    .into_sfn()?;

    // Re-fetch to get updated state with dashboards
    let collection =
        kyomi_auth::collection_service::get_collection(ac.db(), &collection_id, &ac.ws_id, &ac.auth.user_id)
            .await
            .into_sfn()?;

    let collection = collection.ok_or_else(|| {
        ServerFnError::new("Collection not found")
    })?;

    Ok(to_collection_item(&collection))
}

/// Delete a collection.
#[server(prefix = "/leptos-api")]
pub async fn delete_collection(collection_id: String) -> Result<(), ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;

    kyomi_auth::collection_service::delete_collection(ac.db(), &collection_id, &ac.ws_id)
        .await
        .into_sfn()?;

    Ok(())
}

/// Add a dashboard to a collection.
///
/// Both the collection and dashboard must exist in the same workspace.
#[server(prefix = "/leptos-api")]
pub async fn add_dashboard_to_collection(
    collection_id: String,
    dashboard_id: String,
) -> Result<(), ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;

    kyomi_auth::collection_service::add_dashboard(
        ac.db(),
        &collection_id,
        &dashboard_id,
        &ac.ws_id,
        &ac.auth.user_id,
        None, // position — append to end
    )
    .await
    .into_sfn()?;

    Ok(())
}

/// Remove a dashboard from a collection.
#[server(prefix = "/leptos-api")]
pub async fn remove_dashboard_from_collection(
    collection_id: String,
    dashboard_id: String,
) -> Result<(), ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;

    kyomi_auth::collection_service::remove_dashboard(
        ac.db(),
        &collection_id,
        &dashboard_id,
        &ac.ws_id,
    )
    .await
    .into_sfn()?;

    Ok(())
}
