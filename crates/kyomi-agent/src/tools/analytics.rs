// SPDX-License-Identifier: AGPL-3.0-or-later

//! Analytics site MCP tools — list, create, update, and delete analytics sites.
//!
//! These tools are MCP-only (not exposed to the in-app chat agent).
//! Coding agents like Claude Code use them to manage analytics sites
//! from the IDE without switching to the web UI.

use async_trait::async_trait;
use serde_json::json;

use kyomi_auth::analytics_site_service;

use crate::catalog::indexing_service::CatalogIndexingService;
use crate::tools::{AgentTool, ToolContext};
use crate::types::ToolAnnotations;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Format a site as a JSON value for tool output.
fn site_to_json(site: &analytics_site_service::AnalyticsSite) -> serde_json::Value {
    json!({
        "name": site.name,
        "site_id": site.site_id,
        "allowed_domains": site.allowed_domains,
        "snippet": analytics_site_service::snippet_tag(&site.signed_key),
        "datasource_id": site.datasource_id,
        "datasource_slug": site.datasource_slug,
        "created_at": site.created_at.to_rfc3339(),
        "updated_at": site.updated_at.to_rfc3339(),
    })
}

/// Check admin role, returning a user-friendly error string if not admin.
fn require_admin(ctx: &ToolContext) -> Result<(), String> {
    if !ctx.is_workspace_admin() {
        Err("Error: Workspace admin access required. Only workspace admins can manage analytics sites.".into())
    } else {
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// ListAnalyticsSitesTool
// ---------------------------------------------------------------------------

pub struct ListAnalyticsSitesTool;

#[async_trait]
impl AgentTool for ListAnalyticsSitesTool {
    fn name(&self) -> &str {
        "list_analytics_sites"
    }

    fn description(&self) -> &str {
        "List all analytics sites in the current workspace. Returns site details \
         including tracking snippets and linked datasource slugs."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {},
            "required": []
        })
    }

    fn is_mcp_only(&self) -> bool {
        true
    }

    fn annotations(&self) -> Option<ToolAnnotations> {
        Some(ToolAnnotations {
            read_only_hint: Some(true),
            ..Default::default()
        })
    }

    async fn execute(
        &self,
        _args: serde_json::Value,
        ctx: &ToolContext,
    ) -> kyomi_core::Result<String> {
        let sites = analytics_site_service::list_sites(&ctx.db, &ctx.workspace_id).await?;

        if sites.is_empty() {
            return Ok(json!({
                "sites": [],
                "message": "No analytics sites configured. Use create_analytics_site to set one up."
            }).to_string());
        }

        let sites_json: Vec<serde_json::Value> = sites.iter().map(site_to_json).collect();
        Ok(json!({ "sites": sites_json }).to_string())
    }
}

// ---------------------------------------------------------------------------
// CreateAnalyticsSiteTool
// ---------------------------------------------------------------------------

pub struct CreateAnalyticsSiteTool;

#[async_trait]
impl AgentTool for CreateAnalyticsSiteTool {
    fn name(&self) -> &str {
        "create_analytics_site"
    }

