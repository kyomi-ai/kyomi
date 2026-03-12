// SPDX-License-Identifier: AGPL-3.0-or-later

//! Learning admin REST endpoints.
//!
//! Wire-compatible with Python's workspace learning endpoints:
//! - `GET    /{workspace_id}/learnings`
//! - `PATCH  /{workspace_id}/learnings/{learning_id}`
//! - `DELETE /{workspace_id}/learnings/{learning_id}`
//!
//! Permission model:
//! - Workspace admins: can manage ALL learnings
//! - Regular users: can only manage their own user-scoped learnings

use axum::{
    extract::{Path, Query, State},
    routing::get,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use kyomi_auth::{learning_service, middleware::AuthUser};

use crate::state::AppState;

// ===========================================================================
// Router
// ===========================================================================

/// Build the `/workspaces/{workspace_id}/learnings` router.
///
/// Nested under workspaces to match the Python path structure.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/{workspace_id}/learnings", get(list_learnings))
        .route(
            "/{workspace_id}/learnings/{learning_id}",
            axum::routing::patch(update_learning).delete(delete_learning),
        )
}

// ===========================================================================
// Helpers
// ===========================================================================

/// Verify workspace access: user must belong to the requested workspace.
fn verify_workspace_access(
    user: &AuthUser,
    workspace_id: &str,
) -> Result<(), kyomi_core::Error> {
    let user_workspace = user
        .workspace
        .workspace_id
        .as_deref()
        .ok_or_else(|| kyomi_core::Error::BadRequest("Workspace context required".into()))?;

    if user_workspace != workspace_id {
        return Err(kyomi_core::Error::Forbidden("Access denied".into()));
    }
    Ok(())
}

/// Check if user is a workspace admin.
fn is_workspace_admin(user: &AuthUser) -> bool {
    user.workspace
        .workspace_roles
        .iter()
        .any(|r| *r == kyomi_core::WorkspaceRole::WorkspaceAdmin)
}

/// Check learning management permission.
///
/// Admins can manage all learnings. Regular users can only manage their
/// own user-scoped learnings.
fn check_learning_permission(
    user: &AuthUser,
    learning_scope: &str,
    learning_owner: Option<&str>,
) -> Result<(), kyomi_core::Error> {
    if is_workspace_admin(user) {
        return Ok(());
    }

    let is_user_learning = learning_scope == "user";
    let is_owner = learning_owner == Some(&user.user_id);

    if is_user_learning && is_owner {
        return Ok(());
    }

    Err(kyomi_core::Error::Forbidden(
        "Only workspace admins can manage workspace learnings. \
         You can only manage your own personal learnings."
            .into(),
    ))
}

/// Build datasource_config_id -> slug lookup map.
async fn build_ds_slug_map(
    db: &kyomi_core::DbPool,
    workspace_id: &str,
) -> Result<std::collections::HashMap<String, String>, kyomi_core::Error> {
    #[derive(sqlx::FromRow)]
    struct DsSlugRow { id: String, slug: String }

    let rows = kyomi_core::db_fetch_all!(
        db, DsSlugRow,
        "SELECT id, slug FROM datasource_configs WHERE workspace_id = $1",
        workspace_id
    )
    .map_err(|e| kyomi_core::Error::Internal(format!("failed to fetch datasources: {e}")))?;

    let map = rows
        .iter()
        .map(|row| (row.id.clone(), row.slug.clone()))
        .collect();

    Ok(map)
}

/// Convert a learning record to a response, resolving datasource ID to slug.
fn learning_to_response(
    learning: &learning_service::LearningRecord,
    ds_slug_map: &std::collections::HashMap<String, String>,
) -> LearningResponse {
    let datasource_slug = learning
        .datasource_config_id
        .as_ref()
        .and_then(|id| ds_slug_map.get(id))
        .cloned();

    LearningResponse {
        learning_id: learning.learning_id.clone(),
        insight: learning.insight.clone(),
        context: learning.context.clone(),
        enabled: learning.enabled,
        scope: learning.scope.clone(),
        learning_type: learning.learning_type.clone(),
        times_used: learning.times_used,
        last_used_at: learning.last_used_at.map(|dt| dt.to_rfc3339()),
        created_at: learning.created_at.to_rfc3339(),
        learned_from_user: learning
            .learned_from_user
            .clone()
            .unwrap_or_else(|| "unknown".into()),
        learned_from_session: learning
            .learned_from_session
            .clone()
            .unwrap_or_else(|| "unknown".into()),
        datasource_slug,
        reference_queries: learning.reference_queries.clone(),
    }
}

