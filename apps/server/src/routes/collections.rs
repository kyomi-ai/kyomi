// SPDX-License-Identifier: AGPL-3.0-or-later

//! Collection REST endpoints.
//!
//! Wire-compatible with Python's `routers/collections.py`.
//! All business logic is delegated to `kyomi_auth::collection_service`.
//!
//! ## Endpoints
//!
//! - `POST   /`                                  — create collection
//! - `GET    /`                                  — list collections
//! - `GET    /{collection_id}`                   — get collection
//! - `PATCH  /{collection_id}`                   — update collection
//! - `DELETE /{collection_id}`                   — delete collection
//! - `POST   /{collection_id}/dashboards`        — add dashboard
//! - `DELETE /{collection_id}/dashboards/{dashboard_id}` — remove dashboard

use axum::{
    extract::{Path, State},
    routing::get,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use kyomi_auth::{collection_service, middleware::AuthUser};

use crate::state::AppState;

// ===========================================================================
// Router
// ===========================================================================

/// Build the `/collections` router.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/", get(list_collections).post(create_collection))
        .route(
            "/{collection_id}",
            get(get_collection).patch(update_collection).delete(delete_collection),
        )
        .route(
            "/{collection_id}/dashboards",
            axum::routing::post(add_dashboard),
        )
        .route(
            "/{collection_id}/dashboards/{dashboard_id}",
            axum::routing::delete(remove_dashboard),
        )
}

// ===========================================================================
// Helpers
// ===========================================================================

fn get_workspace_id(user: &AuthUser) -> Result<&str, kyomi_core::Error> {
    user.workspace
        .workspace_id
        .as_deref()
        .ok_or_else(|| kyomi_core::Error::BadRequest("Workspace context required".into()))
}

fn collection_to_response(
    coll: &collection_service::CollectionWithDashboards,
) -> CollectionResponse {
    CollectionResponse {
        id: coll.id.clone(),
        workspace_id: coll.workspace_id.clone(),
        name: coll.name.clone(),
        description: coll.description.clone(),
        color: coll.color.clone(),
        is_public: coll.is_public,
        created_at: coll.created_at.to_rfc3339(),
        updated_at: coll.updated_at.to_rfc3339(),
        dashboards: coll
            .dashboards
            .iter()
            .map(|d| DashboardInCollectionResponse {
                dashboard_id: d.dashboard_id.clone(),
                title: d.title.clone(),
                position: d.position,
                added_at: d.added_at.to_rfc3339(),
            })
            .collect(),
    }
}

fn bare_collection_to_response(
    coll: &kyomi_core::models::Collection,
) -> CollectionResponse {
    CollectionResponse {
        id: coll.id.clone(),
        workspace_id: coll.workspace_id.clone(),
        name: coll.name.clone(),
        description: coll.description.clone(),
        color: coll.color.clone(),
        is_public: coll.is_public,
        created_at: coll.created_at.to_rfc3339(),
        updated_at: coll.updated_at.to_rfc3339(),
        dashboards: vec![],
    }
}

// ===========================================================================
// Request / Response Types
// ===========================================================================

#[derive(Deserialize)]
#[cfg_attr(test, derive(Serialize))]
struct CreateCollectionRequest {
    name: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    color: Option<String>,
    #[serde(default)]
    is_public: bool,
}

#[derive(Deserialize)]
#[cfg_attr(test, derive(Serialize))]
struct UpdateCollectionRequest {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    color: Option<String>,
    #[serde(default)]
    is_public: Option<bool>,
}

#[derive(Serialize)]
#[cfg_attr(test, derive(Deserialize))]
struct DashboardInCollectionResponse {
    dashboard_id: String,
    title: String,
    position: i32,
    added_at: String,
}

#[derive(Serialize)]
#[cfg_attr(test, derive(Deserialize))]
struct CollectionResponse {
    id: String,
    workspace_id: String,
    name: String,
    description: Option<String>,
    color: Option<String>,
    is_public: bool,
    created_at: String,
    updated_at: String,
    dashboards: Vec<DashboardInCollectionResponse>,
}

