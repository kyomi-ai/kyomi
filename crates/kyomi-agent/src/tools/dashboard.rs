// SPDX-License-Identifier: AGPL-3.0-or-later

//! Dashboard tools — search, get, create, modify, and delete dashboards.

use async_trait::async_trait;

use kyomi_auth::websocket::helpers as ws_helpers;

use crate::tools::{AgentTool, ToolContext};
use crate::types::ToolAnnotations;

// ---------------------------------------------------------------------------
// SearchDashboardsTool
// ---------------------------------------------------------------------------

/// Search for dashboards in the current workspace.
pub struct SearchDashboardsTool;

#[async_trait]
impl AgentTool for SearchDashboardsTool {
    fn name(&self) -> &str {
        "search_dashboards"
    }

    fn description(&self) -> &str {
        "Search for dashboards in the current workspace. Supports sorting by \
         popularity, recency, or creation date. Use this to find existing \
         dashboards, check for duplicates before creating, or help users locate \
         specific dashboards. Can return top 10 most popular dashboards."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Search query to filter dashboards by title or content"
                },
                "sort_by": {
                    "type": "string",
                    "description": "Sort order: 'popularity' (default), 'recent' (last updated), 'created' (newest first)",
                    "default": "popularity",
                    "enum": ["popularity", "recent", "created"]
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of results to return (default: 10)",
                    "default": 10
                },
                "top_popular": {
                    "type": "boolean",
                    "description": "If true, returns top 10 most popular dashboards (ignores limit)",
                    "default": false
                },
                "doc_type": {
                    "type": "string",
                    "description": "Filter by document type: 'dashboard', 'knowledge', or omit for all",
                    "enum": ["dashboard", "knowledge"]
                }
            },
            "required": []
        })
    }

    fn annotations(&self) -> Option<ToolAnnotations> {
        Some(ToolAnnotations {
            read_only_hint: Some(true),
            ..Default::default()
        })
    }

    async fn execute(
        &self,
        args: serde_json::Value,
        ctx: &ToolContext,
    ) -> kyomi_core::Result<String> {
        let query = args.get("query").and_then(|v| v.as_str());
        let sort_by_str = args
            .get("sort_by")
            .and_then(|v| v.as_str())
            .unwrap_or("popularity");
        let mut limit = args
            .get("limit")
            .and_then(|v| v.as_i64())
            .unwrap_or(10);
        let top_popular = args
            .get("top_popular")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let doc_type_filter = args
            .get("doc_type")
            .and_then(|v| v.as_str())
            .map(kyomi_core::models::DocType::from_str_or_default);

        if top_popular {
            limit = 10;
        }

        let sort_by = match sort_by_str {
            "recent" => kyomi_auth::dashboard_service::SearchSort::Recent,
            "created" => kyomi_auth::dashboard_service::SearchSort::Created,
            _ => kyomi_auth::dashboard_service::SearchSort::Popularity,
        };

        let results = kyomi_auth::dashboard_service::search_dashboards(
            &ctx.db,
            &ctx.workspace_id,
            query,
            doc_type_filter,
            sort_by,
            limit,
        )
        .await?;

        let total_workspace_dashboards =
            kyomi_auth::dashboard_service::get_dashboard_count(&ctx.db, &ctx.workspace_id, None)
                .await?;

        let frontend_url = &ctx.config.frontend_url;

        let dashboards: Vec<serde_json::Value> = results
            .iter()
            .map(|d| {
                serde_json::json!({
                    "dashboard_id": d.dashboard_id,
                    "url": format!("{frontend_url}/dashboard/{}", d.dashboard_id),
                    "title": d.title,
                    "doc_type": d.doc_type,
                    "content": d.content_preview,
                    "created_at": d.created_at.to_rfc3339(),
                    "updated_at": d.updated_at.to_rfc3339(),
                    "last_change_summary": d.last_change_summary,
                    "total_views": d.view_count,
                    "recent_views": d.recent_views,
                    "popularity_score": format!("{:.2}", d.popularity_score),
                })
            })
            .collect();

        let count = dashboards.len();

        Ok(serde_json::json!({
            "dashboards": dashboards,
            "count": count,
            "total_workspace_dashboards": total_workspace_dashboards,
            "sorted_by": sort_by_str,
        })
        .to_string())
    }
}

// ---------------------------------------------------------------------------
// GetDashboardInfoTool
// ---------------------------------------------------------------------------

/// Get detailed information about a specific dashboard.
pub struct GetDashboardInfoTool;