// ===========================================================================
// Request / Response Types
// ===========================================================================

#[derive(Serialize)]
#[cfg_attr(test, derive(Deserialize))]
struct LearningResponse {
    learning_id: String,
    insight: String,
    context: Option<String>,
    enabled: bool,
    scope: String,
    learning_type: String,
    times_used: i32,
    last_used_at: Option<String>,
    created_at: String,
    learned_from_user: String,
    learned_from_session: String,
    datasource_slug: Option<String>,
    reference_queries: Option<Value>,
}

#[derive(Serialize)]
#[cfg_attr(test, derive(Deserialize))]
struct LearningsListResponse {
    items: Vec<LearningResponse>,
    total: i64,
    offset: i64,
    limit: i64,
    has_more: bool,
}

#[derive(Deserialize)]
#[cfg_attr(test, derive(Serialize))]
struct LearningListParams {
    #[serde(default)]
    offset: i64,
    #[serde(default = "default_limit")]
    limit: i64,
    #[serde(default)]
    search: Option<String>,
    #[serde(default)]
    scope: Option<String>,
    #[serde(default)]
    datasource: Option<String>,
    #[serde(default)]
    enabled_only: bool,
}

fn default_limit() -> i64 {
    50
}

#[derive(Deserialize)]
#[cfg_attr(test, derive(Serialize))]
struct LearningUpdateRequest {
    #[serde(default)]
    enabled: Option<bool>,
    #[serde(default)]
    insight: Option<String>,
    #[serde(default)]
    context: Option<String>,
    #[serde(default)]
    learning_type: Option<String>,
    #[serde(default)]
    datasource_slug: Option<String>,
    #[serde(default)]
    reference_queries: Option<Value>,
}

// ===========================================================================
// Endpoint Handlers
// ===========================================================================

// ---------------------------------------------------------------------------
// GET /{workspace_id}/learnings — List learnings
// ---------------------------------------------------------------------------

async fn list_learnings(
    State(state): State<AppState>,
    user: AuthUser,
    Path(workspace_id): Path<String>,
    Query(params): Query<LearningListParams>,
) -> Result<Json<LearningsListResponse>, kyomi_core::Error> {
    verify_workspace_access(&user, &workspace_id)?;

    let limit = params.limit.clamp(1, 100);
    let offset = params.offset.max(0);

    let (learnings, total) = learning_service::get_all_learnings(
        &state.db,
        &workspace_id,
        offset,
        limit,
        params.search.as_deref(),
        params.scope.as_deref(),
        params.datasource.as_deref(),
        params.enabled_only,
    )
    .await?;

    let ds_slug_map = build_ds_slug_map(&state.db, &workspace_id).await?;

    let items: Vec<LearningResponse> = learnings
        .iter()
        .map(|l| learning_to_response(l, &ds_slug_map))
        .collect();

    let has_more = (offset + items.len() as i64) < total;

    Ok(Json(LearningsListResponse {
        items,
        total,
        offset,
        limit,
        has_more,
    }))
}

// ---------------------------------------------------------------------------
// PATCH /{workspace_id}/learnings/{learning_id} — Update learning
// ---------------------------------------------------------------------------