#[derive(Deserialize)]
#[cfg_attr(test, derive(Serialize))]
struct AddDashboardRequest {
    dashboard_id: String,
    #[serde(default)]
    position: Option<i32>,
}

// ===========================================================================
// Endpoint Handlers
// ===========================================================================

// ---------------------------------------------------------------------------
// POST / — Create collection
// ---------------------------------------------------------------------------

async fn create_collection(
    State(state): State<AppState>,
    user: AuthUser,
    Json(request): Json<CreateCollectionRequest>,
) -> Result<Json<CollectionResponse>, kyomi_core::Error> {
    let workspace_id = get_workspace_id(&user)?;

    let collection = collection_service::create_collection(
        &state.db,
        workspace_id,
        &request.name,
        request.description.as_deref(),
        request.color.as_deref(),
        request.is_public,
    )
    .await?;

    Ok(Json(bare_collection_to_response(&collection)))
}

// ---------------------------------------------------------------------------
// GET / — List collections
// ---------------------------------------------------------------------------

async fn list_collections(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<Vec<CollectionResponse>>, kyomi_core::Error> {
    let workspace_id = get_workspace_id(&user)?;

    let collections = collection_service::list_collections(&state.db, workspace_id).await?;

    let response: Vec<CollectionResponse> = collections
        .iter()
        .map(collection_to_response)
        .collect();

    Ok(Json(response))
}

// ---------------------------------------------------------------------------
// GET /{collection_id} — Get collection
// ---------------------------------------------------------------------------

async fn get_collection(
    State(state): State<AppState>,
    user: AuthUser,
    Path(collection_id): Path<String>,
) -> Result<Json<CollectionResponse>, kyomi_core::Error> {
    let workspace_id = get_workspace_id(&user)?;

    let collection =
        collection_service::get_collection(&state.db, &collection_id, workspace_id).await?;

    let collection = collection.ok_or_else(|| {
        kyomi_core::Error::NotFound("Collection not found".into())
    })?;

    Ok(Json(collection_to_response(&collection)))
}

// ---------------------------------------------------------------------------
// PATCH /{collection_id} — Update collection
// ---------------------------------------------------------------------------

async fn update_collection(
    State(state): State<AppState>,
    user: AuthUser,
    Path(collection_id): Path<String>,
    Json(request): Json<UpdateCollectionRequest>,
) -> Result<Json<CollectionResponse>, kyomi_core::Error> {
    let workspace_id = get_workspace_id(&user)?;

    let updates = collection_service::CollectionUpdates {
        name: request.name,
        description: request.description,
        color: request.color,
        is_public: request.is_public,
    };

    collection_service::update_collection(&state.db, &collection_id, workspace_id, &updates)
        .await?;

    // Re-fetch to get updated state with dashboards
    let collection =
        collection_service::get_collection(&state.db, &collection_id, workspace_id).await?;
    let collection = collection.ok_or_else(|| {
        kyomi_core::Error::NotFound("Collection not found".into())
    })?;

    Ok(Json(collection_to_response(&collection)))
}

// ---------------------------------------------------------------------------
// DELETE /{collection_id} — Delete collection
// ---------------------------------------------------------------------------

async fn delete_collection(
    State(state): State<AppState>,
    user: AuthUser,
    Path(collection_id): Path<String>,
) -> Result<Json<Value>, kyomi_core::Error> {
    let workspace_id = get_workspace_id(&user)?;

    collection_service::delete_collection(&state.db, &collection_id, workspace_id).await?;

    Ok(Json(json!({
        "success": true,
        "message": "Collection deleted",
    })))
}

// ---------------------------------------------------------------------------
// POST /{collection_id}/dashboards — Add dashboard to collection
// ---------------------------------------------------------------------------

async fn add_dashboard(
    State(state): State<AppState>,
    user: AuthUser,
    Path(collection_id): Path<String>,
    Json(request): Json<AddDashboardRequest>,
) -> Result<Json<Value>, kyomi_core::Error> {
    let workspace_id = get_workspace_id(&user)?;

    collection_service::add_dashboard(
        &state.db,
        &collection_id,
        &request.dashboard_id,
        workspace_id,
        request.position,
    )
    .await?;

    Ok(Json(json!({
        "success": true,
        "message": "Dashboard added to collection",
    })))
}

// ---------------------------------------------------------------------------
// DELETE /{collection_id}/dashboards/{dashboard_id} — Remove dashboard
// ---------------------------------------------------------------------------

async fn remove_dashboard(
    State(state): State<AppState>,
    user: AuthUser,
    Path((collection_id, dashboard_id)): Path<(String, String)>,
) -> Result<Json<Value>, kyomi_core::Error> {
    let workspace_id = get_workspace_id(&user)?;

    collection_service::remove_dashboard(
        &state.db,
        &collection_id,
        &dashboard_id,
        workspace_id,
    )
    .await?;

    Ok(Json(json!({
        "success": true,
        "message": "Dashboard removed from collection",
    })))
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // -----------------------------------------------------------------------
    // CreateCollectionRequest
    // -----------------------------------------------------------------------

    #[test]
    fn create_collection_request_all_fields() {
        let json = json!({
            "name": "Analytics",
            "description": "Analytics dashboards",
            "color": "#FF5733",
            "is_public": true
        });

        let req: CreateCollectionRequest = serde_json::from_value(json).unwrap();
        assert_eq!(req.name, "Analytics");
        assert_eq!(req.description.as_deref(), Some("Analytics dashboards"));
        assert_eq!(req.color.as_deref(), Some("#FF5733"));
        assert!(req.is_public);
    }

    #[test]
    fn create_collection_request_minimal() {
        let json = json!({"name": "My Collection"});

        let req: CreateCollectionRequest = serde_json::from_value(json).unwrap();
        assert_eq!(req.name, "My Collection");
        assert!(req.description.is_none());
        assert!(req.color.is_none());
        assert!(!req.is_public);
    }

    #[test]
    fn create_collection_request_fails_without_name() {
        let json = json!({"description": "no name"});
        assert!(serde_json::from_value::<CreateCollectionRequest>(json).is_err());
    }

    #[test]
    fn create_collection_request_round_trip() {
        let json = json!({"name": "Test", "color": "#AABBCC"});
        let req: CreateCollectionRequest = serde_json::from_value(json).unwrap();
        let serialized = serde_json::to_value(&req).unwrap();
        let deserialized: CreateCollectionRequest = serde_json::from_value(serialized).unwrap();
        assert_eq!(deserialized.name, "Test");
        assert_eq!(deserialized.color.as_deref(), Some("#AABBCC"));
    }

    // -----------------------------------------------------------------------
    // UpdateCollectionRequest
    // -----------------------------------------------------------------------

    #[test]
    fn update_collection_request_all_fields() {
        let json = json!({
            "name": "Updated",
            "description": "New desc",
            "color": "#000000",
            "is_public": true
        });

        let req: UpdateCollectionRequest = serde_json::from_value(json).unwrap();
        assert_eq!(req.name.as_deref(), Some("Updated"));
        assert_eq!(req.description.as_deref(), Some("New desc"));
        assert_eq!(req.color.as_deref(), Some("#000000"));
        assert_eq!(req.is_public, Some(true));
    }

    #[test]
    fn update_collection_request_empty() {
        let json = json!({});

        let req: UpdateCollectionRequest = serde_json::from_value(json).unwrap();
        assert!(req.name.is_none());
        assert!(req.description.is_none());
        assert!(req.color.is_none());
        assert!(req.is_public.is_none());
    }

    #[test]
    fn update_collection_request_partial() {
        let json = json!({"name": "New Name"});

        let req: UpdateCollectionRequest = serde_json::from_value(json).unwrap();
        assert_eq!(req.name.as_deref(), Some("New Name"));
        assert!(req.description.is_none());
    }

    // -----------------------------------------------------------------------
    // CollectionResponse
    // -----------------------------------------------------------------------

    #[test]
    fn collection_response_serializes_all_fields() {
        let response = CollectionResponse {
            id: "coll-123".into(),
            workspace_id: "ws-xyz".into(),
            name: "Analytics".into(),
            description: Some("Analytics collection".into()),
            color: Some("#FF5733".into()),
            is_public: true,
            created_at: "2025-01-15T09:00:00+00:00".into(),
            updated_at: "2025-01-16T10:00:00+00:00".into(),
            dashboards: vec![DashboardInCollectionResponse {
                dashboard_id: "dash-1".into(),
                title: "Revenue".into(),
                position: 0,
                added_at: "2025-01-15T09:00:00+00:00".into(),
            }],
        };

        let json = serde_json::to_value(&response).unwrap();
        assert_eq!(json["id"], "coll-123");
        assert_eq!(json["name"], "Analytics");
        assert_eq!(json["color"], "#FF5733");
        assert!(json["is_public"].as_bool().unwrap());
        assert_eq!(json["dashboards"].as_array().unwrap().len(), 1);
        assert_eq!(json["dashboards"][0]["dashboard_id"], "dash-1");
        assert_eq!(json["dashboards"][0]["position"], 0);
    }

    #[test]
    fn collection_response_null_optional_fields() {
        let response = CollectionResponse {
            id: "coll-1".into(),
            workspace_id: "ws-1".into(),
            name: "Test".into(),
            description: None,
            color: None,
            is_public: false,
            created_at: "2025-01-15T09:00:00+00:00".into(),
            updated_at: "2025-01-15T09:00:00+00:00".into(),
            dashboards: vec![],
        };

        let json = serde_json::to_value(&response).unwrap();
        assert!(json["description"].is_null());
        assert!(json["color"].is_null());
        assert!(!json["is_public"].as_bool().unwrap());
        assert!(json["dashboards"].as_array().unwrap().is_empty());
    }

    #[test]
    fn collection_response_round_trip() {
        let response = CollectionResponse {
            id: "c1".into(),
            workspace_id: "w1".into(),
            name: "Test".into(),
            description: None,
            color: None,
            is_public: false,
            created_at: "2025-01-15T09:00:00+00:00".into(),
            updated_at: "2025-01-15T09:00:00+00:00".into(),
            dashboards: vec![],
        };

        let json_str = serde_json::to_string(&response).unwrap();
        let deserialized: CollectionResponse = serde_json::from_str(&json_str).unwrap();
        assert_eq!(deserialized.id, "c1");
        assert_eq!(deserialized.name, "Test");
    }

    // -----------------------------------------------------------------------
    // AddDashboardRequest
    // -----------------------------------------------------------------------

    #[test]
    fn add_dashboard_request_with_position() {
        let json = json!({"dashboard_id": "dash-123", "position": 5});

        let req: AddDashboardRequest = serde_json::from_value(json).unwrap();
        assert_eq!(req.dashboard_id, "dash-123");
        assert_eq!(req.position, Some(5));
    }

    #[test]
    fn add_dashboard_request_without_position() {
        let json = json!({"dashboard_id": "dash-456"});

        let req: AddDashboardRequest = serde_json::from_value(json).unwrap();
        assert_eq!(req.dashboard_id, "dash-456");
        assert!(req.position.is_none());
    }

    #[test]
    fn add_dashboard_request_fails_without_dashboard_id() {
        let json = json!({"position": 0});
        assert!(serde_json::from_value::<AddDashboardRequest>(json).is_err());
    }

    #[test]
    fn add_dashboard_request_round_trip() {
        let json = json!({"dashboard_id": "d1", "position": 2});
        let req: AddDashboardRequest = serde_json::from_value(json).unwrap();
        let serialized = serde_json::to_value(&req).unwrap();
        let deserialized: AddDashboardRequest = serde_json::from_value(serialized).unwrap();
        assert_eq!(deserialized.dashboard_id, "d1");
        assert_eq!(deserialized.position, Some(2));
    }

    // -----------------------------------------------------------------------
    // DashboardInCollectionResponse
    // -----------------------------------------------------------------------

    #[test]
    fn dashboard_in_collection_response_serializes() {
        let response = DashboardInCollectionResponse {
            dashboard_id: "dash-1".into(),
            title: "Revenue Dashboard".into(),
            position: 0,
            added_at: "2025-01-15T09:00:00+00:00".into(),
        };

        let json = serde_json::to_value(&response).unwrap();
        assert_eq!(json["dashboard_id"], "dash-1");
        assert_eq!(json["title"], "Revenue Dashboard");
        assert_eq!(json["position"], 0);
    }
}
