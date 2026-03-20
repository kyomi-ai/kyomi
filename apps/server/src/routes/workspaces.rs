// SPDX-License-Identifier: AGPL-3.0-or-later

//! Workspace management endpoints.
//!
//! Wire-compatible with Python's `routers/workspaces.py` (lines 104–1329).
//! Endpoints after line 1329 (members, invitations, ownership) are Phase 4D.
//!
//! Auth patterns:
//! - `AuthUser` extractor → any authenticated user
//! - `require_workspace_admin()` → workspace_admin role check

use axum::{
    extract::{Path, State},
    routing::{delete, get, patch, post},
    Json, Router,
};
use serde::Deserialize;

use kyomi_auth::{
    datasource_service, email_service::EmailService, middleware::AuthUser, user_service,
    websocket::helpers as ws_helpers, workspace_service,
};
use kyomi_core::capability;
use kyomi_core::enums::{
    DatasourceType, InvitationStatus, SubscriptionTier, TransferStatus, WorkspaceRole,
};

use crate::state::AppState;

/// Build the `/workspaces` router with all workspace management endpoints.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/current", get(get_current_workspace))
        .route("/my-workspaces", get(get_my_workspaces))
        .route("/settings", get(get_settings).patch(update_settings))
        .route("/default-dashboard", get(get_default_dashboard))
        .route("/billing", get(get_billing))
        .route("/model-settings", post(update_model_settings))
        .route("/catalog/status", get(get_catalog_status))
        .route(
            "/onboarding/catalog/complete",
            post(complete_catalog_onboarding),
        )
        .route(
            "/{workspace_id}/knowledge",
            get(get_workspace_knowledge).put(update_workspace_knowledge),
        )
        .route(
            "/chartml-config",
            get(get_chartml_config).put(update_chartml_config),
        )
        .route(
            "/settings/microsoft-oauth",
            get(get_microsoft_oauth).put(update_microsoft_oauth),
        )
        // Phase 4D — Members, Invitations, Ownership Transfer
        .route("/members", get(list_members))
        .route(
            "/members/{member_user_id}/role",
            patch(update_member_role_handler),
        )
        .route("/members/{member_user_id}", delete(remove_member_handler))
        // IMPORTANT: /invitations/pending must come BEFORE /invitations/{invitation_id}
        // to prevent axum from treating "pending" as an invitation_id.
        .route("/invitations/pending", get(get_pending_invitations))
        .route(
            "/invitations",
            post(create_invitation_handler).get(list_invitations),
        )
        .route(
            "/invitations/{invitation_id}",
            delete(cancel_invitation_handler),
        )
        .route(
            "/invitations/{invitation_id}/accept",
            post(accept_invitation_handler),
        )
        .route(
            "/invitations/{invitation_id}/decline",
            post(decline_invitation_handler),
        )
        .route("/ownership/transfer", post(initiate_ownership_transfer))
        .route(
            "/ownership/transfer/{transfer_id}/accept",
            post(accept_ownership_transfer_handler),
        )
        .route(
            "/ownership/transfer/{transfer_id}/decline",
            post(decline_ownership_transfer_handler),
        )
        .route(
            "/ownership/transfer/{transfer_id}",
            delete(cancel_ownership_transfer_handler),
        )
        .route("/ownership/transfers", get(get_ownership_transfers))
        // Knowledge admin endpoints
        .route("/admin/populate-graph", post(admin_populate_graph))
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Reject non-workspace-admin users with 403.
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

/// Reject if the user is not the workspace owner.
fn require_workspace_owner(user: &AuthUser) -> Result<(), kyomi_core::Error> {
    if !user.workspace.is_owner {
        return Err(kyomi_core::Error::Forbidden(
            "Only the workspace owner can perform this action".into(),
        ));
    }
    Ok(())
}

/// Map an external/input role name to the database role name.
///
/// Input "admin" → DB "workspace_admin"  (from constants.toml)
/// Input "user"  → DB "workspace_user"   (from constants.toml)
/// Anything else → DB "workspace_user"   (safe default)
fn map_role_to_db(role: &str) -> &'static str {
    let roles = &kyomi_core::constants::get().workspace.roles;
    match role {
        "admin" => &roles.admin,
        _ => &roles.user,
    }
}

/// Generate an invitation ID: `inv-{uuid_hex[0..24]}`.
fn generate_invitation_id() -> String {
    let hex = uuid::Uuid::new_v4().simple().to_string();
    format!("inv-{}", &hex[..24])
}

/// Generate a transfer ID: `transfer-{uuid_hex[0..20]}`.
fn generate_transfer_id() -> String {
    let hex = uuid::Uuid::new_v4().simple().to_string();
    format!("transfer-{}", &hex[..20])
}

/// Load the current workspace from the database, or return an appropriate error.
async fn get_current_ws(
    db: &kyomi_core::DbPool,
    user: &AuthUser,
) -> Result<kyomi_core::models::Workspace, kyomi_core::Error> {
    let ws_id = user
        .workspace
        .workspace_id
        .as_deref()
        .ok_or_else(|| kyomi_core::Error::BadRequest("No workspace selected".into()))?;
    workspace_service::get_workspace_full(db, ws_id)
        .await?
        .ok_or_else(|| kyomi_core::Error::NotFound("Workspace not found".into()))
}

/// Check if any BigQuery datasource in the workspace has arrow streaming enabled.
///
/// Queries `datasource_configs` for active BigQuery datasources and inspects
/// `connection_config.enable_arrow_streaming`. Returns `false` on any error
/// (fail-open — the capability will simply be "direct_api").
async fn has_bq_arrow_streaming(db: &kyomi_core::DbPool, workspace_id: &str) -> bool {
    let datasources = match datasource_service::list_datasources(db, workspace_id, false).await {
        Ok(ds) => ds,
        Err(_) => return false,
    };

    datasources.iter().any(|ds| {
        ds.datasource_type == DatasourceType::Bigquery
            && ds
                .connection_config
                .get("enable_arrow_streaming")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
    })
}

/// Read a nested key from the workspace settings JSON.
///
/// `workspace.settings` is a JSON object. This helper digs into
/// `settings[key]` and returns the value, or `None` if missing.
fn settings_get<'a>(
    settings: &'a Option<serde_json::Value>,
    key: &str,
) -> Option<&'a serde_json::Value> {
    settings.as_ref().and_then(|s| s.get(key))
}

/// Read a nested key from `settings.custom_settings[key]`.
fn custom_settings_get<'a>(
    settings: &'a Option<serde_json::Value>,
    key: &str,
) -> Option<&'a serde_json::Value> {
    settings
        .as_ref()
        .and_then(|s| s.get("custom_settings"))
        .and_then(|cs| cs.get(key))
}

/// Check if a string is a valid GUID (Azure AD tenant ID format).
///
/// Format: `xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx` where x is a hex digit.
fn is_valid_guid(s: &str) -> bool {
    let lower = s.to_lowercase();
    let parts: Vec<&str> = lower.split('-').collect();
    if parts.len() != 5 {
        return false;
    }
    let expected_lengths = [8, 4, 4, 4, 12];
    for (part, &expected_len) in parts.iter().zip(expected_lengths.iter()) {
        if part.len() != expected_len || !part.chars().all(|c| c.is_ascii_hexdigit()) {
            return false;
        }
    }
    true
}

/// Check if a string is a valid domain name.
///
/// Accepts: `contoso.com`, `my-company.onmicrosoft.com`, etc.
fn is_valid_domain(s: &str) -> bool {
    if s.is_empty() || !s.contains('.') {
        return false;
    }
    let first = s.chars().next().unwrap();
    if !first.is_ascii_alphanumeric() {
        return false;
    }
    // Must end with a TLD of at least 2 alpha chars
    if let Some(last_dot) = s.rfind('.') {
        let tld = &s[last_dot + 1..];
        if tld.len() < 2 || !tld.chars().all(|c| c.is_ascii_alphabetic()) {
            return false;
        }
    }
    // All chars must be alphanumeric, dot, or hyphen
    s.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
}