#[async_trait]
impl AgentTool for GetDashboardInfoTool {
    fn name(&self) -> &str {
        "get_dashboard_info"
    }

    fn description(&self) -> &str {
        "Get detailed information about a specific dashboard including full content."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "dashboard_id": {
                    "type": "string",
                    "description": "The dashboard ID to retrieve"
                }
            },
            "required": ["dashboard_id"]
        })
    }

    fn annotations(&self) -> Option<ToolAnnotations> {
        Some(ToolAnnotations {
            read_only_hint: Some(true),
            ..Default::default()
        })
    }

    async fn execute(
        &self,
        args: serde_json::Value,
        ctx: &ToolContext,
    ) -> kyomi_core::Result<String> {
        let dashboard_id = args
            .get("dashboard_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                kyomi_core::Error::BadRequest(
                    "Missing required parameter 'dashboard_id'".into(),
                )
            })?;

        let dashboard = kyomi_auth::dashboard_service::get_dashboard(
            &ctx.db,
            dashboard_id,
            &ctx.workspace_id,
        )
        .await?;

        let dashboard = match dashboard {
            Some(d) => d,
            None => {
                return Ok(serde_json::json!({
                    "error": format!("Dashboard not found: {dashboard_id}")
                })
                .to_string());
            }
        };

        // Record the view for popularity tracking (fire-and-forget style error handling)
        let _ = kyomi_auth::dashboard_service::record_view(
            &ctx.db,
            dashboard_id,
            &ctx.user_id,
            &ctx.workspace_id,
        )
        .await;

        let frontend_url = &ctx.config.frontend_url;

        Ok(serde_json::json!({
            "success": true,
            "dashboard_id": dashboard.dashboard_id,
            "url": format!("{frontend_url}/dashboard/{}", dashboard.dashboard_id),
            "title": dashboard.title,
            "content": dashboard.content,
            "created_at": dashboard.created_at.to_rfc3339(),
            "updated_at": dashboard.updated_at.to_rfc3339(),
            "last_change_summary": dashboard.last_change_summary,
            "message": format!("Retrieved dashboard '{}'", dashboard.title),
        })
        .to_string())
    }
}

// ---------------------------------------------------------------------------
// CreateDashboardTool
// ---------------------------------------------------------------------------

/// Create a new dashboard in the workspace.
pub struct CreateDashboardTool;

#[async_trait]
impl AgentTool for CreateDashboardTool {
    fn name(&self) -> &str {
        "create_dashboard"
    }

    fn description(&self) -> &str {
        "Create a new dashboard. You MUST search for existing dashboards first \
         using search_dashboards to avoid duplicates. Use get_chartml_spec to \
         check ChartML syntax before creating dashboards with charts. ChartML \
         blocks are validated before saving - invalid syntax will return \
         detailed errors."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "title": {
                    "type": "string",
                    "description": "Dashboard title (3-255 characters)"
                },
                "content": {
                    "type": "string",
                    "description": "Dashboard content in markdown format with optional ChartML blocks.",
                    "default": ""
                },
                "verified_no_duplicates": {
                    "type": "boolean",
                    "description": "Set to true to confirm you've searched for duplicates. Must be true to create dashboard."
                }
            },
            "required": ["title", "verified_no_duplicates"]
        })
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
        let title = args
            .get("title")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                kyomi_core::Error::BadRequest("Missing required parameter 'title'".into())
            })?;
        let content = args
            .get("content")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let verified = args
            .get("verified_no_duplicates")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        if !verified {
            return Ok(serde_json::json!({
                "error": "You must confirm that you've searched for existing dashboards \
                          before creating a new one. Use the search_dashboards tool first \
                          to check for duplicates."
            })
            .to_string());
        }

        // Validate SQL in ChartML blocks before saving (skip for trial mode).
        if !ctx.is_trial
            && let Some(sql_errors) =
                super::query_utils::validate_chartml_sql(&ctx.query_context(), content).await
        {
            return Ok(serde_json::json!({
                "success": false,
                "error": format!("Dashboard contains invalid SQL: {sql_errors}"),
                "validation_errors": [sql_errors],
            })
            .to_string());
        }

        let dashboard_id = match kyomi_auth::dashboard_service::create_dashboard(
            &ctx.db,
            &ctx.user_id,
            &ctx.workspace_id,
            title,
            content,
            kyomi_core::models::DocType::Dashboard,
        )
        .await
        {
            Ok(id) => id,
            Err(kyomi_core::Error::Forbidden(msg)) => {
                return Ok(serde_json::json!({
                    "success": false,
                    "error": msg,
                    "upgrade_required": true,
                })
                .to_string());
            }
            Err(kyomi_core::Error::BadRequest(msg)) => {
                return Ok(serde_json::json!({
                    "success": false,
                    "error": msg,
                    "validation_errors": [msg],
                })
                .to_string());
            }
            Err(e) => return Err(e),
        };

        // Spawn background embedding generation for non-trivial content
        if content.len() >= 50 {
            kyomi_auth::dashboard_service::spawn_embedding_generation(
                ctx.db.clone(),
                ctx.embedding.wait_ready().await?.clone(),
                dashboard_id.clone(),
                ctx.workspace_id.clone(),
                title.trim().to_string(),
                content.to_string(),
            );
        }

        // Broadcast dashboard creation to workspace members.
        ws_helpers::send_dashboard_update(
            &ctx.ws_manager,
            &ctx.workspace_id,
            &dashboard_id,
            "created",
            &ctx.user_id,
            &ctx.user_display_name,
            Some(&ctx.user_id),
        )
        .await;

        let frontend_url = &ctx.config.frontend_url;

        Ok(serde_json::json!({
            "success": true,
            "dashboard_id": dashboard_id,
            "url": format!("{frontend_url}/dashboard/{dashboard_id}"),
            "title": title.trim(),
            "message": format!(
                "Created dashboard '{}'. Use dashboard_id '{}' to reference it.",
                title.trim(),
                dashboard_id
            ),
        })
        .to_string())
    }
}

