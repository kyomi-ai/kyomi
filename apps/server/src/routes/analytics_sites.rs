// SPDX-License-Identifier: AGPL-3.0-or-later

//! Analytics site REST endpoints.
//!
//! Manages multi-tenant analytics sites — each site gets a signed key
//! that the collector can verify statelessly (no DB lookup).
//!
//! All business logic is delegated to `kyomi_auth::analytics_site_service`.
//!
//! ## Endpoints
//!
//! - `POST   /`       — create analytics site
//! - `GET    /`       — list analytics sites
//! - `GET    /{id}`   — get analytics site
//! - `PUT    /{id}`   — update analytics site
//! - `DELETE /{id}`   — delete analytics site

use axum::{
    extract::{Path, State},
    routing::get,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use kyomi_agent::catalog::indexing_service::CatalogIndexingService;
use kyomi_auth::{analytics_quota, analytics_site_service, middleware::AuthUser};
use kyomi_core::enums::WorkspaceRole;

use crate::state::AppState;

// ===========================================================================
// Router
// ===========================================================================

/// Build the `/analytics/sites` router.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/", get(list_sites).post(create_site))
        .route("/{id}", get(get_site).put(update_site).delete(delete_site))
}

/// Build the `/analytics/usage` router.
pub fn usage_routes() -> Router<AppState> {
    Router::new().route("/", get(get_usage))
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

fn require_workspace_admin(user: &AuthUser) -> Result<(), kyomi_core::Error> {
    if !user
        .workspace
        .workspace_roles
        .contains(&WorkspaceRole::WorkspaceAdmin)
    {
        return Err(kyomi_core::Error::Forbidden(
            "Workspace admin access required".into(),
        ));
    }
    Ok(())
}

fn site_to_response(site: &analytics_site_service::AnalyticsSite) -> SiteResponse {
    SiteResponse {
        id: site.id.clone(),
        name: site.name.clone(),
        site_id: site.site_id.clone(),
        allowed_domains: site.allowed_domains.clone(),
        signed_key: site.signed_key.clone(),
        snippet: analytics_site_service::snippet_tag(&site.signed_key),
        datasource_id: site.datasource_id.clone(),
        datasource_slug: site.datasource_slug.clone(),
        created_at: site.created_at.to_rfc3339(),
        updated_at: site.updated_at.to_rfc3339(),
    }
}

// ===========================================================================
// Request / Response Types
// ===========================================================================

#[derive(Deserialize)]
#[cfg_attr(test, derive(Serialize))]
struct CreateSiteRequest {
    name: String,
    allowed_domains: Vec<String>,
    #[serde(default)]
    datasource_slug: Option<String>,
}

#[derive(Deserialize)]
#[cfg_attr(test, derive(Serialize))]
struct UpdateSiteRequest {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    allowed_domains: Option<Vec<String>>,
    #[serde(default)]
    datasource_slug: Option<String>,
}

#[derive(Serialize)]
#[cfg_attr(test, derive(Deserialize))]
struct SiteResponse {
    id: String,
    name: String,
    site_id: String,
    allowed_domains: Vec<String>,
    signed_key: String,
    snippet: String,
    datasource_id: Option<String>,
    datasource_slug: Option<String>,
    created_at: String,
    updated_at: String,
}

// ===========================================================================
// Endpoint Handlers
// ===========================================================================

// ---------------------------------------------------------------------------
// POST / — Create analytics site
// ---------------------------------------------------------------------------

async fn create_site(
    State(state): State<AppState>,
    user: AuthUser,
    Json(request): Json<CreateSiteRequest>,
) -> Result<Json<SiteResponse>, kyomi_core::Error> {
    let workspace_id = get_workspace_id(&user)?;
    require_workspace_admin(&user)?;

    let name = request.name.trim();
    if name.is_empty() || name.len() > 255 {
        return Err(kyomi_core::Error::BadRequest(
            "Site name must be 1-255 characters".into(),
        ));
    }
    if request.allowed_domains.is_empty() {
        return Err(kyomi_core::Error::BadRequest(
            "At least one domain is required".into(),
        ));
    }

    if state.config.analytics_signing_secret.is_empty() {
        return Err(kyomi_core::Error::ServiceUnavailable(
            "Analytics signing secret is not configured".into(),
        ));
    }

    let site = analytics_site_service::create_site(
        &state.db,
        workspace_id,
        name,
        &request.allowed_domains,
        &state.config.analytics_signing_secret,
        request.datasource_slug.as_deref(),
        &state.config.analytics_clickhouse_host,
        state.config.analytics_clickhouse_port,
        &state.config.analytics_clickhouse_password,
        state.config.analytics_clickhouse_secure,
    )
    .await?;

    tracing::info!(
        site_id = %site.site_id,
        workspace_id = %workspace_id,
        "Created analytics site via API"
    );

    // Spawn background quota sync + catalog indexing
    if let Some(ref datasource_id) = site.datasource_id {
        CatalogIndexingService::spawn_analytics_post_create(
            state.db.clone(),
            state.redis.clone(),
            state.encryption_key.clone(),
            state.embedding.clone(),
            workspace_id.to_string(),
            datasource_id.clone(),
            user.workspace.subscription_tier.to_string(),
        );
    }

    Ok(Json(site_to_response(&site)))
}

// ---------------------------------------------------------------------------
// GET / — List analytics sites
// ---------------------------------------------------------------------------

async fn list_sites(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<Vec<SiteResponse>>, kyomi_core::Error> {
    let workspace_id = get_workspace_id(&user)?;
    require_workspace_admin(&user)?;

    let sites = analytics_site_service::list_sites(&state.db, workspace_id).await?;

    Ok(Json(sites.iter().map(site_to_response).collect()))
}

// ---------------------------------------------------------------------------
// GET /{id} — Get analytics site
// ---------------------------------------------------------------------------

async fn get_site(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
) -> Result<Json<SiteResponse>, kyomi_core::Error> {
    let workspace_id = get_workspace_id(&user)?;
    require_workspace_admin(&user)?;

    let site = analytics_site_service::get_site(&state.db, &id, workspace_id)
        .await?
        .ok_or_else(|| kyomi_core::Error::NotFound("Analytics site not found".into()))?;

    Ok(Json(site_to_response(&site)))
}

// ---------------------------------------------------------------------------
// PUT /{id} — Update analytics site
// ---------------------------------------------------------------------------

async fn update_site(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
    Json(request): Json<UpdateSiteRequest>,
) -> Result<Json<SiteResponse>, kyomi_core::Error> {
    let workspace_id = get_workspace_id(&user)?;
    require_workspace_admin(&user)?;

    if request.name.is_none() && request.allowed_domains.is_none() && request.datasource_slug.is_none() {
        return Err(kyomi_core::Error::BadRequest(
            "No updates provided".into(),
        ));
    }

    let trimmed_name: Option<String> = request.name.as_ref().map(|n| n.trim().to_string());

    if let Some(ref name) = trimmed_name {
        if name.is_empty() || name.len() > 255 {
            return Err(kyomi_core::Error::BadRequest(
                "Site name must be 1-255 characters".into(),
            ));
        }
    }

    if let Some(ref domains) = request.allowed_domains {
        if domains.is_empty() {
            return Err(kyomi_core::Error::BadRequest(
                "At least one domain is required".into(),
            ));
        }
    }

    if request.allowed_domains.is_some() && state.config.analytics_signing_secret.is_empty() {
        return Err(kyomi_core::Error::ServiceUnavailable(
            "Analytics signing secret is not configured".into(),
        ));
    }

    let site = analytics_site_service::update_site(
        &state.db,
        &id,
        workspace_id,
        trimmed_name.as_deref(),
        request.allowed_domains.as_deref(),
        &state.config.analytics_signing_secret,
        request.datasource_slug.as_deref(),
    )
    .await?;

    tracing::info!(site_id = %site.site_id, "Updated analytics site via API");

    Ok(Json(site_to_response(&site)))
}

// ---------------------------------------------------------------------------
// DELETE /{id} — Delete analytics site
// ---------------------------------------------------------------------------

async fn delete_site(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
) -> Result<Json<Value>, kyomi_core::Error> {
    let workspace_id = get_workspace_id(&user)?;
    require_workspace_admin(&user)?;

    analytics_site_service::delete_site(
        &state.db,
        &id,
        workspace_id,
        &state.config.analytics_clickhouse_host,
        state.config.analytics_clickhouse_port,
        &state.config.analytics_clickhouse_password,
        state.config.analytics_clickhouse_secure,
    )
    .await?;

    tracing::info!(site_id = %id, "Deleted analytics site via API");

    Ok(Json(json!({
        "success": true,
        "message": "Analytics site deleted",
    })))
}

// ---------------------------------------------------------------------------
// GET /analytics/usage — Get event usage for the workspace
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct UsageResponse {
    period: String,
    events_used: u64,
    events_limit: u64,
    grace_limit: u64,
    usage_percent: f64,
    status: String,
    sites: Vec<SiteUsage>,
}

#[derive(Serialize)]
struct SiteUsage {
    site_id: String,
    name: String,
    events: u64,
}

async fn get_usage(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<UsageResponse>, kyomi_core::Error> {
    let workspace_id = get_workspace_id(&user)?;

    // Get tier config
    let tier_str = user.workspace.subscription_tier.as_ref();
    let configs = analytics_quota::default_tier_configs();
    let config = configs.get(tier_str).ok_or_else(|| {
        kyomi_core::Error::Internal(format!("Unknown subscription tier: {tier_str}"))
    })?;

    // Get usage from Redis — returns 0 when Redis is absent (single-instance mode)
    let events_used = if let Some(mut redis_conn) = state.redis.clone() {
        analytics_quota::get_usage_count(&mut redis_conn, workspace_id)
            .await
            .unwrap_or(0)
    } else {
        0u64
    };

    // Get per-site breakdown from ClickHouse
    let sites_list = analytics_site_service::list_sites(&state.db, workspace_id).await?;
    // Only query sites that have been provisioned with per-site databases
    let site_db_pairs: Vec<(String, String)> = sites_list
        .iter()
        .filter_map(|s| s.clickhouse_database.as_ref().map(|db| (s.site_id.clone(), db.clone())))
        .collect();

    let per_site_counts = analytics_quota::get_per_site_counts_from_clickhouse(
        &state.config.analytics_clickhouse_host,
        state.config.analytics_clickhouse_port,
        &state.config.analytics_clickhouse_password,
        &site_db_pairs,
        state.config.analytics_clickhouse_secure,
    )
    .await
    .unwrap_or_default();

    let sites: Vec<SiteUsage> = sites_list
        .iter()
        .map(|s| SiteUsage {
            site_id: s.site_id.clone(),
            name: s.name.clone(),
            events: per_site_counts.get(&s.site_id).copied().unwrap_or(0),
        })
        .collect();

    let events_limit = config.monthly_event_limit;
    let grace_limit = config.grace_limit();
    let usage_percent = if events_limit > 0 {
        (events_used as f64 / events_limit as f64) * 100.0
    } else {
        0.0
    };

    let status = if events_used >= grace_limit {
        "blocked"
    } else if events_used >= events_limit {
        "exceeded"
    } else if events_used >= events_limit * 80 / 100 {
        "warning"
    } else {
        "ok"
    };

    let period = chrono::Utc::now().format("%Y-%m").to_string();

    Ok(Json(UsageResponse {
        period,
        events_used,
        events_limit,
        grace_limit,
        usage_percent,
        status: status.to_string(),
        sites,
    }))
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // -----------------------------------------------------------------------
    // CreateSiteRequest contract tests
    // -----------------------------------------------------------------------

    #[test]
    fn create_site_request_all_fields() {
        let json = json!({
            "name": "My Website",
            "allowed_domains": ["example.com", "app.example.com"]
        });

        let req: CreateSiteRequest = serde_json::from_value(json).unwrap();
        assert_eq!(req.name, "My Website");
        assert_eq!(req.allowed_domains, vec!["example.com", "app.example.com"]);
    }

    #[test]
    fn create_site_request_fails_without_name() {
        let json = json!({"allowed_domains": ["example.com"]});
        assert!(serde_json::from_value::<CreateSiteRequest>(json).is_err());
    }

    #[test]
    fn create_site_request_fails_without_domains() {
        let json = json!({"name": "My Site"});
        assert!(serde_json::from_value::<CreateSiteRequest>(json).is_err());
    }

    #[test]
    fn create_site_request_round_trip() {
        let json = json!({
            "name": "Test Site",
            "allowed_domains": ["test.com"]
        });

        let req: CreateSiteRequest = serde_json::from_value(json).unwrap();
        let serialized = serde_json::to_value(&req).unwrap();
        let deserialized: CreateSiteRequest = serde_json::from_value(serialized).unwrap();
        assert_eq!(deserialized.name, "Test Site");
        assert_eq!(deserialized.allowed_domains, vec!["test.com"]);
    }

    // -----------------------------------------------------------------------
    // UpdateSiteRequest contract tests
    // -----------------------------------------------------------------------

    #[test]
    fn update_site_request_all_fields() {
        let json = json!({
            "name": "Updated Name",
            "allowed_domains": ["new.example.com"]
        });

        let req: UpdateSiteRequest = serde_json::from_value(json).unwrap();
        assert_eq!(req.name.as_deref(), Some("Updated Name"));
        assert_eq!(
            req.allowed_domains.as_deref(),
            Some(vec!["new.example.com".to_string()].as_slice())
        );
    }

    #[test]
    fn update_site_request_name_only() {
        let json = json!({"name": "New Name"});

        let req: UpdateSiteRequest = serde_json::from_value(json).unwrap();
        assert_eq!(req.name.as_deref(), Some("New Name"));
        assert!(req.allowed_domains.is_none());
    }

    #[test]
    fn update_site_request_domains_only() {
        let json = json!({"allowed_domains": ["a.com", "b.com"]});

        let req: UpdateSiteRequest = serde_json::from_value(json).unwrap();
        assert!(req.name.is_none());
        assert_eq!(
            req.allowed_domains,
            Some(vec!["a.com".to_string(), "b.com".to_string()])
        );
    }

    #[test]
    fn update_site_request_empty_object() {
        let json = json!({});

        let req: UpdateSiteRequest = serde_json::from_value(json).unwrap();
        assert!(req.name.is_none());
        assert!(req.allowed_domains.is_none());
    }

    #[test]
    fn update_site_request_round_trip() {
        let json = json!({"name": "Updated", "allowed_domains": ["x.com"]});

        let req: UpdateSiteRequest = serde_json::from_value(json).unwrap();
        let serialized = serde_json::to_value(&req).unwrap();
        let deserialized: UpdateSiteRequest = serde_json::from_value(serialized).unwrap();
        assert_eq!(deserialized.name.as_deref(), Some("Updated"));
    }

    // -----------------------------------------------------------------------
    // SiteResponse contract tests
    // -----------------------------------------------------------------------

    #[test]
    fn site_response_serializes_all_fields() {
        let response = SiteResponse {
            id: "550e8400-e29b-41d4-a716-446655440000".into(),
            name: "My Website".into(),
            site_id: "abcd1234".into(),
            allowed_domains: vec!["example.com".into(), "app.example.com".into()],
            signed_key: "payload.signature".into(),
            snippet: analytics_site_service::snippet_tag("payload.signature"),
            datasource_id: Some("ds-abc123".into()),
            datasource_slug: Some("my-website-analytics".into()),
            created_at: "2025-01-15T09:00:00+00:00".into(),
            updated_at: "2025-01-16T10:00:00+00:00".into(),
        };

        let json = serde_json::to_value(&response).unwrap();
        assert_eq!(json["id"], "550e8400-e29b-41d4-a716-446655440000");
        assert_eq!(json["name"], "My Website");
        assert_eq!(json["site_id"], "abcd1234");
        assert_eq!(json["allowed_domains"].as_array().unwrap().len(), 2);
        assert_eq!(json["signed_key"], "payload.signature");
        assert!(json["snippet"].as_str().unwrap().contains("data-key"));
        assert_eq!(json["created_at"], "2025-01-15T09:00:00+00:00");
        assert_eq!(json["updated_at"], "2025-01-16T10:00:00+00:00");
    }

    #[test]
    fn site_response_round_trip() {
        let response = SiteResponse {
            id: "test-uuid".into(),
            name: "Test".into(),
            site_id: "ef012345".into(),
            allowed_domains: vec!["test.com".into()],
            signed_key: "k.s".into(),
            snippet: "snippet".into(),
            datasource_id: None,
            datasource_slug: None,
            created_at: "2025-01-15T09:00:00+00:00".into(),
            updated_at: "2025-01-15T09:00:00+00:00".into(),
        };

        let json_str = serde_json::to_string(&response).unwrap();
        let deserialized: SiteResponse = serde_json::from_str(&json_str).unwrap();
        assert_eq!(deserialized.id, "test-uuid");
        assert_eq!(deserialized.name, "Test");
        assert_eq!(deserialized.site_id, "ef012345");
    }

    #[test]
    fn site_response_empty_domains() {
        let response = SiteResponse {
            id: "id".into(),
            name: "Name".into(),
            site_id: "sid".into(),
            allowed_domains: vec![],
            signed_key: "key".into(),
            snippet: "snippet".into(),
            datasource_id: None,
            datasource_slug: None,
            created_at: "2025-01-15T09:00:00+00:00".into(),
            updated_at: "2025-01-15T09:00:00+00:00".into(),
        };

        let json = serde_json::to_value(&response).unwrap();
        assert!(json["allowed_domains"].as_array().unwrap().is_empty());
    }
}