    fn description(&self) -> &str {
        "Create an analytics site for tracking website events. Returns a <script> snippet \
         to embed in the site's HTML <head>. Automatically provisions a queryable ClickHouse \
         datasource that you can use with query_datasource, create_dashboard, and other tools."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Human-readable site name (e.g., 'My Website', 'Docs Site')"
                },
                "allowed_domains": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Domains allowed to send analytics events (e.g., ['example.com', 'app.example.com']). Subdomains of listed domains are automatically allowed."
                },
                "datasource_slug": {
                    "type": "string",
                    "description": "Optional slug for the auto-provisioned datasource (e.g., 'my-site-analytics'). Defaults to '{name-slugified}-analytics'."
                }
            },
            "required": ["name", "allowed_domains"]
        })
    }

    fn is_mcp_only(&self) -> bool {
        true
    }

    fn annotations(&self) -> Option<ToolAnnotations> {
        Some(ToolAnnotations {
            read_only_hint: Some(false),
            ..Default::default()
        })
    }

    async fn execute(
        &self,
        args: serde_json::Value,
        ctx: &ToolContext,
    ) -> kyomi_core::Result<String> {
        if let Err(msg) = require_admin(ctx) {
            return Ok(msg);
        }

        let name = args.get("name").and_then(|v| v.as_str()).ok_or_else(|| {
            kyomi_core::Error::BadRequest("Missing required parameter 'name'".into())
        })?;

        let name = name.trim();
        if name.is_empty() || name.len() > 255 {
            return Ok("Error: Site name must be 1-255 characters.".into());
        }

        let allowed_domains: Vec<String> = args
            .get("allowed_domains")
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().filter_map(|e| e.as_str().map(String::from)).collect())
            .unwrap_or_default();

        if allowed_domains.is_empty() {
            return Ok("Error: allowed_domains is required and must contain at least one domain.".into());
        }

        let datasource_slug = args.get("datasource_slug").and_then(|v| v.as_str());

        if ctx.config.analytics_signing_secret.is_empty() {
            return Ok("Error: Analytics is not configured on this instance.".into());
        }

        let site = analytics_site_service::create_site(analytics_site_service::CreateSiteParams {
            db: &ctx.db,
            workspace_id: &ctx.workspace_id,
            name,
            domains: &allowed_domains,
            secret: &ctx.config.analytics_signing_secret,
            datasource_slug,
            clickhouse: analytics_site_service::ClickHouseProvisioning {
                host: &ctx.config.analytics_clickhouse_host,
                port: ctx.config.analytics_clickhouse_port,
                admin_password: &ctx.config.analytics_clickhouse_password,
                secure: ctx.config.analytics_clickhouse_secure,
            },
        })
        .await?;

        // Spawn background quota sync + catalog indexing
        if let Some(ref datasource_id) = site.datasource_id {
            #[derive(sqlx::FromRow)]
            struct TierRow {
                subscription_tier: String,
            }
            let tier = kyomi_core::db_fetch_optional!(
                ctx.db, TierRow,
                "SELECT subscription_tier FROM workspaces WHERE workspace_id = $1",
                &ctx.workspace_id
            )
            .ok()
            .flatten()
            .map(|row| row.subscription_tier)
            .unwrap_or_else(|| "free".to_string());

            CatalogIndexingService::spawn_analytics_post_create(
                ctx.db.clone(),
                None, // raw Redis not available via ToolContext; quota sync skipped
                ctx.encryption_key.clone(),
                ctx.embedding.clone(),
                ctx.workspace_id.clone(),
                datasource_id.clone(),
                tier,
            );
        }

        let mut result = site_to_json(&site);
        result["message"] = json!(
            "Analytics site created. Add the snippet to your site's <head> tag to start collecting events."
        );
        Ok(result.to_string())
    }
}

// ---------------------------------------------------------------------------
// UpdateAnalyticsSiteTool
// ---------------------------------------------------------------------------

pub struct UpdateAnalyticsSiteTool;

#[async_trait]
impl AgentTool for UpdateAnalyticsSiteTool {
    fn name(&self) -> &str {
        "update_analytics_site"
    }

    fn description(&self) -> &str {
        "Update an analytics site's name, allowed domains, and/or datasource slug. \
         If domains change, the tracking snippet is regenerated (the old snippet stops working)."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "site_id": {
                    "type": "string",
                    "description": "The 16-character hex site identifier (from list_analytics_sites)."
                },
                "name": {
                    "type": "string",
                    "description": "New site name (optional)."
                },
                "allowed_domains": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "New list of allowed domains (optional). Replaces the existing list entirely."
                },
                "datasource_slug": {
                    "type": "string",
                    "description": "New slug for the auto-provisioned datasource (optional). Changes the identifier used to query the datasource."
                }
            },
            "required": ["site_id"]
        })
    }

    fn is_mcp_only(&self) -> bool {
        true
    }

    fn annotations(&self) -> Option<ToolAnnotations> {
        Some(ToolAnnotations {
            destructive_hint: Some(true),
            ..Default::default()
        })
    }

    async fn execute(
        &self,
        args: serde_json::Value,
        ctx: &ToolContext,
    ) -> kyomi_core::Result<String> {
        if let Err(msg) = require_admin(ctx) {
            return Ok(msg);
        }

        let site_id = args.get("site_id").and_then(|v| v.as_str()).ok_or_else(|| {
            kyomi_core::Error::BadRequest("Missing required parameter 'site_id'".into())
        })?;

        // Look up the site by site_id to get the UUID
        let existing = analytics_site_service::get_site_by_site_id(
            &ctx.db, site_id, &ctx.workspace_id,
        )
        .await?
        .ok_or_else(|| {
            kyomi_core::Error::NotFound(format!("Analytics site with site_id '{site_id}' not found"))
        })?;

        let name = args.get("name").and_then(|v| v.as_str()).map(|n| n.trim().to_string());
        let allowed_domains: Option<Vec<String>> = args
            .get("allowed_domains")
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().filter_map(|e| e.as_str().map(String::from)).collect());
        let datasource_slug = args.get("datasource_slug").and_then(|v| v.as_str()).map(|s| s.trim().to_string());

        if name.is_none() && allowed_domains.is_none() && datasource_slug.is_none() {
            return Ok("Error: Provide at least one of 'name', 'allowed_domains', or 'datasource_slug' to update.".into());
        }

        if let Some(ref n) = name
            && (n.is_empty() || n.len() > 255)
        {
            return Ok("Error: Site name must be 1-255 characters.".into());
        }

        if let Some(ref domains) = allowed_domains
            && domains.is_empty()
        {
            return Ok("Error: allowed_domains must contain at least one domain.".into());
        }

        if allowed_domains.is_some() && ctx.config.analytics_signing_secret.is_empty() {
            return Ok("Error: Analytics is not configured on this instance.".into());
        }

        let site = analytics_site_service::update_site(
            &ctx.db,
            &existing.id,
            &ctx.workspace_id,
            name.as_deref(),
            allowed_domains.as_deref(),
            &ctx.config.analytics_signing_secret,
            datasource_slug.as_deref(),
        )
        .await?;

        let mut result = site_to_json(&site);
        if allowed_domains.is_some() {
            result["message"] = json!(
                "Site updated. Domains changed — the tracking snippet has been regenerated. \
                 Update the snippet in your HTML."
            );
        } else {
            result["message"] = json!("Site updated.");
        }
        Ok(result.to_string())
    }
}