/// Merge a key-value pair into `settings.custom_settings`.
fn merge_custom_settings(
    settings: &Option<serde_json::Value>,
    key: &str,
    value: serde_json::Value,
) -> serde_json::Value {
    let mut s = settings
        .clone()
        .unwrap_or_else(|| serde_json::json!({}));

    // Ensure custom_settings exists
    if s.get("custom_settings").is_none()
        && let Some(obj) = s.as_object_mut()
    {
        obj.insert(
            "custom_settings".to_string(),
            serde_json::json!({}),
        );
    }

    if let Some(cs) = s.get_mut("custom_settings").and_then(|v| v.as_object_mut()) {
        cs.insert(key.to_string(), value);
    }

    s
}

// ---------------------------------------------------------------------------
// Request types
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct UpdateSettingsRequest {
    name: Option<String>,
    arrow_download_enabled: Option<bool>,
    default_dashboard_id: Option<serde_json::Value>, // can be string or null
}

#[derive(Deserialize)]
struct ModelSettingsRequest {
    default_model: String,
}

#[derive(Deserialize)]
struct CompleteCatalogOnboardingRequest {
    #[serde(default)]
    project_ids: Vec<String>,
    billing_project: Option<String>,
    default_project: Option<String>,
    #[serde(default = "default_query_size_limit")]
    query_size_limit_gb: Option<i32>,
}

fn default_query_size_limit() -> Option<i32> {
    Some(50)
}

#[derive(Deserialize)]
struct UpdateKnowledgeRequest {
    knowledge: String,
}

#[derive(Deserialize)]
struct ChartMLConfigRequest {
    config: Option<serde_json::Value>,
}

#[derive(Deserialize)]
struct MicrosoftOAuthRequest {
    enabled: bool,
    tenant_id: Option<String>,
    client_id: Option<String>,
    client_secret: Option<String>,
}

// Phase 4D request types

#[derive(Deserialize)]
struct CreateInvitationRequest {
    email: String,
    role: String,
}

#[derive(Deserialize)]
struct UpdateRoleRequest {
    role: String,
}

#[derive(Deserialize)]
struct InitiateTransferRequest {
    to_user_id: String,
}

// ---------------------------------------------------------------------------
// GET /workspaces/current — Current workspace info
// ---------------------------------------------------------------------------