async fn update_learning(
    State(state): State<AppState>,
    user: AuthUser,
    Path((workspace_id, learning_id)): Path<(String, String)>,
    Json(request): Json<LearningUpdateRequest>,
) -> Result<Json<Value>, kyomi_core::Error> {
    verify_workspace_access(&user, &workspace_id)?;

    // Fetch the single learning to check permissions
    let learning = learning_service::get_learning_by_id(&state.db, &learning_id, &workspace_id)
        .await?
        .ok_or_else(|| kyomi_core::Error::NotFound("Learning not found".into()))?;

    check_learning_permission(&user, &learning.scope, learning.learned_from_user.as_deref())?;

    // Validate learning_type if provided
    if let Some(ref lt) = request.learning_type
        && !learning_service::VALID_LEARNING_TYPES.contains(&lt.as_str())
    {
        return Err(kyomi_core::Error::BadRequest(format!(
            "Invalid learning_type: {lt}. Must be one of: {}",
            learning_service::VALID_LEARNING_TYPES.join(", ")
        )));
    }

    // Resolve datasource_slug to config_id if provided
    let datasource_config_id = if let Some(ref slug) = request.datasource_slug {
        if slug.is_empty() {
            // Empty string means clear (set to global)
            Some(String::new())
        } else {
            // Resolve slug to ID
            #[derive(sqlx::FromRow)]
            struct DsIdRow { id: String }

            let row = kyomi_core::db_fetch_optional!(
                &state.db, DsIdRow,
                "SELECT id FROM datasource_configs WHERE workspace_id = $1 AND slug = $2",
                &workspace_id,
                slug
            )
            .map_err(|e| {
                kyomi_core::Error::Internal(format!("failed to resolve datasource: {e}"))
            })?;

            match row {
                Some(r) => Some(r.id),
                None => {
                    return Err(kyomi_core::Error::BadRequest(format!(
                        "Datasource '{slug}' not found"
                    )));
                }
            }
        }
    } else {
        None
    };

    let updates = learning_service::LearningUpdates {
        insight: request.insight,
        context: request.context,
        enabled: request.enabled,
        datasource_config_id,
        learning_type: request.learning_type,
        reference_queries: request.reference_queries,
        structured_metadata: None,
    };

    // Check if there are actual updates
    let has_updates = updates.insight.is_some()
        || updates.context.is_some()
        || updates.enabled.is_some()
        || updates.datasource_config_id.is_some()
        || updates.learning_type.is_some()
        || updates.reference_queries.is_some();

    if !has_updates {
        return Err(kyomi_core::Error::BadRequest(
            "No fields to update".into(),
        ));
    }

    let enabled_change = updates.enabled;
    let insight_changed = updates.insight.is_some();

    learning_service::update_learning(
        &state.db,
        state.embedding.wait_ready().await?,
        &learning_id,
        &workspace_id,
        &updates,
    )
    .await?;

    // Graph sync: re-populate when insight changes (text/embedding updated),
    // or handle enabled state changes.
    if enabled_change == Some(true) || insight_changed {
        // Re-enable OR insight text changed — rebuild the embedding + references
        repopulate_learning_graph(&state, &workspace_id, &learning_id).await;
    }
    // No graph cleanup needed on disable — cascade deletes handle this

    Ok(Json(json!({"success": true})))
}

// ---------------------------------------------------------------------------
// DELETE /{workspace_id}/learnings/{learning_id} — Delete learning
// ---------------------------------------------------------------------------

async fn delete_learning(
    State(state): State<AppState>,
    user: AuthUser,
    Path((workspace_id, learning_id)): Path<(String, String)>,
) -> Result<Json<Value>, kyomi_core::Error> {
    verify_workspace_access(&user, &workspace_id)?;

    // Fetch the single learning to check permissions
    let learning = learning_service::get_learning_by_id(&state.db, &learning_id, &workspace_id)
        .await?
        .ok_or_else(|| kyomi_core::Error::NotFound("Learning not found".into()))?;

    check_learning_permission(&user, &learning.scope, learning.learned_from_user.as_deref())?;

    learning_service::delete_learning(&state.db, &learning_id, &workspace_id).await?;

    // No graph cleanup needed — cascade deletes handle this

    Ok(Json(json!({"success": true})))
}

// ===========================================================================
// Knowledge embedding helpers (fire-and-forget)
// ===========================================================================

