// SPDX-License-Identifier: AGPL-3.0-or-later

//! Server functions for Analytics settings.
//!
//! These replace the REST API calls for analytics sites:
//! - `GET /analytics/sites` -> `list_analytics_sites()`
//! - `GET /analytics/usage` -> `get_analytics_usage()`
//! - `POST /analytics/sites` -> `create_analytics_site()`
//! - `PUT /analytics/sites/{id}` -> `update_analytics_site()`
//! - `DELETE /analytics/sites/{id}` -> `delete_analytics_site()`
//!
//! Calls the same service-layer code as `apps/server/src/routes/analytics_sites.rs`.

use leptos::prelude::*;
use serde::{Deserialize, Serialize};

#[cfg(feature = "ssr")]
use super::{extract_auth, extract_context, workspace_id};

// ─── Types ──────────────────────────────────────────────────────────────────

/// An analytics site returned from the server.
///
/// Matches the JSON shape returned by `GET /analytics/sites`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AnalyticsSiteData {
    pub id: String,
    pub name: String,
    pub site_id: String,
    pub allowed_domains: Vec<String>,
    pub snippet: String,
    pub datasource_slug: Option<String>,
    pub created_at: String,
}

/// Analytics event usage data.
///
/// Matches the JSON shape returned by `GET /analytics/usage`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AnalyticsUsageData {
    /// Events consumed this billing period.
    pub events_used: u64,
    /// Included monthly event quota (resets each period).
    pub events_limit: u64,
    /// Usage percentage against the included quota (0..=100+).
    pub usage_percent: f64,
    /// Non-expiring bundle event balance (from bundle purchases).
    pub bundle_balance: u64,
    pub status: String,
}

// ─── Server Functions ───────────────────────────────────────────────────────

