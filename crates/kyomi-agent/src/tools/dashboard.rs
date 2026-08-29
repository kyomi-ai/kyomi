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
            &ctx.user_id,
            query,
            doc_type_filter,
            sort_by,
            limit,
        )
        .await?;

        // Count documents matching the same filter the search used, so the
        // total reported to the agent is consistent with the result set
        // (e.g. filtering to doc_type=knowledge should count knowledge docs,
        // not dashboards). Scoped to ctx.user_id's visibility — see
        // get_document_count's doc comment (KYO-181).
        let total_workspace_documents = kyomi_auth::dashboard_service::get_document_count(
            &ctx.db,
            &ctx.workspace_id,
            doc_type_filter,
            &ctx.user_id,
        )
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
            "documents": dashboards,
            // Backward-compatible alias — older prompts expect `dashboards`.
            // Kept pointing at the same array until we deprecate it.
            "dashboards": dashboards,
            "count": count,
            "total_workspace_documents": total_workspace_documents,
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
            &ctx.user_id,
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

        // Validate SQL in ChartML blocks before saving.
        if let Some(sql_errors) =
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
            None, // Embedding generation handled separately below
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

        // Spawn background dashboard summary generation (only if none exists).
        if !content.is_empty()
            && kyomi_auth::dashboard_service::extract_summary(content).is_none()
        {
            crate::execution::generate_dashboard_summary(
                crate::execution::DashboardSummaryParams {
                    db: ctx.db.clone(),
                    ws_manager: ctx.ws_manager.clone(),
                    dashboard_id: dashboard_id.clone(),
                    user_id: ctx.user_id.clone(),
                    workspace_id: ctx.workspace_id.clone(),
                    title: title.trim().to_string(),
                    content: content.to_string(),
                    app_config: ctx.config.clone(),
                    doc_type: "dashboard".to_string(),
                },
            );
        }

        // Broadcast dashboard creation to workspace members.
        // Broadcast to all workspace members including the actor's other tabs —
        // same-user multi-tab sync requires this. QueryCache is stale-while-
        // revalidate so the actor's own tab refetches silently (no flash).
        ws_helpers::send_dashboard_update(
            &ctx.ws_manager,
            &ctx.workspace_id,
            &dashboard_id,
            "created",
            &ctx.user_id,
            &ctx.user_display_name,
            None,
        )
        .await;
        ws_helpers::broadcast_dashboard_sync(
            &ctx.db, &ctx.ws_manager, &dashboard_id, &ctx.workspace_id,
            kyomi_types::sync::SyncActionType::Insert,
            &ctx.user_id,
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
         owner to update. Important: to add or change dashboard content (charts, \
         text, markdown), you MUST include the 'content' parameter with the \
         complete markdown. Omitting 'content' only updates the title — it does \
         not add any content. Use get_chartml_spec to verify ChartML syntax \
         before updating. ChartML blocks are validated before saving — invalid \
         syntax will return detailed errors."
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

        // Validate SQL in ChartML blocks before saving.
        if let Some(c) = content
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

        // Reject title-only updates on empty dashboards before writing.
        // Returning success:true with a soft warning caused a 21-call loop —
        // models read "success" and repeat the same call. A hard failure breaks
        // the reinforcement cycle. Title-only renames on dashboards that already
        // have content still proceed normally below.
        if content.is_none()
            && let Ok(Some(dash)) = kyomi_auth::dashboard_service::get_dashboard(
                &ctx.db, dashboard_id, &ctx.workspace_id, &ctx.user_id,
            )
            .await
            && dash.content.trim().is_empty()
        {
            return Ok(serde_json::json!({
                "success": false,
                "error": "The dashboard is empty and no content was provided. \
                          You MUST include the 'content' parameter with the \
                          full dashboard markdown (text and ChartML blocks). \
                          Omitting 'content' only updates the title — it does \
                          not add any charts or text.",
                "dashboard_id": dashboard_id,
            })
            .to_string());
        }

        match kyomi_auth::dashboard_service::update_dashboard(
            kyomi_auth::dashboard_service::UpdateDashboardParams {
                db: &ctx.db,
                embed: None, // no rechunking from agent tool (yet)
                dashboard_id,
                workspace_id: &ctx.workspace_id,
                user_id: &ctx.user_id,
                title,
                content,
                change_summary,
                expected_content_hash: None, // no CAS for agent tool
            },
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
                effective_title.clone(),
                c.to_string(),
            );

            // Generate dashboard summary if none exists.
            if kyomi_auth::dashboard_service::extract_summary(c).is_none() {
                crate::execution::generate_dashboard_summary(
                    crate::execution::DashboardSummaryParams {
                        db: ctx.db.clone(),
                        ws_manager: ctx.ws_manager.clone(),
                        dashboard_id: dashboard_id.to_string(),
                        user_id: ctx.user_id.clone(),
                        workspace_id: ctx.workspace_id.clone(),
                        title: effective_title,
                        content: c.to_string(),
                        app_config: ctx.config.clone(),
                        doc_type: "dashboard".to_string(),
                    },
                );
            }
        }

        // Broadcast dashboard update to workspace members.
        // Broadcast to all workspace members including the actor's other tabs —
        // same-user multi-tab sync requires this. QueryCache is stale-while-
        // revalidate so the actor's own tab refetches silently (no flash).
        ws_helpers::send_dashboard_update(
            &ctx.ws_manager,
            &ctx.workspace_id,
            dashboard_id,
            "updated",
            &ctx.user_id,
            &ctx.user_display_name,
            None,
        )
        .await;
        ws_helpers::broadcast_dashboard_sync(
            &ctx.db, &ctx.ws_manager, dashboard_id, &ctx.workspace_id,
            kyomi_types::sync::SyncActionType::Update,
            &ctx.user_id,
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
                // Broadcast to all workspace members including the actor's other tabs —
                // same-user multi-tab sync requires this. QueryCache is stale-while-
                // revalidate so the actor's own tab refetches silently (no flash).
                ws_helpers::send_dashboard_update(
                    &ctx.ws_manager,
                    &ctx.workspace_id,
                    dashboard_id,
                    "deleted",
                    &ctx.user_id,
                    &ctx.user_display_name,
                    None,
                )
                .await;
                ws_helpers::broadcast_entity_delete(
                    &ctx.ws_manager, kyomi_types::sync::entity_types::DASHBOARD,
                    dashboard_id, &ctx.workspace_id,
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
    use kyomi_auth::websocket::WebSocketManager;

    use crate::test_support::{build_ctx, seed_user_and_workspace, test_pool};

    /// Insert a second user, `"user-b"`, into workspace `"ws-1"` alongside
    /// the `"user-a"` owner `seed_user_and_workspace` sets up. Used by the
    /// ownership-boundary tests below (a non-owner must never modify or
    /// delete another user's dashboard).
    async fn seed_second_user(db: &kyomi_core::DbPool) {
        let sq = match db {
            kyomi_core::DbPool::Sqlite(sq) => sq,
            kyomi_core::DbPool::Postgres(_) => unreachable!("test pool is always sqlite"),
        };
        sqlx::query("INSERT INTO users (user_id, email) VALUES ('user-b', 'b@test.local')")
            .execute(sq)
            .await
            .expect("insert user-b");
        sqlx::query(
            "INSERT INTO workspace_users (workspace_id, user_id, role, active) \
             VALUES ('ws-1', 'user-b', 'user', 1)",
        )
        .execute(sq)
        .await
        .expect("insert workspace_users user-b");
    }

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

    #[test]
    fn modify_dashboard_description_mentions_content_requirement() {
        let desc = ModifyDashboardTool.description();
        assert!(
            desc.contains("MUST include the 'content' parameter"),
            "Description should mention content requirement"
        );
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

    // =========================================================================
    // KYO-537 characterization tests — execute() behavior, happy paths and
    // every early-return failure branch.
    // =========================================================================

    // -- SearchDashboardsTool -------------------------------------------------

    #[tokio::test]
    async fn search_dashboards_execute_empty_workspace_returns_empty_results() {
        let db = test_pool().await;
        seed_user_and_workspace(&db).await;
        let ctx = build_ctx(db);

        let result = SearchDashboardsTool
            .execute(serde_json::json!({}), &ctx)
            .await
            .expect("search_dashboards execute");
        let parsed: serde_json::Value = serde_json::from_str(&result).expect("json");

        assert_eq!(parsed["count"], serde_json::json!(0), "{result}");
        assert_eq!(parsed["documents"], serde_json::json!([]), "{result}");
        assert_eq!(parsed["dashboards"], serde_json::json!([]), "{result}");
        assert_eq!(parsed["total_workspace_documents"], serde_json::json!(0), "{result}");
        assert_eq!(parsed["sorted_by"], serde_json::json!("popularity"), "{result}");
    }

    #[tokio::test]
    async fn search_dashboards_execute_returns_seeded_dashboard_with_expected_fields() {
        let db = test_pool().await;
        seed_user_and_workspace(&db).await;
        let dashboard_id = kyomi_auth::dashboard_service::create_dashboard(
            &db, "user-a", "ws-1", "Quarterly Revenue", "content", kyomi_core::models::DocType::Dashboard, None,
        )
        .await
        .expect("seed dashboard");
        let ctx = build_ctx(db);

        let result = SearchDashboardsTool
            .execute(serde_json::json!({}), &ctx)
            .await
            .expect("search_dashboards execute");
        let parsed: serde_json::Value = serde_json::from_str(&result).expect("json");

        assert_eq!(parsed["count"], serde_json::json!(1), "{result}");
        let doc = &parsed["documents"][0];
        assert_eq!(doc["dashboard_id"], serde_json::json!(dashboard_id), "{result}");
        assert_eq!(
            doc["url"],
            serde_json::json!(format!("http://localhost:5173/dashboard/{dashboard_id}")),
            "{result}"
        );
        assert_eq!(doc["title"], serde_json::json!("Quarterly Revenue"), "{result}");
        assert_eq!(doc["doc_type"], serde_json::json!("dashboard"), "{result}");
        assert_eq!(doc["total_views"], serde_json::json!(0), "{result}");
        assert_eq!(doc["recent_views"], serde_json::json!(0), "{result}");
        // Same array object referenced by both keys — the "dashboards" alias
        // documented at dashboard.rs as backward-compatible.
        assert_eq!(parsed["documents"], parsed["dashboards"], "{result}");
    }

    // -- GetDashboardInfoTool --------------------------------------------------

    #[tokio::test]
    async fn get_dashboard_info_missing_dashboard_id_is_bad_request() {
        let ctx = build_ctx(test_pool().await);
        let err = GetDashboardInfoTool
            .execute(serde_json::json!({}), &ctx)
            .await
            .expect_err("dashboard_id is required");
        assert!(matches!(err, kyomi_core::Error::BadRequest(_)), "got: {err:?}");
    }

    #[tokio::test]
    async fn get_dashboard_info_not_found_returns_error_json_not_err() {
        let ctx = build_ctx(test_pool().await);
        let result = GetDashboardInfoTool
            .execute(serde_json::json!({"dashboard_id": "nope"}), &ctx)
            .await
            .expect("a missing dashboard is a structured result, not an Err");
        let parsed: serde_json::Value = serde_json::from_str(&result).expect("json");
        assert_eq!(
            parsed,
            serde_json::json!({"error": "Dashboard not found: nope"}),
            "{result}"
        );
    }

    #[tokio::test]
    async fn get_dashboard_info_happy_path() {
        let db = test_pool().await;
        seed_user_and_workspace(&db).await;
        let dashboard_id = kyomi_auth::dashboard_service::create_dashboard(
            &db, "user-a", "ws-1", "Runbook", "full body content", kyomi_core::models::DocType::Dashboard, None,
        )
        .await
        .expect("seed dashboard");
        let ctx = build_ctx(db);

        let result = GetDashboardInfoTool
            .execute(serde_json::json!({"dashboard_id": dashboard_id}), &ctx)
            .await
            .expect("get_dashboard_info execute");
        let parsed: serde_json::Value = serde_json::from_str(&result).expect("json");

        assert_eq!(parsed["success"], serde_json::json!(true), "{result}");
        assert_eq!(parsed["dashboard_id"], serde_json::json!(dashboard_id), "{result}");
        assert_eq!(parsed["title"], serde_json::json!("Runbook"), "{result}");
        assert_eq!(parsed["content"], serde_json::json!("full body content"), "{result}");
        assert_eq!(
            parsed["message"],
            serde_json::json!("Retrieved dashboard 'Runbook'"),
            "{result}"
        );
    }

    // -- CreateDashboardTool ----------------------------------------------------

    #[tokio::test]
    async fn create_dashboard_missing_title_is_bad_request() {
        let ctx = build_ctx(test_pool().await);
        let err = CreateDashboardTool
            .execute(serde_json::json!({"verified_no_duplicates": true}), &ctx)
            .await
            .expect_err("title is required");
        assert!(matches!(err, kyomi_core::Error::BadRequest(_)), "got: {err:?}");
    }

    #[tokio::test]
    async fn create_dashboard_unverified_duplicates_returns_error_json() {
        let db = test_pool().await;
        seed_user_and_workspace(&db).await;
        let ctx = build_ctx(db);

        let result = CreateDashboardTool
            .execute(serde_json::json!({"title": "New Dashboard"}), &ctx)
            .await
            .expect("execute");
        let parsed: serde_json::Value = serde_json::from_str(&result).expect("json");

        assert!(
            parsed["error"].as_str().unwrap_or_default().contains("search_dashboards tool first"),
            "{result}"
        );

        let count = kyomi_auth::dashboard_service::get_dashboard_count(&ctx.db, "ws-1", Some("user-a"))
            .await
            .expect("count");
        assert_eq!(count, 0, "an unverified create must not write a row");
    }

    #[tokio::test]
    async fn create_dashboard_happy_path_creates_and_broadcasts() {
        let db = test_pool().await;
        seed_user_and_workspace(&db).await;
        let manager = WebSocketManager::new(None, db.clone());
        let (_conn, mut rx) = manager.connect("user-a").expect("connect user-a");
        rx.try_recv().expect("heartbeat");

        let mut ctx = build_ctx(db);
        ctx.ws_manager = manager;

        let result = CreateDashboardTool
            .execute(
                serde_json::json!({"title": "New Dashboard", "verified_no_duplicates": true}),
                &ctx,
            )
            .await
            .expect("execute");
        let parsed: serde_json::Value = serde_json::from_str(&result).expect("json");

        assert_eq!(parsed["success"], serde_json::json!(true), "{result}");
        assert_eq!(parsed["title"], serde_json::json!("New Dashboard"), "{result}");
        let dashboard_id = parsed["dashboard_id"].as_str().expect("dashboard_id").to_string();
        assert_eq!(
            parsed["url"],
            serde_json::json!(format!("http://localhost:5173/dashboard/{dashboard_id}")),
            "{result}"
        );

        let msg1 = rx.try_recv().expect("dashboard_update broadcast");
        assert!(msg1.contains("dashboard_update"), "{msg1}");
        assert!(msg1.contains("\"action\":\"created\""), "{msg1}");
        let msg2 = rx.try_recv().expect("sync_action broadcast");
        assert!(msg2.contains("sync_action"), "{msg2}");
    }

    #[tokio::test]
    async fn create_dashboard_free_tier_limit_returns_forbidden_json() {
        let db = test_pool().await;
        seed_user_and_workspace(&db).await;
        let sq = match &db {
            kyomi_core::DbPool::Sqlite(sq) => sq,
            kyomi_core::DbPool::Postgres(_) => unreachable!(),
        };
        for i in 0..5 {
            sqlx::query(
                "INSERT INTO dashboards \
                 (dashboard_id, user_id, workspace_id, title, content, doc_type, created_by, updated_by) \
                 VALUES (?, 'user-a', 'ws-1', ?, '', 'dashboard', 'user-a', 'user-a')",
            )
            .bind(format!("d-{i}"))
            .bind(format!("Existing {i}"))
            .execute(sq)
            .await
            .expect("seed pre-existing dashboard");
        }
        let ctx = build_ctx(db);

        let result = CreateDashboardTool
            .execute(
                serde_json::json!({"title": "One Too Many", "verified_no_duplicates": true}),
                &ctx,
            )
            .await
            .expect("execute");
        let parsed: serde_json::Value = serde_json::from_str(&result).expect("json");

        assert_eq!(parsed["success"], serde_json::json!(false), "{result}");
        assert_eq!(parsed["upgrade_required"], serde_json::json!(true), "{result}");
        assert!(
            parsed["error"].as_str().unwrap_or_default().contains("Free tier is limited to 5"),
            "{result}"
        );
    }

    // -- ModifyDashboardTool ------------------------------------------------

    #[tokio::test]
    async fn modify_dashboard_missing_dashboard_id_is_bad_request() {
        let ctx = build_ctx(test_pool().await);
        let err = ModifyDashboardTool
            .execute(serde_json::json!({"title": "x"}), &ctx)
            .await
            .expect_err("dashboard_id is required");
        assert!(matches!(err, kyomi_core::Error::BadRequest(_)), "got: {err:?}");
    }

    #[tokio::test]
    async fn modify_dashboard_no_title_or_content_returns_error_json() {
        let ctx = build_ctx(test_pool().await);
        let result = ModifyDashboardTool
            .execute(serde_json::json!({"dashboard_id": "d-1"}), &ctx)
            .await
            .expect("execute");
        let parsed: serde_json::Value = serde_json::from_str(&result).expect("json");
        assert_eq!(
            parsed,
            serde_json::json!({"error": "At least one of 'title' or 'content' must be provided"}),
            "{result}"
        );
    }

    /// KYO-537 named pin (ticket item 2): a title-only update against a
    /// dashboard whose content is empty must be a hard failure
    /// (`success: false`), not a soft `success: true`. The in-code comment
    /// at `dashboard.rs` ~545 records that the soft-success version of this
    /// branch caused a 21-call model loop — the model reads `success: true`
    /// and repeats the same no-op call believing it worked.
    #[tokio::test]
    async fn modify_dashboard_title_only_on_empty_dashboard_is_hard_failure() {
        let db = test_pool().await;
        seed_user_and_workspace(&db).await;
        let dashboard_id = kyomi_auth::dashboard_service::create_dashboard(
            &db, "user-a", "ws-1", "Empty Dashboard", "", kyomi_core::models::DocType::Dashboard, None,
        )
        .await
        .expect("seed empty dashboard");
        let ctx = build_ctx(db);

        let result = ModifyDashboardTool
            .execute(
                serde_json::json!({"dashboard_id": dashboard_id, "title": "Renamed"}),
                &ctx,
            )
            .await
            .expect("execute");
        let parsed: serde_json::Value = serde_json::from_str(&result).expect("json");

        assert_eq!(
            parsed["success"],
            serde_json::json!(false),
            "a title-only update on an empty dashboard must hard-fail, not soft-succeed: {result}"
        );
        assert!(
            parsed["error"].as_str().unwrap_or_default().contains("MUST include the 'content' parameter"),
            "{result}"
        );

        let dash = kyomi_auth::dashboard_service::get_dashboard(&ctx.db, &dashboard_id, "ws-1", "user-a")
            .await
            .expect("lookup")
            .expect("exists");
        assert_eq!(dash.title, "Empty Dashboard", "the rejected title change must not apply");
    }

    #[tokio::test]
    async fn modify_dashboard_title_only_on_nonempty_dashboard_succeeds() {
        let db = test_pool().await;
        seed_user_and_workspace(&db).await;
        let dashboard_id = kyomi_auth::dashboard_service::create_dashboard(
            &db, "user-a", "ws-1", "Has Content", "not empty", kyomi_core::models::DocType::Dashboard, None,
        )
        .await
        .expect("seed non-empty dashboard");
        let ctx = build_ctx(db);

        let result = ModifyDashboardTool
            .execute(
                serde_json::json!({"dashboard_id": dashboard_id, "title": "Renamed"}),
                &ctx,
            )
            .await
            .expect("execute");
        let parsed: serde_json::Value = serde_json::from_str(&result).expect("json");

        assert_eq!(
            parsed["success"],
            serde_json::json!(true),
            "the empty-dashboard hard-failure branch must not fire when content already exists: {result}"
        );
    }

    #[tokio::test]
    async fn modify_dashboard_not_found_returns_error_json() {
        let ctx = build_ctx(test_pool().await);
        let result = ModifyDashboardTool
            .execute(
                serde_json::json!({"dashboard_id": "nope", "title": "x"}),
                &ctx,
            )
            .await
            .expect("a missing dashboard is a structured result, not an Err");
        let parsed: serde_json::Value = serde_json::from_str(&result).expect("json");
        assert_eq!(
            parsed["error"],
            serde_json::json!("Dashboard nope not found"),
            "{result}"
        );
    }

    #[tokio::test]
    async fn modify_dashboard_forbidden_when_not_owner_returns_error_json() {
        let db = test_pool().await;
        seed_user_and_workspace(&db).await;
        seed_second_user(&db).await;
        let dashboard_id = kyomi_auth::dashboard_service::create_dashboard(
            &db, "user-b", "ws-1", "Owned By B", "content", kyomi_core::models::DocType::Dashboard, None,
        )
        .await
        .expect("seed dashboard owned by user-b");
        let ctx = build_ctx(db); // build_ctx defaults to user-a

        let result = ModifyDashboardTool
            .execute(
                serde_json::json!({"dashboard_id": dashboard_id, "title": "Hijacked"}),
                &ctx,
            )
            .await
            .expect("a forbidden update is a structured result, not an Err");
        let parsed: serde_json::Value = serde_json::from_str(&result).expect("json");
        assert_eq!(
            parsed["error"],
            serde_json::json!("Only the dashboard owner can update it"),
            "{result}"
        );
    }

    /// KYO-537 named pin (ticket item 5, dashboard side): `modify_dashboard`
    /// always passes `expected_content_hash: None` to `update_dashboard`
    /// (`dashboard.rs` ~578, `// no CAS for agent tool`) — unlike the
    /// knowledge-side tools (see `write_knowledge_file_conflict_on_stale_hash`
    /// / `edit_knowledge_file_conflict_on_stale_hash` in `tools/knowledge.rs`),
    /// a concurrent edit between two `modify_dashboard` calls can never be
    /// rejected as a CAS conflict — the second call always wins outright.
    /// KYO-539 is expected to change this deliberately; this test exists so
    /// that change is a visible, intentional diff against a known baseline.
    #[tokio::test]
    async fn modify_dashboard_cas_is_never_enforced() {
        let db = test_pool().await;
        seed_user_and_workspace(&db).await;
        let dashboard_id = kyomi_auth::dashboard_service::create_dashboard(
            &db, "user-a", "ws-1", "Contested", "v1", kyomi_core::models::DocType::Dashboard, None,
        )
        .await
        .expect("seed dashboard");
        let ctx = build_ctx(db);

        // First "concurrent" writer changes the content (and hence the
        // content_hash) out from under whatever the second writer read.
        let first = ModifyDashboardTool
            .execute(
                serde_json::json!({"dashboard_id": dashboard_id, "content": "v2 from writer A"}),
                &ctx,
            )
            .await
            .expect("execute");
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&first).expect("json")["success"],
            serde_json::json!(true)
        );

        // Second writer's update also succeeds outright — no stale-hash
        // rejection is possible via this tool, because it never supplies
        // expected_content_hash in the first place.
        let second = ModifyDashboardTool
            .execute(
                serde_json::json!({"dashboard_id": dashboard_id, "content": "v3 from writer B, clobbering A"}),
                &ctx,
            )
            .await
            .expect("execute");
        let parsed: serde_json::Value = serde_json::from_str(&second).expect("json");
        assert_eq!(
            parsed["success"],
            serde_json::json!(true),
            "modify_dashboard must currently never reject on a stale write: {second}"
        );

        let dash = kyomi_auth::dashboard_service::get_dashboard(&ctx.db, &dashboard_id, "ws-1", "user-a")
            .await
            .expect("lookup")
            .expect("exists");
        assert_eq!(dash.content, "v3 from writer B, clobbering A");
    }

    #[tokio::test]
    async fn modify_dashboard_happy_path_updates_and_broadcasts() {
        let db = test_pool().await;
        seed_user_and_workspace(&db).await;
        let dashboard_id = kyomi_auth::dashboard_service::create_dashboard(
            &db, "user-a", "ws-1", "Original", "v1", kyomi_core::models::DocType::Dashboard, None,
        )
        .await
        .expect("seed dashboard");

        let manager = WebSocketManager::new(None, db.clone());
        let (_conn, mut rx) = manager.connect("user-a").expect("connect user-a");
        rx.try_recv().expect("heartbeat");

        let mut ctx = build_ctx(db);
        ctx.ws_manager = manager;

        let result = ModifyDashboardTool
            .execute(
                serde_json::json!({
                    "dashboard_id": dashboard_id,
                    "title": "Renamed",
                    "content": "v2",
                }),
                &ctx,
            )
            .await
            .expect("execute");
        let parsed: serde_json::Value = serde_json::from_str(&result).expect("json");

        assert_eq!(parsed["success"], serde_json::json!(true), "{result}");
        assert_eq!(parsed["dashboard_id"], serde_json::json!(dashboard_id), "{result}");
        assert_eq!(parsed["title"], serde_json::json!("Renamed"), "{result}");
        assert_eq!(
            parsed["message"],
            serde_json::json!("Updated dashboard 'Renamed'"),
            "{result}"
        );

        let msg1 = rx.try_recv().expect("dashboard_update broadcast");
        assert!(msg1.contains("dashboard_update"), "{msg1}");
        assert!(msg1.contains("\"action\":\"updated\""), "{msg1}");
        let msg2 = rx.try_recv().expect("sync_action broadcast");
        assert!(msg2.contains("sync_action"), "{msg2}");
    }

    // -- DeleteDashboardTool --------------------------------------------------

    #[tokio::test]
    async fn delete_dashboard_missing_dashboard_id_is_bad_request() {
        let ctx = build_ctx(test_pool().await);
        let err = DeleteDashboardTool
            .execute(serde_json::json!({}), &ctx)
            .await
            .expect_err("dashboard_id is required");
        assert!(matches!(err, kyomi_core::Error::BadRequest(_)), "got: {err:?}");
    }

    #[tokio::test]
    async fn delete_dashboard_not_found_returns_error_json() {
        let ctx = build_ctx(test_pool().await);
        let result = DeleteDashboardTool
            .execute(serde_json::json!({"dashboard_id": "nope"}), &ctx)
            .await
            .expect("a missing dashboard is a structured result, not an Err");
        let parsed: serde_json::Value = serde_json::from_str(&result).expect("json");
        assert_eq!(
            parsed["error"],
            serde_json::json!("Dashboard nope not found"),
            "{result}"
        );
    }

    #[tokio::test]
    async fn delete_dashboard_forbidden_when_not_owner_returns_error_json() {
        let db = test_pool().await;
        seed_user_and_workspace(&db).await;
        seed_second_user(&db).await;
        let dashboard_id = kyomi_auth::dashboard_service::create_dashboard(
            &db, "user-b", "ws-1", "Owned By B", "content", kyomi_core::models::DocType::Dashboard, None,
        )
        .await
        .expect("seed dashboard owned by user-b");
        let ctx = build_ctx(db); // build_ctx defaults to user-a

        let result = DeleteDashboardTool
            .execute(serde_json::json!({"dashboard_id": dashboard_id}), &ctx)
            .await
            .expect("a forbidden delete is a structured result, not an Err");
        let parsed: serde_json::Value = serde_json::from_str(&result).expect("json");
        assert_eq!(
            parsed["error"],
            serde_json::json!("Only the dashboard owner can delete it"),
            "{result}"
        );

        let still_there = kyomi_auth::dashboard_service::get_dashboard(&ctx.db, &dashboard_id, "ws-1", "user-b")
            .await
            .expect("lookup");
        assert!(still_there.is_some(), "a forbidden delete must not remove the row");
    }

    #[tokio::test]
    async fn delete_dashboard_happy_path_deletes_and_broadcasts() {
        let db = test_pool().await;
        seed_user_and_workspace(&db).await;
        let dashboard_id = kyomi_auth::dashboard_service::create_dashboard(
            &db, "user-a", "ws-1", "Doomed", "content", kyomi_core::models::DocType::Dashboard, None,
        )
        .await
        .expect("seed dashboard");

        let manager = WebSocketManager::new(None, db.clone());
        let (_conn, mut rx) = manager.connect("user-a").expect("connect user-a");
        rx.try_recv().expect("heartbeat");

        let mut ctx = build_ctx(db);
        ctx.ws_manager = manager;

        let result = DeleteDashboardTool
            .execute(serde_json::json!({"dashboard_id": dashboard_id}), &ctx)
            .await
            .expect("execute");
        let parsed: serde_json::Value = serde_json::from_str(&result).expect("json");

        assert_eq!(parsed["success"], serde_json::json!(true), "{result}");
        assert_eq!(parsed["dashboard_id"], serde_json::json!(dashboard_id), "{result}");

        let msg1 = rx.try_recv().expect("dashboard_update broadcast");
        assert!(msg1.contains("dashboard_update"), "{msg1}");
        assert!(msg1.contains("\"action\":\"deleted\""), "{msg1}");
        let msg2 = rx.try_recv().expect("sync_action broadcast");
        assert!(msg2.contains("sync_action"), "{msg2}");

        let gone = kyomi_auth::dashboard_service::get_dashboard(&ctx.db, &dashboard_id, "ws-1", "user-a")
            .await
            .expect("lookup");
        assert!(gone.is_none(), "the dashboard row must actually be gone");
    }
}