/// Re-populate embedding + references for a learning after re-enable.
async fn repopulate_learning_graph(state: &AppState, workspace_id: &str, learning_id: &str) {
    let embed = match state.embedding.wait_ready().await {
        Ok(e) => e,
        Err(e) => {
            tracing::warn!(error = %e, "Embedding service not available for learning repopulation");
            return;
        }
    };
    match kyomi_knowledge::populate::populate_learning_embedding(&state.db, embed, learning_id)
        .await
    {
        Ok(()) => tracing::debug!(learning_id, "Learning embedding re-populated"),
        Err(e) => tracing::warn!(error = %e, "Learning embedding repopulation failed (non-fatal)"),
    }
    // Materialize references
    if let Err(e) = kyomi_knowledge::references::materialize_learning_references(
        &state.db, learning_id, workspace_id, None,
    )
    .await
    {
        tracing::warn!(error = %e, "Learning reference materialization failed (non-fatal)");
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // -----------------------------------------------------------------------
    // LearningResponse
    // -----------------------------------------------------------------------

    #[test]
    fn learning_response_serializes_all_fields() {
        let response = LearningResponse {
            learning_id: "learn-abc".into(),
            insight: "Revenue is in sales.transactions".into(),
            context: Some("User asked about revenue".into()),
            enabled: true,
            scope: "workspace".into(),
            learning_type: "navigation".into(),
            times_used: 5,
            last_used_at: Some("2025-01-15T09:00:00+00:00".into()),
            created_at: "2025-01-10T00:00:00+00:00".into(),
            learned_from_user: "user-abc".into(),
            learned_from_session: "sess-xyz".into(),
            datasource_slug: Some("production-postgres".into()),
            reference_queries: Some(json!([{"sql": "SELECT * FROM sales", "comment": "test"}])),
        };

        let json = serde_json::to_value(&response).unwrap();
        assert_eq!(json["learning_id"], "learn-abc");
        assert_eq!(json["insight"], "Revenue is in sales.transactions");
        assert_eq!(json["scope"], "workspace");
        assert_eq!(json["learning_type"], "navigation");
        assert_eq!(json["times_used"], 5);
        assert_eq!(json["datasource_slug"], "production-postgres");
        assert!(json["reference_queries"].is_array());
    }

    #[test]
    fn learning_response_null_optional_fields() {
        let response = LearningResponse {
            learning_id: "learn-1".into(),
            insight: "Test".into(),
            context: None,
            enabled: false,
            scope: "user".into(),
            learning_type: "preference".into(),
            times_used: 0,
            last_used_at: None,
            created_at: "2025-01-10T00:00:00+00:00".into(),
            learned_from_user: "user-1".into(),
            learned_from_session: "sess-1".into(),
            datasource_slug: None,
            reference_queries: None,
        };

        let json = serde_json::to_value(&response).unwrap();
        assert!(json["context"].is_null());
        assert!(json["last_used_at"].is_null());
        assert!(json["datasource_slug"].is_null());
        assert!(json["reference_queries"].is_null());
    }

    #[test]
    fn learning_response_round_trip() {
        let response = LearningResponse {
            learning_id: "l1".into(),
            insight: "test".into(),
            context: None,
            enabled: true,
            scope: "workspace".into(),
            learning_type: "metric".into(),
            times_used: 3,
            last_used_at: None,
            created_at: "2025-01-10T00:00:00+00:00".into(),
            learned_from_user: "u1".into(),
            learned_from_session: "s1".into(),
            datasource_slug: None,
            reference_queries: None,
        };

        let json_str = serde_json::to_string(&response).unwrap();
        let deserialized: LearningResponse = serde_json::from_str(&json_str).unwrap();
        assert_eq!(deserialized.learning_id, "l1");
        assert_eq!(deserialized.learning_type, "metric");
    }

    // -----------------------------------------------------------------------
    // LearningsListResponse
    // -----------------------------------------------------------------------

    #[test]
    fn learnings_list_response_serializes() {
        let response = LearningsListResponse {
            items: vec![],
            total: 42,
            offset: 0,
            limit: 50,
            has_more: false,
        };

        let json = serde_json::to_value(&response).unwrap();
        assert_eq!(json["total"], 42);
        assert_eq!(json["offset"], 0);
        assert_eq!(json["limit"], 50);
        assert!(!json["has_more"].as_bool().unwrap());
        assert!(json["items"].is_array());
    }

    #[test]
    fn learnings_list_response_has_more_true() {
        let response = LearningsListResponse {
            items: vec![],
            total: 100,
            offset: 0,
            limit: 50,
            has_more: true,
        };

        let json = serde_json::to_value(&response).unwrap();
        assert!(json["has_more"].as_bool().unwrap());
    }

    // -----------------------------------------------------------------------
    // LearningListParams
    // -----------------------------------------------------------------------

    #[test]
    fn learning_list_params_defaults() {
        let json = json!({});
        let params: LearningListParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.offset, 0);
        assert_eq!(params.limit, 50);
        assert!(params.search.is_none());
        assert!(params.scope.is_none());
        assert!(params.datasource.is_none());
        assert!(!params.enabled_only);
    }

    #[test]
    fn learning_list_params_custom_values() {
        let json = json!({
            "offset": 10,
            "limit": 25,
            "search": "revenue",
            "scope": "workspace",
            "datasource": "prod-pg",
            "enabled_only": true
        });

        let params: LearningListParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.offset, 10);
        assert_eq!(params.limit, 25);
        assert_eq!(params.search.as_deref(), Some("revenue"));
        assert_eq!(params.scope.as_deref(), Some("workspace"));
        assert_eq!(params.datasource.as_deref(), Some("prod-pg"));
        assert!(params.enabled_only);
    }

    // -----------------------------------------------------------------------
    // LearningUpdateRequest
    // -----------------------------------------------------------------------

    #[test]
    fn learning_update_request_all_fields() {
        let json = json!({
            "enabled": false,
            "insight": "Updated insight text",
            "context": "New context",
            "learning_type": "metric",
            "datasource_slug": "production-postgres",
            "reference_queries": [{"sql": "SELECT 1", "comment": "test"}]
        });

        let req: LearningUpdateRequest = serde_json::from_value(json).unwrap();
        assert_eq!(req.enabled, Some(false));
        assert_eq!(req.insight.as_deref(), Some("Updated insight text"));
        assert_eq!(req.context.as_deref(), Some("New context"));
        assert_eq!(req.learning_type.as_deref(), Some("metric"));
        assert_eq!(req.datasource_slug.as_deref(), Some("production-postgres"));
        assert!(req.reference_queries.is_some());
    }

    #[test]
    fn learning_update_request_empty() {
        let json = json!({});

        let req: LearningUpdateRequest = serde_json::from_value(json).unwrap();
        assert!(req.enabled.is_none());
        assert!(req.insight.is_none());
        assert!(req.context.is_none());
        assert!(req.learning_type.is_none());
        assert!(req.datasource_slug.is_none());
        assert!(req.reference_queries.is_none());
    }

    #[test]
    fn learning_update_request_toggle_only() {
        let json = json!({"enabled": true});

        let req: LearningUpdateRequest = serde_json::from_value(json).unwrap();
        assert_eq!(req.enabled, Some(true));
        assert!(req.insight.is_none());
    }

    #[test]
    fn learning_update_request_clear_datasource() {
        let json = json!({"datasource_slug": ""});

        let req: LearningUpdateRequest = serde_json::from_value(json).unwrap();
        assert_eq!(req.datasource_slug.as_deref(), Some(""));
    }

    #[test]
    fn learning_update_request_round_trip() {
        let json = json!({"insight": "Test", "enabled": false});
        let req: LearningUpdateRequest = serde_json::from_value(json).unwrap();
        let serialized = serde_json::to_value(&req).unwrap();
        let deserialized: LearningUpdateRequest = serde_json::from_value(serialized).unwrap();
        assert_eq!(deserialized.insight.as_deref(), Some("Test"));
        assert_eq!(deserialized.enabled, Some(false));
    }

    // -----------------------------------------------------------------------
    // Default function tests
    // -----------------------------------------------------------------------

    #[test]
    fn default_limit_is_50() {
        assert_eq!(default_limit(), 50);
    }
}