// ---------------------------------------------------------------------------
// ModifyDashboardTool
// ---------------------------------------------------------------------------

/// Update an existing dashboard's title and/or content.
pub struct ModifyDashboardTool;

#[async_trait]
impl AgentTool for ModifyDashboardTool {
    fn name(&self) -> &str {
        "modify_dashboard"
    }

    fn description(&self) -> &str {
        "Update an existing dashboard's title and/or content. You must be the \
         owner to update. Use get_chartml_spec to verify ChartML syntax before \
         updating. ChartML blocks are validated before saving - invalid syntax \
         will return detailed errors."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "dashboard_id": {
                    "type": "string",
                    "description": "Dashboard ID to update"
                },
                "title": {
                    "type": "string",
                    "description": "New dashboard title (3-255 characters)"
                },
                "content": {
                    "type": "string",
                    "description": "New dashboard content in markdown format."
                },
                "change_summary": {
                    "type": "string",
                    "description": "Optional summary of changes (auto-generated if not provided)"
                }
            },
            "required": ["dashboard_id"]
        })
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
        let dashboard_id = args
            .get("dashboard_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                kyomi_core::Error::BadRequest(
                    "Missing required parameter 'dashboard_id'".into(),
                )
            })?;
        let title = args.get("title").and_then(|v| v.as_str());
        let content = args.get("content").and_then(|v| v.as_str());
        let change_summary = args.get("change_summary").and_then(|v| v.as_str());

        if title.is_none() && content.is_none() {
            return Ok(serde_json::json!({
                "error": "At least one of 'title' or 'content' must be provided"
            })
            .to_string());
        }

        // Validate SQL in ChartML blocks before saving (skip for trial mode).
        if !ctx.is_trial
            && let Some(c) = content
            && let Some(sql_errors) =
                super::query_utils::validate_chartml_sql(&ctx.query_context(), c).await
        {
            return Ok(serde_json::json!({
                "success": false,
                "error": format!("Dashboard contains invalid SQL: {sql_errors}"),
                "validation_errors": [sql_errors],
            })
            .to_string());
        }

        match kyomi_auth::dashboard_service::update_dashboard(
            &ctx.db,
            None, // embed: no rechunking from agent tool (yet)
            dashboard_id,
            &ctx.workspace_id,
            &ctx.user_id,
            title,
            content,
            change_summary,
            None, // expected_content_hash: no CAS for agent tool
        )
        .await
        {
            Ok(updated) => {
                if !updated {
                    return Ok(serde_json::json!({
                        "error": format!("Dashboard not found: {dashboard_id}")
                    })
                    .to_string());
                }
            }
            Err(kyomi_core::Error::NotFound(msg)) => {
                return Ok(serde_json::json!({ "error": msg }).to_string());
            }
            Err(kyomi_core::Error::Forbidden(msg)) => {
                return Ok(serde_json::json!({ "error": msg }).to_string());
            }
            Err(kyomi_core::Error::BadRequest(msg)) => {
                return Ok(serde_json::json!({
                    "success": false,
                    "error": msg,
                    "validation_errors": [msg],
                })
                .to_string());
            }
            Err(e) => return Err(e),
        }

        // Spawn background embedding if content changed and is substantial
        if let Some(c) = content
            && c.len() >= 50
        {
            let effective_title = title
                .map(|t| t.trim().to_string())
                .unwrap_or_default();
            kyomi_auth::dashboard_service::spawn_embedding_generation(
                ctx.db.clone(),
                ctx.embedding.wait_ready().await?.clone(),
                dashboard_id.to_string(),
                ctx.workspace_id.clone(),
                effective_title,
                c.to_string(),
            );
        }

        // Broadcast dashboard update to workspace members.
        ws_helpers::send_dashboard_update(
            &ctx.ws_manager,
            &ctx.workspace_id,
            dashboard_id,
            "updated",
            &ctx.user_id,
            &ctx.user_display_name,
            Some(&ctx.user_id),
        )
        .await;

        let frontend_url = &ctx.config.frontend_url;
        let display_title = title.map(|t| t.trim()).unwrap_or("(unchanged)");

        Ok(serde_json::json!({
            "success": true,
            "dashboard_id": dashboard_id,
            "url": format!("{frontend_url}/dashboard/{dashboard_id}"),
            "title": display_title,
            "message": format!("Updated dashboard '{display_title}'"),
        })
        .to_string())
    }
}