async fn get_current_workspace(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<serde_json::Value>, kyomi_core::Error> {
    let workspace = get_current_ws(&state.db, &user).await?;

    // Get user count
    let user_count = match workspace_service::count_workspace_users(
        &state.db,
        &workspace.workspace_id,
    )
    .await
    {
        Ok(count) => count,
        Err(e) => {
            tracing::warn!("Failed to count workspace users: {e}");
            1
        }
    };

    Ok(Json(serde_json::json!({
        "workspace_id": workspace.workspace_id,
        "name": workspace.name,
        "domain": workspace.domain,
        "status": workspace.status,
        "created_at": workspace.created_at.to_rfc3339(),
        "user_count": user_count,
        "trial_ends_at": workspace.trial_ends_at.map(|t| t.to_rfc3339()),
    })))
}

// ---------------------------------------------------------------------------
// GET /workspaces/my-workspaces — All user workspaces
// ---------------------------------------------------------------------------

async fn get_my_workspaces(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<serde_json::Value>, kyomi_core::Error> {
    let current_workspace_id = user.workspace.workspace_id.clone();

    let workspace_memberships =
        workspace_service::get_user_workspaces(&state.db, &user.user_id).await?;

    let mut result = Vec::new();
    for (ws, wu) in &workspace_memberships {
        let member_count = match workspace_service::count_workspace_users(
            &state.db,
            &ws.workspace_id,
        )
        .await
        {
            Ok(count) => count,
            Err(e) => {
                tracing::warn!(
                    "Failed to count users for workspace {}: {e}",
                    ws.workspace_id
                );
                1
            }
        };

        result.push(serde_json::json!({
            "workspace_id": ws.workspace_id,
            "name": ws.name,
            "role": wu.role,
            "subscription_tier": ws.subscription_tier,
            "member_count": member_count,
            "is_current": current_workspace_id.as_deref() == Some(&ws.workspace_id),
        }));
    }

    tracing::info!(
        "Found {} workspaces for user {}",
        result.len(),
        user.user_id
    );

    Ok(Json(serde_json::json!(result)))
}

// ---------------------------------------------------------------------------
// GET /workspaces/settings — Settings + capabilities
// ---------------------------------------------------------------------------

async fn get_settings(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<serde_json::Value>, kyomi_core::Error> {
    let workspace = get_current_ws(&state.db, &user).await?;

    // Compute capabilities
    let capabilities = if state.config.self_hosted {
        capability::compute_capabilities_self_hosted(false)
    } else {
        let bq_arrow_enabled = has_bq_arrow_streaming(&state.db, &workspace.workspace_id).await;
        capability::compute_capabilities(&workspace, bq_arrow_enabled)
    };
    let capabilities_json = serde_json::to_value(&capabilities)
        .map_err(|e| kyomi_core::Error::Internal(format!("capability serialization: {e}")))?;

    // Add bigquery_mode alias for frontend compatibility
    let mut caps_map = capabilities_json
        .as_object()
        .cloned()
        .unwrap_or_default();
    if let Some(mode) = caps_map.get("bigquery_retrieval_mode").cloned() {
        caps_map.insert("bigquery_mode".to_string(), mode);
    }

    // Extract default_model from settings.custom_settings
    let default_model = custom_settings_get(&workspace.settings, "default_model")
        .and_then(|v| v.as_str())
        .unwrap_or("claude-sonnet-4-5-20250929");

    // Extract arrow_download_enabled from settings
    let arrow_download_enabled = settings_get(&workspace.settings, "arrow_download_enabled")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);

    // Build user BigQuery preferences for database_connections
    let db_user = user_service::get_user_by_id(&state.db, &user.user_id).await?;
    let mut database_connections: Vec<serde_json::Value> = Vec::new();

    if let Some(ref u) = db_user
        && (u.billing_project.is_some() || u.default_project.is_some())
    {
        database_connections.push(serde_json::json!({
            "type": "bigquery",
            "billing_project": u.billing_project.as_deref().unwrap_or(""),
            "default_project": u.default_project.as_deref().unwrap_or(""),
            "query_size_limit_gb": u.query_size_limit_gb,
        }));
    }

    // Settings blob (the full workspace.settings JSON or empty object)
    let settings_blob = workspace.settings.clone().unwrap_or(serde_json::json!({}));

    Ok(Json(serde_json::json!({
        "workspace_id": workspace.workspace_id,
        "name": workspace.name,
        "subscription_tier": capabilities.subscription_tier,
        "arrow_download_enabled": arrow_download_enabled,
        "database_connections": database_connections,
        "default_model": default_model,
        "settings": settings_blob,
        "capabilities": caps_map,
    })))
}

// ---------------------------------------------------------------------------
// PATCH /workspaces/settings — Update workspace settings (admin)
// ---------------------------------------------------------------------------

async fn update_settings(
    State(state): State<AppState>,
    user: AuthUser,
    Json(data): Json<UpdateSettingsRequest>,
) -> Result<Json<serde_json::Value>, kyomi_core::Error> {
    require_workspace_admin(&user)?;

    let workspace = get_current_ws(&state.db, &user).await?;
    let mut current_settings = workspace.settings.clone().unwrap_or(serde_json::json!({}));

    // Update name if provided
    if let Some(ref name) = data.name {
        let trimmed = name.trim();
        if trimmed.is_empty() {
            return Err(kyomi_core::Error::BadRequest(
                "Workspace name cannot be empty".into(),
            ));
        }
        workspace_service::update_workspace_name(&state.db, &workspace.workspace_id, trimmed)
            .await?;
        tracing::info!(
            "Updated workspace {} name to: {}",
            workspace.workspace_id,
            trimmed
        );
    }

    // Update arrow_download_enabled if provided
    if let Some(arrow_enabled) = data.arrow_download_enabled {
        // Ensure workspace is on Pro tier or higher (check raw DB tier, matching Python)
        if !matches!(
            workspace.subscription_tier,
            SubscriptionTier::Pro | SubscriptionTier::Team | SubscriptionTier::Enterprise
        ) {
            return Err(kyomi_core::Error::Forbidden(
                "Arrow download is only available on Pro, Team, and Enterprise plans".into(),
            ));
        }

        if let Some(obj) = current_settings.as_object_mut() {
            obj.insert(
                "arrow_download_enabled".to_string(),
                serde_json::json!(arrow_enabled),
            );
        }
    }

    // Update default_dashboard_id if provided
    if let Some(ref dashboard_value) = data.default_dashboard_id {
        // Value can be a string (dashboard ID) or null (clear default)
        if let Some(obj) = current_settings.as_object_mut() {
            obj.insert(
                "default_dashboard_id".to_string(),
                dashboard_value.clone(),
            );
        }
        tracing::info!(
            "Updated default_dashboard_id to: {}",
            dashboard_value
        );
    }

    // Write updated settings back to DB
    workspace_service::update_workspace_settings(
        &state.db,
        &workspace.workspace_id,
        &current_settings,
    )
    .await?;

    Ok(Json(serde_json::json!({
        "message": "Workspace settings updated successfully"
    })))
}

// ---------------------------------------------------------------------------
// GET /workspaces/default-dashboard — Default dashboard ID
// ---------------------------------------------------------------------------

async fn get_default_dashboard(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<serde_json::Value>, kyomi_core::Error> {
    let workspace = get_current_ws(&state.db, &user).await?;

    let default_dashboard_id = settings_get(&workspace.settings, "default_dashboard_id")
        .cloned()
        .unwrap_or(serde_json::Value::Null);

    // If set, we could verify the dashboard still exists, but dashboards
    // are Phase 5 — return the stored value as-is for now.
    Ok(Json(serde_json::json!({
        "default_dashboard_id": default_dashboard_id,
    })))
}

// ---------------------------------------------------------------------------
// GET /workspaces/billing — Billing/tier info (admin)
// ---------------------------------------------------------------------------

async fn get_billing(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<serde_json::Value>, kyomi_core::Error> {
    require_workspace_admin(&user)?;

    let workspace = get_current_ws(&state.db, &user).await?;
    let capabilities = if state.config.self_hosted {
        capability::compute_capabilities_self_hosted(false)
    } else {
        let bq_arrow_enabled = has_bq_arrow_streaming(&state.db, &workspace.workspace_id).await;
        capability::compute_capabilities(&workspace, bq_arrow_enabled)
    };
    let tier = capabilities.subscription_tier;
    let credits = capability::get_credits_info(&workspace, tier);

    // Tier pricing table
    let (tier_name, price_monthly, price_annual) = match tier {
        SubscriptionTier::Free => ("Free", 0.0, 0.0),
        SubscriptionTier::Basic => ("Basic", 12.0, 108.0),
        SubscriptionTier::Starter => ("Starter", 12.0, 108.0),
        SubscriptionTier::Pro => ("Pro", 25.0, 228.0),
        SubscriptionTier::Team => ("Team", 65.0, 588.0),
        SubscriptionTier::Enterprise => ("Enterprise", 299.0, 3588.0),
    };

    let billing_cycle = workspace
        .billing_cycle
        .as_deref()
        .unwrap_or_else(|| {
            if tier == SubscriptionTier::Free {
                "none"
            } else {
                "monthly"
            }
        });

    let current_price = if billing_cycle == "monthly" {
        price_monthly
    } else if price_annual > 0.0 {
        price_annual / 12.0
    } else {
        0.0
    };

    let next_billing_date = workspace
        .subscription_period_end
        .map(|t| t.to_rfc3339());

    let dashboard_count: i64 = kyomi_core::db_fetch_scalar!(
        &state.db, i64,
        "SELECT COUNT(*) FROM dashboards WHERE workspace_id = $1",
        &workspace.workspace_id
    )
    .map_err(|e| kyomi_core::Error::Internal(format!("failed to count dashboards: {e}")))?;
    let dashboard_limit: Option<i32> = if tier == SubscriptionTier::Free { Some(5) } else { None };

    Ok(Json(serde_json::json!({
        "subscription": {
            "tier": tier,
            "tier_name": tier_name,
            "status": workspace.subscription_status,
            "billing_cycle": billing_cycle,
            "price_monthly": current_price,
            "price_annual": price_annual,
            "next_billing_date": next_billing_date,
            "stripe_customer_id": workspace.stripe_customer_id,
            "user_limit": capabilities.user_limit,
        },
        "usage": {
            "ai_credits": {
                "percentage_used": credits.percentage_used,
                "used_usd": credits.used_usd,
                "limit_usd": credits.limit_usd,
                "remaining_usd": credits.remaining_usd,
                "exhausted": credits.exhausted,
            },
            "dashboards": {
                "current": dashboard_count,
                "limit": dashboard_limit,
                "is_limited": tier == SubscriptionTier::Free,
            },
        },
        "features": {
            "ai_enabled": capabilities.ai_chat_enabled,
            "bigquery_mode": capabilities.bigquery_retrieval_mode,
            "multi_user": capabilities.multi_user_enabled,
        },
    })))
}

// ---------------------------------------------------------------------------
// POST /workspaces/model-settings — Update default LLM model (admin)
// ---------------------------------------------------------------------------

async fn update_model_settings(
    State(state): State<AppState>,
    user: AuthUser,
    Json(data): Json<ModelSettingsRequest>,
) -> Result<Json<serde_json::Value>, kyomi_core::Error> {
    require_workspace_admin(&user)?;

    let workspace = get_current_ws(&state.db, &user).await?;

    // Merge default_model into custom_settings
    let updated_settings = merge_custom_settings(
        &workspace.settings,
        "default_model",
        serde_json::json!(data.default_model),
    );

    workspace_service::update_workspace_settings(
        &state.db,
        &workspace.workspace_id,
        &updated_settings,
    )
    .await?;

    tracing::info!(
        "Model settings saved for workspace {} by {}",
        workspace.workspace_id,
        user.email
    );

    Ok(Json(serde_json::json!({
        "message": "Model settings saved successfully",
        "default_model": data.default_model,
    })))
}

// ---------------------------------------------------------------------------
// GET /workspaces/catalog/status — Catalog refresh status
// ---------------------------------------------------------------------------

async fn get_catalog_status(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<serde_json::Value>, kyomi_core::Error> {
    let workspace = get_current_ws(&state.db, &user).await?;

    Ok(Json(serde_json::json!({
        "status": workspace.catalog_refresh_status.as_ref().map(AsRef::as_ref).unwrap_or("idle"),
        "progress": workspace.catalog_refresh_progress,
        "last_refresh": workspace.last_catalog_refresh.map(|t| t.to_rfc3339()),
    })))
}

// ---------------------------------------------------------------------------
// POST /workspaces/onboarding/catalog/complete — Complete catalog onboarding
// ---------------------------------------------------------------------------

async fn complete_catalog_onboarding(
    State(state): State<AppState>,
    user: AuthUser,
    Json(data): Json<CompleteCatalogOnboardingRequest>,
) -> Result<Json<serde_json::Value>, kyomi_core::Error> {
    let workspace = get_current_ws(&state.db, &user).await?;

    // Update workspace-level catalog settings (only if project_ids provided
    // and not already completed)
    if !data.project_ids.is_empty() && !workspace.catalog_onboarding_completed {
        let projects_json = serde_json::to_value(&data.project_ids)
            .map_err(|e| kyomi_core::Error::Internal(format!("json: {e}")))?;

        workspace_service::update_catalog_onboarding(
            &state.db,
            &workspace.workspace_id,
            true,
            &projects_json,
        )
        .await?;

        tracing::info!(
            "Workspace catalog configured: {} projects",
            data.project_ids.len()
        );
    }

    // Always update user-level BigQuery preferences
    user_service::update_bigquery_preferences(
        &state.db,
        &user.user_id,
        data.billing_project.as_deref(),
        data.default_project.as_deref(),
        data.query_size_limit_gb,
    )
    .await?;

    tracing::info!(
        "User onboarding completed for {}: billing_project={:?}, default_project={:?}, query_size_limit_gb={:?}",
        user.user_id,
        data.billing_project,
        data.default_project,
        data.query_size_limit_gb,
    );

    // Re-fetch workspace to get the (possibly updated) catalog fields
    let ws = get_current_ws(&state.db, &user).await?;

    Ok(Json(serde_json::json!({
        "status": "success",
        "message": "Onboarding completed",
        "workspace_id": ws.workspace_id,
        "indexed_projects": ws.catalog_indexed_projects.unwrap_or(serde_json::json!([])),
        "billing_project": data.billing_project,
        "default_project": data.default_project,
        "query_size_limit_gb": data.query_size_limit_gb,
    })))
}

// ---------------------------------------------------------------------------
// GET /workspaces/workspace-knowledge — Get workspace knowledge
// ---------------------------------------------------------------------------

async fn get_workspace_knowledge(
    State(state): State<AppState>,
    user: AuthUser,
    Path(workspace_id): Path<String>,
) -> Result<Json<serde_json::Value>, kyomi_core::Error> {
    // Validate path workspace_id matches user's current workspace
    let current_ws_id = user.workspace.workspace_id.as_deref()
        .ok_or_else(|| kyomi_core::Error::BadRequest("No workspace selected".into()))?;
    if workspace_id != current_ws_id {
        return Err(kyomi_core::Error::Forbidden(
            "You do not have access to this workspace".into(),
        ));
    }
    let workspace = get_current_ws(&state.db, &user).await?;

    let knowledge = workspace.business_knowledge.unwrap_or_default();
    let updated_at = workspace.knowledge_updated_at.map(|t| t.to_rfc3339());

    Ok(Json(serde_json::json!({
        "knowledge": knowledge,
        "updated_at": updated_at,
    })))
}

// ---------------------------------------------------------------------------
// PUT /workspaces/{workspace_id}/knowledge — Update workspace knowledge (admin)
// ---------------------------------------------------------------------------

async fn update_workspace_knowledge(
    State(state): State<AppState>,
    user: AuthUser,
    Path(workspace_id): Path<String>,
    Json(data): Json<UpdateKnowledgeRequest>,
) -> Result<Json<serde_json::Value>, kyomi_core::Error> {
    require_workspace_admin(&user)?;

    // Validate path workspace_id matches user's current workspace
    let current_ws_id = user.workspace.workspace_id.as_deref()
        .ok_or_else(|| kyomi_core::Error::BadRequest("No workspace selected".into()))?;
    if workspace_id != current_ws_id {
        return Err(kyomi_core::Error::Forbidden(
            "You do not have access to this workspace".into(),
        ));
    }
    let workspace = get_current_ws(&state.db, &user).await?;

    workspace_service::update_workspace_knowledge(
        &state.db,
        &workspace.workspace_id,
        &data.knowledge,
    )
    .await?;

    // Re-fetch to get updated timestamp
    let ws = get_current_ws(&state.db, &user).await?;
    let updated_at = ws.knowledge_updated_at.map(|t| t.to_rfc3339());

    tracing::info!(
        "Updated knowledge for workspace {}",
        workspace.workspace_id
    );

    Ok(Json(serde_json::json!({
        "knowledge": data.knowledge,
        "updated_at": updated_at,
    })))
}

// ---------------------------------------------------------------------------
// GET /workspaces/chartml-config — Workspace ChartML config
// ---------------------------------------------------------------------------

async fn get_chartml_config(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<serde_json::Value>, kyomi_core::Error> {
    let workspace = get_current_ws(&state.db, &user).await?;

    let chartml_config = custom_settings_get(&workspace.settings, "chartml_config")
        .cloned()
        .unwrap_or(serde_json::json!({}));

    tracing::info!(
        "ChartML config retrieved for workspace {}",
        workspace.workspace_id
    );

    Ok(Json(serde_json::json!({ "config": chartml_config })))
}

// ---------------------------------------------------------------------------
// PUT /workspaces/chartml-config — Update workspace ChartML config (admin)
// ---------------------------------------------------------------------------

async fn update_chartml_config(
    State(state): State<AppState>,
    user: AuthUser,
    Json(data): Json<ChartMLConfigRequest>,
) -> Result<Json<serde_json::Value>, kyomi_core::Error> {
    require_workspace_admin(&user)?;

    let workspace = get_current_ws(&state.db, &user).await?;

    // Validate config is a JSON object if provided
    if let Some(ref config) = data.config
        && !config.is_object()
    {
        return Err(kyomi_core::Error::BadRequest(
            "ChartML config must be a valid JSON object".into(),
        ));
    }

    let config_value = data.config.unwrap_or(serde_json::json!({}));

    // Merge into custom_settings
    let updated_settings =
        merge_custom_settings(&workspace.settings, "chartml_config", config_value);

    workspace_service::update_workspace_settings(
        &state.db,
        &workspace.workspace_id,
        &updated_settings,
    )
    .await?;

    tracing::info!(
        "ChartML config saved for workspace {} by {}",
        workspace.workspace_id,
        user.email
    );

    Ok(Json(serde_json::json!({
        "message": "ChartML config saved successfully"
    })))
}

// ---------------------------------------------------------------------------
// GET /workspaces/settings/microsoft-oauth — MS OAuth config (admin)
// ---------------------------------------------------------------------------

async fn get_microsoft_oauth(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<serde_json::Value>, kyomi_core::Error> {
    require_workspace_admin(&user)?;

    let workspace = get_current_ws(&state.db, &user).await?;

    let microsoft_oauth = custom_settings_get(
        &workspace.settings,
        "microsoft_enterprise_oauth",
    )
    .cloned()
    .unwrap_or(serde_json::json!({}));

    // Return config without exposing client_secret
    Ok(Json(serde_json::json!({
        "enabled": microsoft_oauth.get("enabled").and_then(|v| v.as_bool()).unwrap_or(false),
        "tenant_id": microsoft_oauth.get("tenant_id"),
        "client_id": microsoft_oauth.get("client_id"),
        "has_client_secret": microsoft_oauth.get("client_secret").and_then(|v| v.as_str()).map(|s| !s.is_empty()).unwrap_or(false),
    })))
}

// ---------------------------------------------------------------------------
// PUT /workspaces/settings/microsoft-oauth — Update MS OAuth config (admin)
// ---------------------------------------------------------------------------

async fn update_microsoft_oauth(
    State(state): State<AppState>,
    user: AuthUser,
    Json(data): Json<MicrosoftOAuthRequest>,
) -> Result<Json<serde_json::Value>, kyomi_core::Error> {
    require_workspace_admin(&user)?;

    let workspace = get_current_ws(&state.db, &user).await?;

    // Validate: if enabled, tenant_id is required
    if data.enabled && data.tenant_id.is_none() {
        return Err(kyomi_core::Error::BadRequest(
            "tenant_id is required when enabling Microsoft Enterprise OAuth".into(),
        ));
    }

    // Validate tenant_id format if provided (GUID or domain)
    if let Some(ref tenant_id) = data.tenant_id {
        let is_valid = is_valid_guid(tenant_id) || is_valid_domain(tenant_id);
        if !is_valid {
            return Err(kyomi_core::Error::BadRequest(
                "tenant_id must be a valid Azure AD tenant GUID or domain".into(),
            ));
        }
    }

    // Preserve existing client_secret if not provided in request
    let existing_oauth = custom_settings_get(
        &workspace.settings,
        "microsoft_enterprise_oauth",
    )
    .cloned()
    .unwrap_or(serde_json::json!({}));

    let preserved_secret = data.client_secret.clone().or_else(|| {
        existing_oauth
            .get("client_secret")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
    });

    let now = chrono::Utc::now().to_rfc3339();

    let microsoft_oauth_config = serde_json::json!({
        "enabled": data.enabled,
        "tenant_id": data.tenant_id,
        "client_id": data.client_id,
        "client_secret": preserved_secret,
        "updated_at": now,
        "updated_by": user.user_id,
    });

    let updated_settings = merge_custom_settings(
        &workspace.settings,
        "microsoft_enterprise_oauth",
        microsoft_oauth_config,
    );

    workspace_service::update_workspace_settings(
        &state.db,
        &workspace.workspace_id,
        &updated_settings,
    )
    .await?;

    tracing::info!(
        "Microsoft OAuth settings updated for workspace {} by {} (enabled={})",
        workspace.workspace_id,
        user.email,
        data.enabled
    );

    Ok(Json(serde_json::json!({
        "message": "Microsoft OAuth settings saved successfully",
        "enabled": data.enabled,
        "tenant_id": data.tenant_id,
    })))
}

// ===========================================================================
// Phase 4D — Members
// ===========================================================================

// ---------------------------------------------------------------------------
// GET /workspaces/members — List workspace members
// ---------------------------------------------------------------------------

async fn list_members(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<serde_json::Value>, kyomi_core::Error> {
    let workspace = get_current_ws(&state.db, &user).await?;

    let members =
        workspace_service::get_workspace_members_with_users(&state.db, &workspace.workspace_id)
            .await?;

    let result: Vec<serde_json::Value> = members
        .iter()
        .map(|m| {
            serde_json::json!({
                "user_id": m.user_id,
                "email": m.email,
                "name": m.name,
                "role": m.role,
                "active": m.active,
                "joined_at": m.wu_created_at.to_rfc3339(),
                "is_owner": m.user_id == workspace.owner_user_id,
            })
        })
        .collect();

    Ok(Json(serde_json::json!(result)))
}

// ---------------------------------------------------------------------------
// PATCH /workspaces/members/{member_user_id}/role — Update member role (admin)
// ---------------------------------------------------------------------------

async fn update_member_role_handler(
    State(state): State<AppState>,
    user: AuthUser,
    Path(member_user_id): Path<String>,
    Json(data): Json<UpdateRoleRequest>,
) -> Result<Json<serde_json::Value>, kyomi_core::Error> {
    require_workspace_admin(&user)?;

    let workspace = get_current_ws(&state.db, &user).await?;

    // Cannot change the owner's role
    if member_user_id == workspace.owner_user_id {
        return Err(kyomi_core::Error::BadRequest(
            "Cannot change workspace owner's role".into(),
        ));
    }

    // Map input role to DB role
    let db_role = map_role_to_db(&data.role);

    // Self-demotion guard: if demoting self from admin, ensure at least 2 admins
    if member_user_id == user.user_id && db_role == "workspace_user" {
        let admin_count =
            workspace_service::count_admins(&state.db, &workspace.workspace_id).await?;
        if admin_count < 2 {
            return Err(kyomi_core::Error::BadRequest(
                "Cannot demote yourself: you are the only admin".into(),
            ));
        }
    }

    // Check target member exists
    let target_member =
        workspace_service::get_workspace_user(&state.db, &workspace.workspace_id, &member_user_id)
            .await?;
    if target_member.is_none() {
        return Err(kyomi_core::Error::NotFound(
            "Member not found in workspace".into(),
        ));
    }

    workspace_service::update_member_role(
        &state.db,
        &workspace.workspace_id,
        &member_user_id,
        db_role,
    )
    .await?;

    // Notify the affected member of their role change
    let ws_name = workspace.name.clone().unwrap_or_default();
    ws_helpers::send_member_role_changed(
        &state.ws_manager,
        &member_user_id,
        &workspace.workspace_id,
        &ws_name,
        db_role,
    )
    .await;

    tracing::info!(
        "Updated role for {} to {} in workspace {}",
        member_user_id,
        db_role,
        workspace.workspace_id
    );

    Ok(Json(serde_json::json!({
        "success": true,
        "user_id": member_user_id,
        "new_role": db_role,
        "message": format!("Member role updated to {}", data.role),
    })))
}

// ---------------------------------------------------------------------------
// DELETE /workspaces/members/{member_user_id} — Remove member (admin)
// ---------------------------------------------------------------------------

async fn remove_member_handler(
    State(state): State<AppState>,
    user: AuthUser,
    Path(member_user_id): Path<String>,
) -> Result<Json<serde_json::Value>, kyomi_core::Error> {
    require_workspace_admin(&user)?;

    let workspace = get_current_ws(&state.db, &user).await?;

    // Cannot remove workspace owner (403, matching Python)
    if member_user_id == workspace.owner_user_id {
        return Err(kyomi_core::Error::Forbidden(
            "Cannot remove workspace owner. Transfer ownership first.".into(),
        ));
    }

    // If removing self, check admin count >= 2
    if member_user_id == user.user_id {
        let admin_count =
            workspace_service::count_admins(&state.db, &workspace.workspace_id).await?;
        if admin_count < 2 {
            return Err(kyomi_core::Error::BadRequest(
                "Cannot remove yourself: you are the only admin".into(),
            ));
        }
    }

    // Check target member exists
    let target_member =
        workspace_service::get_workspace_user(&state.db, &workspace.workspace_id, &member_user_id)
            .await?;
    if target_member.is_none() {
        return Err(kyomi_core::Error::NotFound(
            "Member not found in workspace".into(),
        ));
    }

    // Auto-transfer shared conversations to workspace owner
    let transfer_result = kyomi_core::db_execute!(
        &state.db,
        "UPDATE chat_sessions SET user_id = $1 \
         WHERE user_id = $2 AND workspace_id = $3 AND shared = true",
        &workspace.owner_user_id,
        &member_user_id,
        &workspace.workspace_id
    );

    let transferred_count = match transfer_result {
        Ok(result) => {
            let count = result.rows_affected();
            if count > 0 {
                tracing::info!(
                    "Auto-transferred {} shared session(s) from {} to workspace owner {} in workspace {}",
                    count,
                    member_user_id,
                    workspace.owner_user_id,
                    workspace.workspace_id
                );
            }
            count
        }
        Err(e) => {
            tracing::warn!(
                "Failed to transfer shared conversations for removed member {}: {}",
                member_user_id,
                e
            );
            0
        }
    };

    workspace_service::remove_member(&state.db, &workspace.workspace_id, &member_user_id).await?;

    // Send WebSocket notification to removed user
    let workspace_name = workspace
        .name
        .as_deref()
        .unwrap_or("a workspace")
        .to_string();
    let ws_message = format!("You have been removed from \"{}\"", workspace_name);
    ws_helpers::send_workspace_removed(
        &state.ws_manager,
        &member_user_id,
        &workspace.workspace_id,
        &workspace_name,
        &ws_message,
    )
    .await;

    tracing::info!(
        "Removed member {} from workspace {} (transferred {} shared sessions)",
        member_user_id,
        workspace.workspace_id,
        transferred_count
    );

    Ok(Json(serde_json::json!({
        "success": true,
        "user_id": member_user_id,
        "message": "Member removed from workspace",
    })))
}

// ===========================================================================
// Phase 4D — Invitations
// ===========================================================================

// ---------------------------------------------------------------------------
// POST /workspaces/invitations — Create invitation (admin)
// ---------------------------------------------------------------------------

async fn create_invitation_handler(
    State(state): State<AppState>,
    user: AuthUser,
    Json(data): Json<CreateInvitationRequest>,
) -> Result<Json<serde_json::Value>, kyomi_core::Error> {
    require_workspace_admin(&user)?;

    let workspace = get_current_ws(&state.db, &user).await?;
    let email = data.email.trim().to_lowercase();

    // Validate email is not empty
    if email.is_empty() || !email.contains('@') {
        return Err(kyomi_core::Error::BadRequest(
            "Invalid email address".into(),
        ));
    }

    // Check if already a member
    let is_member =
        workspace_service::check_existing_member_by_email(&state.db, &workspace.workspace_id, &email)
            .await?;
    if is_member {
        return Err(kyomi_core::Error::BadRequest(
            "User is already a member of this workspace".into(),
        ));
    }

    // Check for existing pending invitation
    let has_pending =
        workspace_service::check_pending_invitation(&state.db, &workspace.workspace_id, &email)
            .await?;
    if has_pending {
        return Err(kyomi_core::Error::BadRequest(
            "Invitation already pending for this email".into(),
        ));
    }

    // User limit check: current_users + pending_invitations >= user_limit
    // Self-hosted mode: no user limits.
    if !state.config.self_hosted {
        let current_users =
            workspace_service::count_workspace_users(&state.db, &workspace.workspace_id).await?;
        let pending_invitations =
            workspace_service::count_pending_invitations(&state.db, &workspace.workspace_id).await?;
        let user_limit = workspace.user_limit.unwrap_or(1) as i64;

        if current_users + pending_invitations >= user_limit {
            return Err(kyomi_core::Error::BadRequest(
                "Workspace user limit reached. Upgrade your plan to add more users.".into(),
            ));
        }
    }

    let invitation_id = generate_invitation_id();
    let expires_at = chrono::Utc::now() + chrono::Duration::days(7);

    let role = map_role_to_db(&data.role);

    let invitation = workspace_service::create_invitation(
        &state.db,
        &invitation_id,
        &workspace.workspace_id,
        &email,
        role,
        &user.user_id,
        expires_at,
    )
    .await?;

    // Look up inviter name for the response (fallback to email, matching Python)
    let inviter_name = user
        .name
        .clone()
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| user.email.clone());

    let workspace_name = workspace
        .name
        .as_deref()
        .unwrap_or("a workspace")
        .to_string();

    // Send WebSocket notification if invitee has an account
    let invited_user = user_service::get_user_by_email(&state.db, &email).await?;
    if let Some(ref invited) = invited_user {
        let ws_message = format!(
            "You have been invited by {} to join \"{}\"",
            inviter_name, workspace_name
        );
        ws_helpers::send_workspace_invitation(
            &state.ws_manager,
            &invited.user_id,
            &invitation_id,
            &workspace.workspace_id,
            &workspace_name,
            &inviter_name,
            &data.role,
            &ws_message,
        )
        .await;
    }

    // Send invitation email (fire-and-forget, log on failure)
    // Self-hosted without SMTP: skip email — invitee can still accept via direct link
    if state.config.self_hosted && !state.config.smtp_configured() {
        tracing::info!(
            "Self-hosted SMTP-less: skipping invitation email to {} (invitation still created)",
            email
        );
    } else {
        let email_clone = email.clone();
        let ws_name = workspace_name.clone();
        let inv_name = inviter_name.clone();
        let inv_role = data.role.clone();
        tokio::spawn(async move {
            let email_svc = EmailService::from_env();
            let sent = email_svc
                .send_workspace_invitation(&email_clone, &ws_name, &inv_name, &inv_role)
                .await;
            if sent {
                tracing::info!("Invitation email sent to {}", email_clone);
            } else {
                tracing::warn!(
                    "Failed to send invitation email to {} (SMTP may not be configured)",
                    email_clone
                );
            }
        });
    }

    tracing::info!(
        "Created invitation {} for {} to workspace {} by {}",
        invitation_id,
        email,
        workspace.workspace_id,
        user.user_id
    );

    Ok(Json(serde_json::json!({
        "invitation_id": invitation.invitation_id,
        "workspace_id": invitation.workspace_id,
        "workspace_name": workspace_name,
        "email": invitation.email,
        "role": invitation.role,
        "invited_by_user_id": invitation.invited_by_user_id,
        "invited_by_name": inviter_name,
        "status": invitation.status,
        "created_at": invitation.created_at.to_rfc3339(),
        "expires_at": invitation.expires_at.to_rfc3339(),
    })))
}

// ---------------------------------------------------------------------------
// GET /workspaces/invitations — List all invitations (admin)
// ---------------------------------------------------------------------------

async fn list_invitations(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<serde_json::Value>, kyomi_core::Error> {
    require_workspace_admin(&user)?;

    let workspace = get_current_ws(&state.db, &user).await?;

    let invitations =
        workspace_service::get_pending_invitations_for_workspace(&state.db, &workspace.workspace_id)
            .await?;

    // Pre-fetch inviter names (dedup by user_id to avoid N+1)
    let mut inviter_names = std::collections::HashMap::new();
    for inv in &invitations {
        if !inviter_names.contains_key(&inv.invited_by_user_id) {
            let inviter =
                user_service::get_user_by_id(&state.db, &inv.invited_by_user_id).await?;
            let name = inviter
                .as_ref()
                .and_then(|u| u.name.clone())
                .filter(|n| !n.is_empty())
                .or_else(|| inviter.as_ref().map(|u| u.email.clone()))
                .unwrap_or_else(|| "Someone".to_string());
            inviter_names.insert(inv.invited_by_user_id.clone(), name);
        }
    }

    let result: Vec<serde_json::Value> = invitations
        .iter()
        .map(|inv| {
            let inviter_name = inviter_names
                .get(&inv.invited_by_user_id)
                .cloned()
                .unwrap_or_else(|| "Someone".to_string());

            serde_json::json!({
                "invitation_id": inv.invitation_id,
                "workspace_id": inv.workspace_id,
                "workspace_name": workspace.name,
                "email": inv.email,
                "role": inv.role,
                "invited_by_user_id": inv.invited_by_user_id,
                "invited_by_name": inviter_name,
                "status": inv.status,
                "created_at": inv.created_at.to_rfc3339(),
                "expires_at": inv.expires_at.to_rfc3339(),
            })
        })
        .collect();

    Ok(Json(serde_json::json!(result)))
}

// ---------------------------------------------------------------------------
// GET /workspaces/invitations/pending — Pending invitations for current user
// ---------------------------------------------------------------------------

async fn get_pending_invitations(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<serde_json::Value>, kyomi_core::Error> {
    let invitations =
        workspace_service::get_pending_invitations_for_email(&state.db, &user.email).await?;

    let mut result = Vec::new();
    for inv in &invitations {
        // Look up workspace name for each invitation
        let ws_name = workspace_service::get_workspace_full(&state.db, &inv.workspace_id)
            .await?
            .and_then(|ws| ws.name.clone());

        // Look up inviter name
        let inviter_name = user_service::get_user_by_id(&state.db, &inv.invited_by_user_id)
            .await?
            .and_then(|u| u.name.clone());

        result.push(serde_json::json!({
            "invitation_id": inv.invitation_id,
            "workspace_id": inv.workspace_id,
            "workspace_name": ws_name,
            "email": inv.email,
            "role": inv.role,
            "invited_by_user_id": inv.invited_by_user_id,
            "invited_by_name": inviter_name,
            "status": inv.status,
            "created_at": inv.created_at.to_rfc3339(),
            "expires_at": inv.expires_at.to_rfc3339(),
        }));
    }

    Ok(Json(serde_json::json!(result)))
}

// ---------------------------------------------------------------------------
// DELETE /workspaces/invitations/{invitation_id} — Cancel invitation (admin)
// ---------------------------------------------------------------------------

async fn cancel_invitation_handler(
    State(state): State<AppState>,
    user: AuthUser,
    Path(invitation_id): Path<String>,
) -> Result<Json<serde_json::Value>, kyomi_core::Error> {
    require_workspace_admin(&user)?;

    let workspace = get_current_ws(&state.db, &user).await?;

    let invitation = workspace_service::get_invitation_in_workspace(
        &state.db,
        &invitation_id,
        &workspace.workspace_id,
    )
    .await?
    .ok_or_else(|| kyomi_core::Error::NotFound("Invitation not found".into()))?;

    if invitation.status != InvitationStatus::Pending {
        return Err(kyomi_core::Error::BadRequest(
            "Can only cancel pending invitations".into(),
        ));
    }

    workspace_service::update_invitation_status(&state.db, &invitation_id, "cancelled").await?;

    tracing::info!(
        "Cancelled invitation {} in workspace {} by {}",
        invitation_id,
        workspace.workspace_id,
        user.user_id
    );

    Ok(Json(serde_json::json!({
        "success": true,
        "message": "Invitation cancelled",
    })))
}

// ---------------------------------------------------------------------------
// POST /workspaces/invitations/{invitation_id}/accept — Accept invitation
// ---------------------------------------------------------------------------

async fn accept_invitation_handler(
    State(state): State<AppState>,
    user: AuthUser,
    Path(invitation_id): Path<String>,
) -> Result<Json<serde_json::Value>, kyomi_core::Error> {
    let invitation = workspace_service::get_invitation(&state.db, &invitation_id)
        .await?
        .ok_or_else(|| kyomi_core::Error::NotFound("Invitation not found".into()))?;

    // Check pending status
    if invitation.status != InvitationStatus::Pending {
        return Err(kyomi_core::Error::BadRequest(
            "Invitation is no longer pending".into(),
        ));
    }

    // Check expiry
    if invitation.expires_at < chrono::Utc::now() {
        // Auto-expire the invitation
        workspace_service::update_invitation_status(&state.db, &invitation_id, "expired").await?;
        return Err(kyomi_core::Error::BadRequest(
            "Invitation has expired".into(),
        ));
    }

    // Check email match (case-insensitive, matching Python error message)
    if invitation.email.to_lowercase() != user.email.to_lowercase() {
        return Err(kyomi_core::Error::Forbidden(
            format!(
                "This invitation was sent to {}. Please sign in with that account to accept.",
                invitation.email
            ),
        ));
    }

    // Check not already a member (400, matching Python)
    let is_member = workspace_service::check_existing_member_by_email(
        &state.db,
        &invitation.workspace_id,
        &user.email,
    )
    .await?;
    if is_member {
        return Err(kyomi_core::Error::BadRequest(
            "You are already a member of this workspace".into(),
        ));
    }

    // The invitation already stores the proper WorkspaceRole enum;
    // convert to the &str the service layer expects.
    let db_role = invitation.role.as_ref();

    // Create workspace membership
    workspace_service::create_workspace_user(
        &state.db,
        &invitation.workspace_id,
        &user.user_id,
        db_role,
    )
    .await?;

    // Mark invitation as accepted
    workspace_service::accept_invitation(&state.db, &invitation_id, &user.user_id).await?;

    // Notify all workspace members about the new member
    let user_display = user.name.as_deref().unwrap_or(&user.email);
    ws_helpers::send_member_joined(
        &state.ws_manager,
        &invitation.workspace_id,
        user_display,
        db_role,
    )
    .await;

    tracing::info!(
        "User {} accepted invitation {} to workspace {}",
        user.user_id,
        invitation_id,
        invitation.workspace_id
    );

    Ok(Json(serde_json::json!({
        "success": true,
        "workspace_id": invitation.workspace_id,
        "role": db_role,
        "message": "Successfully joined workspace",
    })))
}

// ---------------------------------------------------------------------------
// POST /workspaces/invitations/{invitation_id}/decline — Decline invitation
// ---------------------------------------------------------------------------

async fn decline_invitation_handler(
    State(state): State<AppState>,
    user: AuthUser,
    Path(invitation_id): Path<String>,
) -> Result<Json<serde_json::Value>, kyomi_core::Error> {
    let invitation = workspace_service::get_invitation(&state.db, &invitation_id)
        .await?
        .ok_or_else(|| kyomi_core::Error::NotFound("Invitation not found".into()))?;

    if invitation.status != InvitationStatus::Pending {
        return Err(kyomi_core::Error::BadRequest(
            "Invitation is no longer pending".into(),
        ));
    }

    // Check email match
    if invitation.email.to_lowercase() != user.email.to_lowercase() {
        return Err(kyomi_core::Error::Forbidden(
            "This invitation was sent to a different email address".into(),
        ));
    }

    workspace_service::update_invitation_status(&state.db, &invitation_id, "declined").await?;

    tracing::info!(
        "User {} declined invitation {} to workspace {}",
        user.user_id,
        invitation_id,
        invitation.workspace_id
    );

    Ok(Json(serde_json::json!({
        "success": true,
        "message": "Invitation declined",
    })))
}

// ===========================================================================
// Phase 4D — Ownership Transfer
// ===========================================================================

// ---------------------------------------------------------------------------
// POST /workspaces/ownership/transfer — Initiate ownership transfer (owner only)
// ---------------------------------------------------------------------------

async fn initiate_ownership_transfer(
    State(state): State<AppState>,
    user: AuthUser,
    Json(data): Json<InitiateTransferRequest>,
) -> Result<Json<serde_json::Value>, kyomi_core::Error> {
    require_workspace_owner(&user)?;

    let workspace = get_current_ws(&state.db, &user).await?;

    // Cannot transfer to self (matching Python error message)
    if data.to_user_id == user.user_id {
        return Err(kyomi_core::Error::BadRequest(
            "You are already the owner".into(),
        ));
    }

    // Check target user is a member of the workspace
    let target_member = workspace_service::get_workspace_user(
        &state.db,
        &workspace.workspace_id,
        &data.to_user_id,
    )
    .await?;
    if target_member.is_none() {
        return Err(kyomi_core::Error::NotFound(
            "Recipient must be a member of the workspace".into(),
        ));
    }

    // Check no existing pending transfer
    let existing_transfer =
        workspace_service::get_pending_transfer_for_workspace(&state.db, &workspace.workspace_id)
            .await?;
    if existing_transfer.is_some() {
        return Err(kyomi_core::Error::BadRequest(
            "There is already a pending ownership transfer for this workspace".into(),
        ));
    }

    let transfer_id = generate_transfer_id();
    let expires_at = chrono::Utc::now() + chrono::Duration::days(7);

    let transfer = workspace_service::create_ownership_transfer(
        &state.db,
        &transfer_id,
        &workspace.workspace_id,
        &user.user_id,
        &data.to_user_id,
        expires_at,
    )
    .await?;

    // Look up target user email for the response
    let target_user = user_service::get_user_by_id(&state.db, &data.to_user_id).await?;
    let to_email = target_user.map(|u| u.email).unwrap_or_default();

    // Send WebSocket notification to the transfer recipient
    let workspace_name = workspace
        .name
        .as_deref()
        .unwrap_or("a workspace");
    ws_helpers::send_ownership_transfer_offered(
        &state.ws_manager,
        &data.to_user_id,
        &transfer_id,
        workspace_name,
        &user.email,
    )
    .await;

    tracing::info!(
        "Ownership transfer {} initiated in workspace {} from {} to {}",
        transfer_id,
        workspace.workspace_id,
        user.user_id,
        data.to_user_id
    );

    Ok(Json(serde_json::json!({
        "success": true,
        "transfer_id": transfer.transfer_id,
        "to_user_email": to_email,
        "expires_at": transfer.expires_at.to_rfc3339(),
        "message": "Ownership transfer request created",
    })))
}

// ---------------------------------------------------------------------------
// POST /workspaces/ownership/transfer/{transfer_id}/accept — Accept transfer
// ---------------------------------------------------------------------------

async fn accept_ownership_transfer_handler(
    State(state): State<AppState>,
    user: AuthUser,
    Path(transfer_id): Path<String>,
) -> Result<Json<serde_json::Value>, kyomi_core::Error> {
    let transfer = workspace_service::get_ownership_transfer(&state.db, &transfer_id)
        .await?
        .ok_or_else(|| kyomi_core::Error::NotFound("Transfer not found".into()))?;

    // Check pending status
    if transfer.status != TransferStatus::Pending {
        return Err(kyomi_core::Error::BadRequest(
            "Transfer is no longer pending".into(),
        ));
    }

    // Check expiry
    if transfer.expires_at < chrono::Utc::now() {
        workspace_service::update_transfer_status(&state.db, &transfer_id, "expired").await?;
        return Err(kyomi_core::Error::BadRequest(
            "Transfer has expired".into(),
        ));
    }

    // Only the recipient can accept
    if transfer.to_user_id != user.user_id {
        return Err(kyomi_core::Error::Forbidden(
            "Only the transfer recipient can accept".into(),
        ));
    }

    // Complete the transfer in a transaction
    workspace_service::complete_ownership_transfer(
        &state.db,
        &transfer_id,
        &transfer.workspace_id,
        &user.user_id,
    )
    .await?;

    // Notify the previous owner that transfer was accepted
    let ws = workspace_service::get_workspace_full(&state.db, &transfer.workspace_id).await?;
    let ws_name = ws.as_ref().and_then(|w| w.name.as_deref()).unwrap_or("a workspace");
    let new_owner_name = user.name.as_deref().unwrap_or(&user.email);
    ws_helpers::send_ownership_transfer_completed(
        &state.ws_manager,
        &transfer.from_user_id,
        &transfer_id,
        &transfer.workspace_id,
        ws_name,
        new_owner_name,
    )
    .await;

    tracing::info!(
        "Ownership transfer {} accepted: workspace {} now owned by {}",
        transfer_id,
        transfer.workspace_id,
        user.user_id
    );

    Ok(Json(serde_json::json!({
        "success": true,
        "workspace_id": transfer.workspace_id,
        "message": "You are now the workspace owner",
    })))
}

// ---------------------------------------------------------------------------
// POST /workspaces/ownership/transfer/{transfer_id}/decline — Decline transfer
// ---------------------------------------------------------------------------

async fn decline_ownership_transfer_handler(
    State(state): State<AppState>,
    user: AuthUser,
    Path(transfer_id): Path<String>,
) -> Result<Json<serde_json::Value>, kyomi_core::Error> {
    let transfer = workspace_service::get_ownership_transfer(&state.db, &transfer_id)
        .await?
        .ok_or_else(|| kyomi_core::Error::NotFound("Transfer not found".into()))?;

    if transfer.status != TransferStatus::Pending {
        return Err(kyomi_core::Error::BadRequest(
            "Transfer is no longer pending".into(),
        ));
    }

    // Only the recipient can decline
    if transfer.to_user_id != user.user_id {
        return Err(kyomi_core::Error::Forbidden(
            "Only the transfer recipient can decline".into(),
        ));
    }

    workspace_service::update_transfer_status(&state.db, &transfer_id, "declined").await?;

    // Notify the original owner that transfer was declined
    let ws = workspace_service::get_workspace_full(&state.db, &transfer.workspace_id).await?;
    let ws_name = ws.as_ref().and_then(|w| w.name.as_deref()).unwrap_or("a workspace");
    let declined_by = user.name.as_deref().unwrap_or(&user.email);
    ws_helpers::send_ownership_transfer_declined(
        &state.ws_manager,
        &transfer.from_user_id,
        &transfer_id,
        &transfer.workspace_id,
        ws_name,
        declined_by,
    )
    .await;

    tracing::info!(
        "Ownership transfer {} declined by {}",
        transfer_id,
        user.user_id
    );

    Ok(Json(serde_json::json!({
        "success": true,
        "message": "Ownership transfer declined",
    })))
}

// ---------------------------------------------------------------------------
// DELETE /workspaces/ownership/transfer/{transfer_id} — Cancel transfer (owner)
// ---------------------------------------------------------------------------

async fn cancel_ownership_transfer_handler(
    State(state): State<AppState>,
    user: AuthUser,
    Path(transfer_id): Path<String>,
) -> Result<Json<serde_json::Value>, kyomi_core::Error> {
    let transfer = workspace_service::get_ownership_transfer(&state.db, &transfer_id)
        .await?
        .ok_or_else(|| kyomi_core::Error::NotFound("Transfer not found".into()))?;

    if transfer.status != TransferStatus::Pending {
        return Err(kyomi_core::Error::BadRequest(
            "Transfer is no longer pending".into(),
        ));
    }

    // Only the initiator (from_user) can cancel
    if transfer.from_user_id != user.user_id {
        return Err(kyomi_core::Error::Forbidden(
            "Only the transfer initiator can cancel".into(),
        ));
    }

    workspace_service::update_transfer_status(&state.db, &transfer_id, "cancelled").await?;

    tracing::info!(
        "Ownership transfer {} cancelled by {}",
        transfer_id,
        user.user_id
    );

    Ok(Json(serde_json::json!({
        "success": true,
        "message": "Ownership transfer cancelled",
    })))
}

// ---------------------------------------------------------------------------
// GET /workspaces/ownership/transfers — List transfers for current user
// ---------------------------------------------------------------------------

async fn get_ownership_transfers(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<serde_json::Value>, kyomi_core::Error> {
    // Get transfers where user is either initiator or recipient
    // Fetch pending transfers for the current user (as recipient)
    let received_transfers =
        workspace_service::get_pending_transfers_for_user(&state.db, &user.user_id).await?;

    // Also check if user has initiated any transfers (as workspace owner)
    let workspace = get_current_ws(&state.db, &user).await?;
    let initiated_transfer =
        workspace_service::get_pending_transfer_for_workspace(&state.db, &workspace.workspace_id)
            .await?;

    // Merge and deduplicate transfers
    let mut all_transfers: Vec<kyomi_core::models::OwnershipTransfer> = received_transfers;
    if let Some(t) = initiated_transfer
        && !all_transfers.iter().any(|existing| existing.transfer_id == t.transfer_id)
    {
        all_transfers.push(t);
    }

    let mut result = Vec::new();
    for transfer in &all_transfers {
        // Look up workspace name and user emails
        let ws = workspace_service::get_workspace_full(&state.db, &transfer.workspace_id).await?;
        let ws_name = ws.and_then(|w| w.name.clone());

        let from_user = user_service::get_user_by_id(&state.db, &transfer.from_user_id).await?;
        let from_email = from_user.map(|u| u.email).unwrap_or_default();

        let to_user = user_service::get_user_by_id(&state.db, &transfer.to_user_id).await?;
        let to_email = to_user.map(|u| u.email).unwrap_or_default();

        result.push(serde_json::json!({
            "transfer_id": transfer.transfer_id,
            "workspace_id": transfer.workspace_id,
            "workspace_name": ws_name,
            "from_user_id": transfer.from_user_id,
            "from_user_email": from_email,
            "to_user_id": transfer.to_user_id,
            "to_user_email": to_email,
            "status": transfer.status,
            "created_at": transfer.created_at.to_rfc3339(),
            "expires_at": transfer.expires_at.to_rfc3339(),
            "is_initiator": transfer.from_user_id == user.user_id,
            "is_recipient": transfer.to_user_id == user.user_id,
        }));
    }

    Ok(Json(serde_json::json!(result)))
}

// ---------------------------------------------------------------------------
// POST /workspaces/admin/populate-graph — Full knowledge population (admin)
// ---------------------------------------------------------------------------

/// Populate knowledge embeddings from PostgreSQL data.
///
/// Runs the full population pipeline: tables, columns, and learnings.
/// Idempotent — safe to run multiple times.
///
/// Requires workspace admin role.
async fn admin_populate_graph(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<serde_json::Value>, kyomi_core::Error> {
    require_workspace_admin(&user)?;

    let workspace = get_current_ws(&state.db, &user).await?;
    let workspace_id = &workspace.workspace_id;

    let embed = state.embedding.get()?;

    kyomi_knowledge::populate::populate_workspace(&state.db, embed, workspace_id)
        .await
        .map_err(|e| kyomi_core::Error::Internal(format!("Knowledge population failed: {e}")))?;

    // Backfill learning references
    let refs_count = kyomi_knowledge::references::backfill_all_references(&state.db, workspace_id)
        .await
        .map_err(|e| kyomi_core::Error::Internal(format!("Reference backfill failed: {e}")))?;

    tracing::info!(
        workspace_id,
        learnings_with_refs = refs_count,
        "Admin knowledge population complete"
    );

    Ok(Json(serde_json::json!({
        "status": "complete",
        "learnings_with_references": refs_count,
    })))
}