/// List all analytics sites for the current workspace.
#[server(prefix = "/leptos-api")]
pub async fn list_analytics_sites() -> Result<Vec<AnalyticsSiteData>, ServerFnError> {
    let auth = extract_auth().await?;
    let ctx = extract_context()?;
    let ws_id = workspace_id(&auth)?;

    require_workspace_admin(&auth)?;

    let sites = kyomi_auth::analytics_site_service::list_sites(&ctx.db, ws_id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;

    Ok(sites
        .iter()
        .map(|s| AnalyticsSiteData {
            id: s.id.clone(),
            name: s.name.clone(),
            site_id: s.site_id.clone(),
            allowed_domains: s.allowed_domains.clone(),
            snippet: kyomi_auth::analytics_site_service::snippet_tag(&s.signed_key),
            datasource_slug: s.datasource_slug.clone(),
            created_at: s.created_at.to_rfc3339(),
        })
        .collect())
}

/// Fetch analytics event usage for the current workspace.
#[server(prefix = "/leptos-api")]
pub async fn get_analytics_usage() -> Result<AnalyticsUsageData, ServerFnError> {
    let auth = extract_auth().await?;
    let ctx = extract_context()?;
    let ws_id = workspace_id(&auth)?;

    // Self-hosted: no quota tracking
    if ctx.config.self_hosted {
        return Ok(AnalyticsUsageData {
            events_used: 0,
            events_limit: 0,
            usage_percent: 0.0,
            bundle_balance: 0,
            status: "ok".to_string(),
        });
    }

    let tier_str = auth.workspace.subscription_tier.as_ref();
    let configs = kyomi_auth::analytics_quota::default_tier_configs();
    let config = configs
        .get(tier_str)
        .ok_or_else(|| ServerFnError::new(format!("Unknown subscription tier: {tier_str}")))?;

    // Get usage from Redis — requires RedisPool from the kv layer.
    // The KVPool abstraction doesn't expose raw Redis, so we create a
    // temporary connection from config when the Redis URL is available.
    let events_used = if let Some(ref redis_url) = ctx.config.redis_url {
        match kyomi_core::redis::create_pool(redis_url).await {
            Ok(mut conn) => {
                kyomi_auth::analytics_quota::get_usage_count(&mut conn, ws_id)
                    .await
                    .unwrap_or(0)
            }
            Err(_) => 0u64,
        }
    } else {
        0u64
    };

    let events_limit = config.monthly_event_limit;
    let grace_limit = config.grace_limit();
    let usage_percent = if events_limit > 0 {
        (events_used as f64 / events_limit as f64) * 100.0
    } else {
        0.0
    };

    // Load non-expiring bundle balance from the workspace row.
    let bundle_balance: u64 = kyomi_core::db_fetch_scalar!(
        &ctx.db,
        i64,
        "SELECT COALESCE(analytics_bundle_events, 0) FROM workspaces WHERE workspace_id = $1",
        ws_id
    )
    .map_err(|e| ServerFnError::new(format!("Failed to load bundle balance: {e}")))?
    .max(0) as u64;

    // Status considers both the included quota AND the bundle reserve.
    // If usage is over the included quota but bundles are available, we're
    // drawing from the reserve — not "exceeded" yet.
    let over_included = events_used.saturating_sub(events_limit);
    let bundle_exhausted = over_included >= bundle_balance;

    let status = if events_used >= grace_limit && bundle_exhausted {
        "blocked"
    } else if events_used >= events_limit && bundle_exhausted {
        "exceeded"
    } else if over_included > 0 && bundle_balance > 0 {
        // Over included and actually drawing from bundle — informational state
        "reserve"
    } else if events_used >= events_limit * 80 / 100 {
        "warning"
    } else {
        "ok"
    };

    Ok(AnalyticsUsageData {
        events_used,
        events_limit,
        usage_percent,
        bundle_balance,
        status: status.to_string(),
    })
}

/// Create a new analytics site.
#[server(prefix = "/leptos-api")]
pub async fn create_analytics_site(
    name: String,
    allowed_domains: String,
    datasource_slug: Option<String>,
) -> Result<AnalyticsSiteData, ServerFnError> {
    let auth = extract_auth().await?;
    let ctx = extract_context()?;
    let ws_id = workspace_id(&auth)?;

    require_workspace_admin(&auth)?;

    let name = name.trim();
    if name.is_empty() || name.len() > 255 {
        return Err(ServerFnError::new("Site name must be 1-255 characters"));
    }

    let domains: Vec<String> = allowed_domains
        .split(',')
        .map(|d| d.trim().to_string())
        .filter(|d| !d.is_empty())
        .collect();

    if domains.is_empty() {
        return Err(ServerFnError::new("At least one domain is required"));
    }

    if ctx.config.analytics_signing_secret.is_empty() {
        return Err(ServerFnError::new(
            "Analytics signing secret is not configured",
        ));
    }

    let site = kyomi_auth::analytics_site_service::create_site(
        &ctx.db,
        ws_id,
        name,
        &domains,
        &ctx.config.analytics_signing_secret,
        datasource_slug.as_deref(),
        &ctx.config.analytics_clickhouse_host,
        ctx.config.analytics_clickhouse_port,
        &ctx.config.analytics_clickhouse_password,
        ctx.config.analytics_clickhouse_secure,
    )
    .await
    .map_err(|e| ServerFnError::new(e.to_string()))?;

    Ok(AnalyticsSiteData {
        id: site.id.clone(),
        name: site.name.clone(),
        site_id: site.site_id.clone(),
        allowed_domains: site.allowed_domains.clone(),
        snippet: kyomi_auth::analytics_site_service::snippet_tag(&site.signed_key),
        datasource_slug: site.datasource_slug.clone(),
        created_at: site.created_at.to_rfc3339(),
    })
}

/// Update an existing analytics site.
#[server(prefix = "/leptos-api")]
pub async fn update_analytics_site(
    site_id: String,
    name: String,
    allowed_domains: String,
    datasource_slug: Option<String>,
) -> Result<AnalyticsSiteData, ServerFnError> {
    let auth = extract_auth().await?;
    let ctx = extract_context()?;
    let ws_id = workspace_id(&auth)?;

    require_workspace_admin(&auth)?;

    let name = name.trim();
    if name.is_empty() || name.len() > 255 {
        return Err(ServerFnError::new("Site name must be 1-255 characters"));
    }

    let domains: Vec<String> = allowed_domains
        .split(',')
        .map(|d| d.trim().to_string())
        .filter(|d| !d.is_empty())
        .collect();

    if domains.is_empty() {
        return Err(ServerFnError::new("At least one domain is required"));
    }

    let site = kyomi_auth::analytics_site_service::update_site(
        &ctx.db,
        &site_id,
        ws_id,
        Some(name),
        Some(&domains),
        &ctx.config.analytics_signing_secret,
        datasource_slug.as_deref(),
    )
    .await
    .map_err(|e| ServerFnError::new(e.to_string()))?;

    Ok(AnalyticsSiteData {
        id: site.id.clone(),
        name: site.name.clone(),
        site_id: site.site_id.clone(),
        allowed_domains: site.allowed_domains.clone(),
        snippet: kyomi_auth::analytics_site_service::snippet_tag(&site.signed_key),
        datasource_slug: site.datasource_slug.clone(),
        created_at: site.created_at.to_rfc3339(),
    })
}

/// Delete an analytics site.
#[server(prefix = "/leptos-api")]
pub async fn delete_analytics_site(site_id: String) -> Result<(), ServerFnError> {
    let auth = extract_auth().await?;
    let ctx = extract_context()?;
    let ws_id = workspace_id(&auth)?;

    require_workspace_admin(&auth)?;

    kyomi_auth::analytics_site_service::delete_site(
        &ctx.db,
        &site_id,
        ws_id,
        &ctx.config.analytics_clickhouse_host,
        ctx.config.analytics_clickhouse_port,
        &ctx.config.analytics_clickhouse_password,
        ctx.config.analytics_clickhouse_secure,
    )
    .await
    .map_err(|e| ServerFnError::new(e.to_string()))?;

    Ok(())
}

// ─── Helpers ────────────────────────────────────────────────────────────────

/// Check that the auth user has workspace admin role.
#[cfg(feature = "ssr")]
fn require_workspace_admin(
    auth: &kyomi_auth::middleware::AuthUser,
) -> Result<(), ServerFnError> {
    if !auth
        .workspace
        .workspace_roles
        .contains(&kyomi_core::enums::WorkspaceRole::WorkspaceAdmin)
    {
        return Err(ServerFnError::new("Workspace admin access required"));
    }
    Ok(())
}