// ---------------------------------------------------------------------------
// DeleteDashboardTool
// ---------------------------------------------------------------------------

/// Delete a dashboard. Only the owner can delete.
pub struct DeleteDashboardTool;

#[async_trait]
impl AgentTool for DeleteDashboardTool {
    fn name(&self) -> &str {
        "delete_dashboard"
    }

    fn description(&self) -> &str {
        "Delete a dashboard. You must be the owner to delete. This action \
         cannot be undone."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "dashboard_id": {
                    "type": "string",
                    "description": "Dashboard ID to delete"
                }
            },
            "required": ["dashboard_id"]
        })
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
        let dashboard_id = args
            .get("dashboard_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                kyomi_core::Error::BadRequest(
                    "Missing required parameter 'dashboard_id'".into(),
                )
            })?;

        match kyomi_auth::dashboard_service::delete_dashboard(
            &ctx.db,
            dashboard_id,
            &ctx.workspace_id,
            &ctx.user_id,
        )
        .await
        {
            Ok(_) => {
                // Broadcast dashboard deletion to workspace members.
                ws_helpers::send_dashboard_update(
                    &ctx.ws_manager,
                    &ctx.workspace_id,
                    dashboard_id,
                    "deleted",
                    &ctx.user_id,
                    &ctx.user_display_name,
                    Some(&ctx.user_id),
                )
                .await;

                Ok(serde_json::json!({
                    "success": true,
                    "dashboard_id": dashboard_id,
                    "message": format!("Deleted dashboard '{dashboard_id}'"),
                })
                .to_string())
            }
            Err(kyomi_core::Error::NotFound(msg)) => {
                Ok(serde_json::json!({ "error": msg }).to_string())
            }
            Err(kyomi_core::Error::Forbidden(msg)) => {
                Ok(serde_json::json!({ "error": msg }).to_string())
            }
            Err(e) => Err(e),
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- SearchDashboardsTool ------------------------------------------------

    #[test]
    fn search_dashboards_name() {
        assert_eq!(SearchDashboardsTool.name(), "search_dashboards");
    }

    #[test]
    fn search_dashboards_description_not_empty() {
        assert!(!SearchDashboardsTool.description().is_empty());
    }

    #[test]
    fn search_dashboards_schema_has_no_required_fields() {
        let schema = SearchDashboardsTool.parameters_schema();
        assert_eq!(schema["type"], "object");
        let required = schema["required"].as_array().expect("required is array");
        assert!(required.is_empty());
    }

    #[test]
    fn search_dashboards_schema_has_expected_properties() {
        let schema = SearchDashboardsTool.parameters_schema();
        let props = schema["properties"].as_object().expect("properties is object");
        assert!(props.contains_key("query"));
        assert!(props.contains_key("sort_by"));
        assert!(props.contains_key("limit"));
        assert!(props.contains_key("top_popular"));
    }

    #[test]
    fn search_dashboards_annotations_read_only() {
        let ann = SearchDashboardsTool.annotations().expect("has annotations");
        assert_eq!(ann.read_only_hint, Some(true));
        assert!(ann.destructive_hint.is_none());
    }

    #[test]
    fn search_dashboards_not_copilot_only() {
        assert!(!SearchDashboardsTool.is_copilot_only());
    }

    // -- GetDashboardInfoTool ------------------------------------------------

    #[test]
    fn get_dashboard_info_name() {
        assert_eq!(GetDashboardInfoTool.name(), "get_dashboard_info");
    }

    #[test]
    fn get_dashboard_info_description_not_empty() {
        assert!(!GetDashboardInfoTool.description().is_empty());
    }

    #[test]
    fn get_dashboard_info_schema_requires_dashboard_id() {
        let schema = GetDashboardInfoTool.parameters_schema();
        let required = schema["required"].as_array().expect("required is array");
        assert!(required.contains(&serde_json::json!("dashboard_id")));
    }

    #[test]
    fn get_dashboard_info_annotations_read_only() {
        let ann = GetDashboardInfoTool.annotations().expect("has annotations");
        assert_eq!(ann.read_only_hint, Some(true));
        assert!(ann.destructive_hint.is_none());
    }

    // -- CreateDashboardTool -------------------------------------------------

    #[test]
    fn create_dashboard_name() {
        assert_eq!(CreateDashboardTool.name(), "create_dashboard");
    }

    #[test]
    fn create_dashboard_description_not_empty() {
        assert!(!CreateDashboardTool.description().is_empty());
    }

    #[test]
    fn create_dashboard_schema_requires_title_and_verified() {
        let schema = CreateDashboardTool.parameters_schema();
        let required = schema["required"].as_array().expect("required is array");
        assert!(required.contains(&serde_json::json!("title")));
        assert!(required.contains(&serde_json::json!("verified_no_duplicates")));
    }

    #[test]
    fn create_dashboard_annotations_not_read_only() {
        let ann = CreateDashboardTool.annotations().expect("has annotations");
        assert_eq!(ann.read_only_hint, Some(false));
        assert!(ann.destructive_hint.is_none());
    }

    // -- ModifyDashboardTool -------------------------------------------------

    #[test]
    fn modify_dashboard_name() {
        assert_eq!(ModifyDashboardTool.name(), "modify_dashboard");
    }

    #[test]
    fn modify_dashboard_description_not_empty() {
        assert!(!ModifyDashboardTool.description().is_empty());
    }

    #[test]
    fn modify_dashboard_schema_requires_dashboard_id() {
        let schema = ModifyDashboardTool.parameters_schema();
        let required = schema["required"].as_array().expect("required is array");
        assert!(required.contains(&serde_json::json!("dashboard_id")));
        assert_eq!(required.len(), 1);
    }

    #[test]
    fn modify_dashboard_schema_has_optional_fields() {
        let schema = ModifyDashboardTool.parameters_schema();
        let props = schema["properties"].as_object().expect("properties is object");
        assert!(props.contains_key("title"));
        assert!(props.contains_key("content"));
        assert!(props.contains_key("change_summary"));
    }

    #[test]
    fn modify_dashboard_annotations_not_read_only() {
        let ann = ModifyDashboardTool.annotations().expect("has annotations");
        assert_eq!(ann.read_only_hint, Some(false));
        assert!(ann.destructive_hint.is_none());
    }

    // -- DeleteDashboardTool -------------------------------------------------

    #[test]
    fn delete_dashboard_name() {
        assert_eq!(DeleteDashboardTool.name(), "delete_dashboard");
    }

    #[test]
    fn delete_dashboard_description_not_empty() {
        assert!(!DeleteDashboardTool.description().is_empty());
    }

    #[test]
    fn delete_dashboard_schema_requires_dashboard_id() {
        let schema = DeleteDashboardTool.parameters_schema();
        let required = schema["required"].as_array().expect("required is array");
        assert!(required.contains(&serde_json::json!("dashboard_id")));
        assert_eq!(required.len(), 1);
    }

    #[test]
    fn delete_dashboard_annotations_destructive() {
        let ann = DeleteDashboardTool.annotations().expect("has annotations");
        assert_eq!(ann.destructive_hint, Some(true));
        assert!(ann.read_only_hint.is_none());
    }

    #[test]
    fn delete_dashboard_not_copilot_only() {
        assert!(!DeleteDashboardTool.is_copilot_only());
    }
}