// ---------------------------------------------------------------------------
// DeleteAnalyticsSiteTool
// ---------------------------------------------------------------------------

pub struct DeleteAnalyticsSiteTool;

#[async_trait]
impl AgentTool for DeleteAnalyticsSiteTool {
    fn name(&self) -> &str {
        "delete_analytics_site"
    }

    fn description(&self) -> &str {
        "Delete an analytics site and its auto-provisioned datasource. \
         This removes the ClickHouse user, row policy, and all associated dashboards/watches. \
         This action cannot be undone."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "site_id": {
                    "type": "string",
                    "description": "The 16-character hex site identifier (from list_analytics_sites)."
                }
            },
            "required": ["site_id"]
        })
    }

    fn is_mcp_only(&self) -> bool {
        true
    }

    fn annotations(&self) -> Option<ToolAnnotations> {
        Some(ToolAnnotations {
            destructive_hint: Some(true),
            ..Default::default()
        })
    }

    async fn execute(
        &self,
        args: serde_json::Value,
        ctx: &ToolContext,
    ) -> kyomi_core::Result<String> {
        if let Err(msg) = require_admin(ctx) {
            return Ok(msg);
        }

        let site_id = args.get("site_id").and_then(|v| v.as_str()).ok_or_else(|| {
            kyomi_core::Error::BadRequest("Missing required parameter 'site_id'".into())
        })?;

        // Look up the site by site_id to get the UUID
        let existing = analytics_site_service::get_site_by_site_id(
            &ctx.db, site_id, &ctx.workspace_id,
        )
        .await?
        .ok_or_else(|| {
            kyomi_core::Error::NotFound(format!("Analytics site with site_id '{site_id}' not found"))
        })?;

        analytics_site_service::delete_site(
            &ctx.db,
            &existing.id,
            &ctx.workspace_id,
            &ctx.config.analytics_clickhouse_host,
            ctx.config.analytics_clickhouse_port,
            &ctx.config.analytics_clickhouse_password,
            ctx.config.analytics_clickhouse_secure,
        )
        .await?;

        Ok(json!({
            "success": true,
            "message": format!(
                "Analytics site '{}' (site_id: {}) deleted. The associated datasource and ClickHouse user have been removed.",
                existing.name, site_id
            ),
        }).to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::AgentTool;

    /// Load shared constants needed by snippet_tag.
    ///
    /// Uses `load_with_fallback` so the call works regardless of the current
    /// working directory when `cargo test` is invoked: if no `constants.toml`
    /// is found on disk, the embedded copy compiled into `kyomi-core` is used.
    /// The underlying `OnceLock` makes subsequent calls a no-op, so this is
    /// safe to call from every test that needs constants.
    fn load_constants_for_test() {
        let _ = kyomi_core::constants::load_with_fallback();
    }

    // -----------------------------------------------------------------------
    // Tool metadata tests
    // -----------------------------------------------------------------------

    #[test]
    fn list_tool_is_mcp_only_and_read_only() {
        let tool = ListAnalyticsSitesTool;
        assert_eq!(tool.name(), "list_analytics_sites");
        assert!(tool.is_mcp_only());
        assert!(!tool.is_copilot_only());
        let ann = tool.annotations().unwrap();
        assert_eq!(ann.read_only_hint, Some(true));
    }

    #[test]
    fn create_tool_is_mcp_only_and_not_read_only() {
        let tool = CreateAnalyticsSiteTool;
        assert_eq!(tool.name(), "create_analytics_site");
        assert!(tool.is_mcp_only());
        let ann = tool.annotations().unwrap();
        assert_eq!(ann.read_only_hint, Some(false));
    }

    #[test]
    fn update_tool_is_mcp_only_and_destructive() {
        let tool = UpdateAnalyticsSiteTool;
        assert_eq!(tool.name(), "update_analytics_site");
        assert!(tool.is_mcp_only());
        let ann = tool.annotations().unwrap();
        assert_eq!(ann.destructive_hint, Some(true));
    }

    #[test]
    fn delete_tool_is_mcp_only_and_destructive() {
        let tool = DeleteAnalyticsSiteTool;
        assert_eq!(tool.name(), "delete_analytics_site");
        assert!(tool.is_mcp_only());
        let ann = tool.annotations().unwrap();
        assert_eq!(ann.destructive_hint, Some(true));
    }

    // -----------------------------------------------------------------------
    // Parameter schema validation tests
    // -----------------------------------------------------------------------

    #[test]
    fn list_tool_has_no_required_params() {
        let tool = ListAnalyticsSitesTool;
        let schema = tool.parameters_schema();
        let required = schema["required"].as_array().unwrap();
        assert!(required.is_empty());
    }

    #[test]
    fn create_tool_requires_name_and_domains() {
        let tool = CreateAnalyticsSiteTool;
        let schema = tool.parameters_schema();
        let required: Vec<&str> = schema["required"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert!(required.contains(&"name"));
        assert!(required.contains(&"allowed_domains"));
        // datasource_slug should be optional
        assert!(!required.contains(&"datasource_slug"));
    }

    #[test]
    fn update_tool_requires_site_id() {
        let tool = UpdateAnalyticsSiteTool;
        let schema = tool.parameters_schema();
        let required: Vec<&str> = schema["required"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert!(required.contains(&"site_id"));
        assert!(!required.contains(&"name"));
        assert!(!required.contains(&"allowed_domains"));
    }

    #[test]
    fn delete_tool_requires_site_id() {
        let tool = DeleteAnalyticsSiteTool;
        let schema = tool.parameters_schema();
        let required: Vec<&str> = schema["required"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert_eq!(required, vec!["site_id"]);
    }

    // -----------------------------------------------------------------------
    // site_to_json helper tests
    // -----------------------------------------------------------------------

    #[test]
    fn site_to_json_includes_snippet() {
        load_constants_for_test();
        let site = analytics_site_service::AnalyticsSite {
            id: uuid::Uuid::new_v4().to_string(),
            workspace_id: "ws-test".into(),
            name: "Test Site".into(),
            site_id: "abcd1234abcd1234".into(),
            allowed_domains: vec!["example.com".into()],
            signed_key: "payload.signature".into(),
            datasource_id: Some("ds-123".into()),
            clickhouse_database: Some("site_ws_test_abcd1234abcd1234".into()),
            datasource_slug: Some("test-analytics".into()),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        };

        let json = site_to_json(&site);
        assert_eq!(json["name"], "Test Site");
        assert_eq!(json["site_id"], "abcd1234abcd1234");
        assert!(json["snippet"].as_str().unwrap().contains("data-key=\"payload.signature\""));
        assert_eq!(json["datasource_slug"], "test-analytics");
    }

    #[test]
    fn site_to_json_handles_null_datasource() {
        load_constants_for_test();
        let site = analytics_site_service::AnalyticsSite {
            id: uuid::Uuid::new_v4().to_string(),
            workspace_id: "ws-test".into(),
            name: "No DS".into(),
            site_id: "ef012345ef012345".into(),
            allowed_domains: vec!["test.com".into()],
            signed_key: "k.s".into(),
            datasource_id: None,
            clickhouse_database: None,
            datasource_slug: None,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        };

        let json = site_to_json(&site);
        assert!(json["datasource_id"].is_null());
        assert!(json["datasource_slug"].is_null());
    }

    // -----------------------------------------------------------------------
    // Registry integration tests
    // -----------------------------------------------------------------------

    #[test]
    fn analytics_tools_registered_in_default_registry() {
        let registry = crate::tools::create_default_registry();
        assert!(registry.get_tool("list_analytics_sites").is_some());
        assert!(registry.get_tool("create_analytics_site").is_some());
        assert!(registry.get_tool("update_analytics_site").is_some());
        assert!(registry.get_tool("delete_analytics_site").is_some());
    }

    #[test]
    fn analytics_tools_excluded_when_mcp_only_filtered() {
        let registry = crate::tools::create_default_registry();
        let filter = crate::tools::ToolFilter {
            exclude_mcp_only: true,
            ..Default::default()
        };
        let tools = registry.get_tools(&filter);
        let tool_names: Vec<&str> = tools.iter().map(|t| t.name()).collect();
        assert!(!tool_names.contains(&"list_analytics_sites"));
        assert!(!tool_names.contains(&"create_analytics_site"));
        assert!(!tool_names.contains(&"update_analytics_site"));
        assert!(!tool_names.contains(&"delete_analytics_site"));
    }
}
