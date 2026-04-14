// SPDX-License-Identifier: AGPL-3.0-or-later

//! Tool framework for the agent system.
//!
//! Provides the [`AgentTool`] trait, [`ToolRegistry`] for managing tools,
//! [`ToolContext`] for shared state passed to tool execution, and
//! [`ToolFilter`] for filtering tools by capability.
//!
//! # Architecture
//!
//! Tools implement the [`AgentTool`] trait and are registered in a
//! [`ToolRegistry`]. The agent calls [`ToolRegistry::get_tool_definitions`]
//! to build the LLM tool list and [`ToolRegistry::get_tool`] to look up
//! tools by name when executing tool calls.

pub mod analytics;
pub mod catalog;
pub mod chart;
pub mod chart_data_resolver;
pub mod chart_palettes;
pub mod chart_renderer;
pub mod chartml;
pub mod copilot;
pub mod dashboard;
pub mod datasource;
pub mod forecast;
pub mod knowledge;
pub mod query_utils;
pub mod resources;
pub mod watch;
pub mod workspace;

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;

use kyomi_core::enums::WorkspaceRole;

use crate::types::{Tool, ToolAnnotations};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Tools only available in copilot mode (embedded in dashboards).
pub const COPILOT_ONLY_TOOLS: &[&str] = &["update_dashboard", "update_chart", "preview_watch"];

/// Tools only exposed via MCP.
pub const MCP_ONLY_TOOLS: &[&str] = &[
    "render_chart",
    "list_analytics_sites",
    "create_analytics_site",
    "update_analytics_site",
    "delete_analytics_site",
];

/// Tool names that signal the agent should stop iterating and return the
/// current response content (e.g., after writing a knowledge document).
pub const FINAL_TOOL_NAMES: &[&str] = &["write_knowledge_file"];

/// Tools available to watch execution agents (data query only, no mutations).
/// Matches Python `watch_scheduler.py` watch_tools list.
pub const WATCH_TOOLS: &[&str] = &[
    "search_knowledge",
    "get_table_info",
    "browse_catalog",
    "query_datasource",
    "list_datasources",
    "forecast_data",
    "list_knowledge_files",
    "read_knowledge_file",
];

/// Tools available to trial chat users (restricted read-only + visualization).
/// Matches Python `trial_chat.py` trial_tools list.
pub const TRIAL_CHAT_TOOLS: &[&str] = &[
    "list_datasources",
    "search_knowledge",
    "get_table_info",
    "query_datasource",
    "validate_sql",
    "get_chartml_spec",
];

// ---------------------------------------------------------------------------
// QueryContext — lightweight subset for datasource query execution
// ---------------------------------------------------------------------------

/// Minimal context for executing datasource queries.
///
/// Extracted from [`ToolContext`] so lightweight callers (e.g., email chart
/// rendering) can resolve chart data without the full agent context.
#[derive(Clone)]
pub struct QueryContext {
    /// PostgreSQL connection pool.
    pub db: kyomi_core::DbPool,
    /// ID of the user making the request.
    pub user_id: String,
    /// ID of the user's active workspace.
    pub workspace_id: String,
    /// AES-256-GCM encryption key for credential decryption.
    pub encryption_key: Arc<[u8; 32]>,
    /// Application configuration (needed for Google OAuth client credentials).
    pub config: Arc<kyomi_core::Config>,
    /// Connect registry for routing queries through Kyomi Connect instances.
    /// `None` when Connect is not available (e.g., lightweight callers).
    pub connect_registry: Option<kyomi_datasource_server::ConnectRegistry>,
}

// ---------------------------------------------------------------------------
// ToolContext
// ---------------------------------------------------------------------------

/// Shared context passed to every tool execution.
///
/// Contains database pools, user identity, and services that tools need
/// to query data, send WebSocket messages, or generate embeddings.
#[derive(Clone)]
pub struct ToolContext {
    /// PostgreSQL connection pool.
    pub db: kyomi_core::DbPool,
    /// KV store (Redis-backed or in-memory).
    pub kv: kyomi_core::KVPool,
    /// ID of the user making the request.
    pub user_id: String,
    /// ID of the user's active workspace.
    pub workspace_id: String,
    /// AES-256-GCM encryption key for credential decryption.
    pub encryption_key: Arc<[u8; 32]>,
    /// Lazy-loaded text embedding service (for semantic search).
    pub embedding: kyomi_embed::LazyEmbedding,
    /// WebSocket manager for sending real-time updates.
    pub ws_manager: kyomi_auth::websocket::WebSocketManager,
    /// Application configuration.
    pub config: Arc<kyomi_core::Config>,
    /// Whether the user is on a trial (limits certain tool behavior).
    pub is_trial: bool,
    /// Active chat session ID (if running inside a chat session).
    pub session_id: Option<String>,
    /// Whether the MCP client supports MCP Apps (structuredContent).
    /// Used by `render_chart` to decide between interactive (MCP App) and image (PNG) output.
    /// Always `false` for non-MCP contexts (chat, watch execution).
    pub supports_mcp_apps: bool,
    /// Workspace roles for the current user (for admin-gated tools).
    pub workspace_roles: Vec<WorkspaceRole>,
    /// Connect registry for routing queries through Kyomi Connect instances.
    /// `None` when Connect is not available (e.g., trial mode).
    pub connect_registry: Option<kyomi_datasource_server::ConnectRegistry>,
    /// Messaging platform registry for alert delivery and platform interactions.
    pub platforms: Arc<kyomi_core::platform::PlatformRegistry>,
    /// Display name for the current user (name or email fallback).
    /// Used for WebSocket event attribution (e.g., "changed_by_name" in dashboard updates).
    pub user_display_name: String,
}

impl ToolContext {
    /// Check if the current user is a workspace admin.
    pub fn is_workspace_admin(&self) -> bool {
        self.workspace_roles.contains(&WorkspaceRole::WorkspaceAdmin)
    }

    /// Extract a lightweight [`QueryContext`] for datasource query execution.
    pub fn query_context(&self) -> QueryContext {
        QueryContext {
            db: self.db.clone(),
            user_id: self.user_id.clone(),
            workspace_id: self.workspace_id.clone(),
            encryption_key: self.encryption_key.clone(),
            config: self.config.clone(),
            connect_registry: self.connect_registry.clone(),
        }
    }
}

// ---------------------------------------------------------------------------
// AgentTool trait
// ---------------------------------------------------------------------------

/// Trait implemented by all agent tools.
///
/// Each tool has a name, description, and JSON Schema for its parameters.
/// The [`execute`](AgentTool::execute) method performs the tool's work and
/// returns a string result for the LLM.
#[async_trait]
pub trait AgentTool: Send + Sync {
    /// Unique name of the tool (e.g., "search_catalog").
    fn name(&self) -> &str;

    /// Human-readable description of what the tool does.
    fn description(&self) -> &str;

    /// JSON Schema describing the tool's input parameters.
    fn parameters_schema(&self) -> serde_json::Value;

    /// Whether this tool is only available in copilot mode.
    fn is_copilot_only(&self) -> bool {
        false
    }

    /// Whether this tool is only exposed via MCP.
    fn is_mcp_only(&self) -> bool {
        false
    }

    /// Optional MCP-compatible annotations for the tool.
    fn annotations(&self) -> Option<ToolAnnotations> {
        None
    }

    /// Execute the tool with the given arguments and context.
    ///
    /// Returns a string result that will be sent back to the LLM as
    /// a tool result message.
    async fn execute(
        &self,
        args: serde_json::Value,
        ctx: &ToolContext,
    ) -> kyomi_core::Result<String>;
}

// ---------------------------------------------------------------------------
// ToolFilter
// ---------------------------------------------------------------------------

/// Filter criteria for selecting tools from the registry.
///
/// Used to exclude copilot-only or MCP-only tools, or to restrict
/// the tool set to a specific subset by name.
#[derive(Debug, Clone, Default)]
pub struct ToolFilter {
    /// Exclude tools marked as copilot-only.
    pub exclude_copilot_only: bool,
    /// Exclude tools marked as MCP-only.
    pub exclude_mcp_only: bool,
    /// If set, only include tools whose names are in this list.
    pub include_only: Option<Vec<String>>,
}

// ---------------------------------------------------------------------------
// ToolRegistry
// ---------------------------------------------------------------------------

/// Registry of available tools for the agent.
///
/// Stores tools by name and provides filtered access for building
/// LLM tool definitions and looking up tools during execution.
pub struct ToolRegistry {
    tools: HashMap<String, Arc<dyn AgentTool>>,
}

impl ToolRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
        }
    }

    /// Register a tool. If a tool with the same name already exists,
    /// it will be replaced.
    pub fn register(&mut self, tool: Arc<dyn AgentTool>) {
        self.tools.insert(tool.name().to_string(), tool);
    }

    /// Look up a tool by name.
    pub fn get_tool(&self, name: &str) -> Option<&Arc<dyn AgentTool>> {
        self.tools.get(name)
    }

    /// Get all tools matching the filter criteria.
    pub fn get_tools(&self, filter: &ToolFilter) -> Vec<&Arc<dyn AgentTool>> {
        self.tools
            .values()
            .filter(|tool| {
                if filter.exclude_copilot_only && tool.is_copilot_only() {
                    return false;
                }
                if filter.exclude_mcp_only && tool.is_mcp_only() {
                    return false;
                }
                if let Some(ref include_list) = filter.include_only
                    && !include_list.iter().any(|name| name == tool.name())
                {
                    return false;
                }
                true
            })
            .collect()
    }

    /// Build [`Tool`] definitions from all tools matching the filter.
    ///
    /// These definitions are passed to the LLM so it knows which tools
    /// are available to call.
    pub fn get_tool_definitions(&self, filter: &ToolFilter) -> Vec<Tool> {
        let mut tools: Vec<Tool> = self.get_tools(filter)
            .into_iter()
            .map(|tool| Tool {
                name: tool.name().to_string(),
                description: tool.description().to_string(),
                parameters: tool.parameters_schema(),
            })
            .collect();
        // Stable sort by name ensures the tool list is deterministic across
        // calls, which is required for Anthropic prompt caching (cache key
        // is a prefix hash — non-deterministic ordering invalidates the cache).
        tools.sort_by(|a, b| a.name.cmp(&b.name));
        tools
    }

    /// Return the names of all registered tools (for error messages).
    pub fn tool_names(&self) -> Vec<&str> {
        self.tools.keys().map(|s| s.as_str()).collect()
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Resolve credentials for a datasource, handling both shared-auth and personal-auth modes.
///
/// - **Shared auth** (e.g., sample datasource with `shared_credentials: true`):
///   Returns empty credentials — the factory's `resolve_shared_credentials()` extracts
///   `shared_username`/`shared_password` from `connection_config`.
/// - **Personal auth**: Decrypts per-user credentials and refreshes OAuth tokens if needed.
pub async fn resolve_credentials(
    ctx: &QueryContext,
    ds: &kyomi_core::models::datasource::DatasourceConfig,
    ds_type: &kyomi_core::datasource_registry::DatasourceType,
) -> kyomi_core::Result<serde_json::Value> {
    let is_shared = kyomi_auth::datasource_auth_service::is_shared_auth(
        ds_type.as_str(),
        &ds.connection_config,
    );

    if is_shared {
        // Shared auth: credentials live in connection_config.
        // The factory's resolve_shared_credentials() will extract them.
        //
        // Special case: BigQuery kyomi_oauth needs the user's Google OAuth token
        // from users.oauth_data (not from datasource credentials). Use centralized
        // token resolution which handles expiry checking and refresh.
        let auth_mode = ds
            .connection_config
            .get("auth_mode")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        if ds_type.as_str() == "bigquery" && auth_mode == "kyomi_oauth"
            && let (Some(client_id), Some(client_secret)) = (
                ctx.config.google_oauth_client_id.as_deref(),
                ctx.config.google_oauth_client_secret.as_deref(),
            ) {
                let tokens = kyomi_auth::google_oauth::ensure_valid_google_token(
                    &ctx.db,
                    &ctx.user_id,
                    &ctx.encryption_key,
                    client_id,
                    client_secret,
                )
                .await?;
                let oauth_data = kyomi_auth::google_oauth::OAuthData {
                    google_oauth_tokens: Some(tokens),
                    ..Default::default()
                };
                return Ok(serde_json::json!({ "oauth_data": oauth_data }));
            }

        Ok(serde_json::json!({}))
    } else {
        // Personal auth: decrypt per-user credentials
        let cred =
            kyomi_auth::datasource_service::get_user_credential(&ctx.db, &ctx.user_id, &ds.id)
                .await?
                .ok_or_else(|| {
                    kyomi_core::Error::NotFound(
                        "No credentials found for this datasource".into(),
                    )
                })?;
        let decrypted = kyomi_auth::credential_service::decrypt_credentials(
            &cred.credentials,
            &ctx.encryption_key,
        )?;

        // OAuth refresh if needed
        let refreshed = kyomi_datasource_server::oauth_refresh::ensure_valid_oauth_credentials(
            &decrypted,
            &ds.connection_config,
            ds_type,
        )
        .await?;

        Ok(refreshed)
    }
}

/// Create the default tool registry with all built-in tools.
pub fn create_default_registry() -> ToolRegistry {
    let mut registry = ToolRegistry::new();

    // Datasource tools
    registry.register(Arc::new(datasource::ListDatasourcesTool));
    registry.register(Arc::new(datasource::QueryDatasourceTool));
    registry.register(Arc::new(datasource::ValidateSqlTool));

    // Catalog tools
    registry.register(Arc::new(catalog::GetTableInfoTool));
    registry.register(Arc::new(catalog::BrowseCatalogTool));

    // Knowledge tools (search + document CRUD)
    registry.register(Arc::new(knowledge::SearchKnowledgeTool));
    registry.register(Arc::new(knowledge::WriteDocumentTool));
    registry.register(Arc::new(knowledge::ReadDocumentTool));
    registry.register(Arc::new(knowledge::ListDocumentsTool));
    registry.register(Arc::new(knowledge::EditDocumentTool));

    // Workspace tools
    registry.register(Arc::new(workspace::GetWorkspaceInfoTool));

    // ChartML spec tool
    registry.register(Arc::new(chartml::GetChartMLSpecTool));

    // Dashboard tools
    registry.register(Arc::new(dashboard::SearchDashboardsTool));
    registry.register(Arc::new(dashboard::GetDashboardInfoTool));
    registry.register(Arc::new(dashboard::CreateDashboardTool));
    registry.register(Arc::new(dashboard::ModifyDashboardTool));
    registry.register(Arc::new(dashboard::DeleteDashboardTool));

    // Watch tools
    registry.register(Arc::new(watch::CreateWatchTool));
    registry.register(Arc::new(watch::PreviewWatchTool));
    registry.register(Arc::new(watch::UpdateWatchTool));
    registry.register(Arc::new(watch::SearchWatchesTool));
    registry.register(Arc::new(watch::DeleteWatchTool));
    registry.register(Arc::new(watch::GetWatchInfoTool));
    registry.register(Arc::new(watch::TriggerWatchTool));

    // Forecast tools
    registry.register(Arc::new(forecast::ForecastDataTool));

    // Copilot tools
    registry.register(Arc::new(copilot::UpdateDashboardCopilotTool));
    registry.register(Arc::new(copilot::UpdateChartCopilotTool));

    // MCP-only tools
    registry.register(Arc::new(chart::RenderChartTool));

    // Analytics site tools (MCP-only)
    registry.register(Arc::new(analytics::ListAnalyticsSitesTool));
    registry.register(Arc::new(analytics::CreateAnalyticsSiteTool));
    registry.register(Arc::new(analytics::UpdateAnalyticsSiteTool));
    registry.register(Arc::new(analytics::DeleteAnalyticsSiteTool));

    // Documentation resource tools
    registry.register(Arc::new(resources::BrowseResourcesTool));
    registry.register(Arc::new(resources::ReadResourceTool));

    registry
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// A simple test tool for unit tests.
    struct TestTool {
        tool_name: String,
        tool_description: String,
        copilot_only: bool,
        mcp_only: bool,
    }

    impl TestTool {
        fn new(name: &str, description: &str) -> Self {
            Self {
                tool_name: name.to_string(),
                tool_description: description.to_string(),
                copilot_only: false,
                mcp_only: false,
            }
        }

        fn copilot_only(mut self) -> Self {
            self.copilot_only = true;
            self
        }

        fn mcp_only(mut self) -> Self {
            self.mcp_only = true;
            self
        }
    }

    #[async_trait]
    impl AgentTool for TestTool {
        fn name(&self) -> &str {
            &self.tool_name
        }

        fn description(&self) -> &str {
            &self.tool_description
        }

        fn parameters_schema(&self) -> serde_json::Value {
            serde_json::json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string" }
                },
                "required": ["query"]
            })
        }

        fn is_copilot_only(&self) -> bool {
            self.copilot_only
        }

        fn is_mcp_only(&self) -> bool {
            self.mcp_only
        }

        async fn execute(
            &self,
            _args: serde_json::Value,
            _ctx: &ToolContext,
        ) -> kyomi_core::Result<String> {
            Ok(format!("{} executed", self.tool_name))
        }
    }

    #[test]
    fn registry_register_and_get_tool() {
        let mut registry = ToolRegistry::new();
        let tool = Arc::new(TestTool::new("search_catalog", "Search for tables."));
        registry.register(tool);

        let found = registry.get_tool("search_catalog");
        assert!(found.is_some());
        assert_eq!(found.unwrap().name(), "search_catalog");
        assert_eq!(found.unwrap().description(), "Search for tables.");

        assert!(registry.get_tool("nonexistent").is_none());
    }

    #[test]
    fn registry_register_replaces_existing() {
        let mut registry = ToolRegistry::new();
        let tool1 = Arc::new(TestTool::new("my_tool", "Version 1"));
        let tool2 = Arc::new(TestTool::new("my_tool", "Version 2"));

        registry.register(tool1);
        registry.register(tool2);

        let found = registry.get_tool("my_tool").unwrap();
        assert_eq!(found.description(), "Version 2");
    }

    #[test]
    fn filter_no_filter_returns_all() {
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(TestTool::new("tool_a", "A")));
        registry.register(Arc::new(TestTool::new("tool_b", "B")));
        registry.register(Arc::new(TestTool::new("tool_c", "C")));

        let filter = ToolFilter::default();
        let tools = registry.get_tools(&filter);
        assert_eq!(tools.len(), 3);
    }

    #[test]
    fn filter_exclude_copilot_only() {
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(TestTool::new("normal_tool", "Normal")));
        registry.register(Arc::new(TestTool::new("copilot_tool", "Copilot").copilot_only()));

        let filter = ToolFilter {
            exclude_copilot_only: true,
            ..Default::default()
        };
        let tools = registry.get_tools(&filter);
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name(), "normal_tool");
    }

    #[test]
    fn filter_exclude_mcp_only() {
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(TestTool::new("normal_tool", "Normal")));
        registry.register(Arc::new(TestTool::new("mcp_tool", "MCP").mcp_only()));

        let filter = ToolFilter {
            exclude_mcp_only: true,
            ..Default::default()
        };
        let tools = registry.get_tools(&filter);
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name(), "normal_tool");
    }

    #[test]
    fn filter_include_only() {
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(TestTool::new("tool_a", "A")));
        registry.register(Arc::new(TestTool::new("tool_b", "B")));
        registry.register(Arc::new(TestTool::new("tool_c", "C")));

        let filter = ToolFilter {
            include_only: Some(vec!["tool_a".to_string(), "tool_c".to_string()]),
            ..Default::default()
        };
        let tools = registry.get_tools(&filter);
        assert_eq!(tools.len(), 2);

        let names: Vec<&str> = tools.iter().map(|t| t.name()).collect();
        assert!(names.contains(&"tool_a"));
        assert!(names.contains(&"tool_c"));
        assert!(!names.contains(&"tool_b"));
    }

    #[test]
    fn filter_combined_copilot_and_include_only() {
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(TestTool::new("tool_a", "A")));
        registry.register(Arc::new(TestTool::new("tool_b", "B").copilot_only()));
        registry.register(Arc::new(TestTool::new("tool_c", "C")));

        // Include only tool_a and tool_b, but also exclude copilot_only.
        let filter = ToolFilter {
            exclude_copilot_only: true,
            include_only: Some(vec!["tool_a".to_string(), "tool_b".to_string()]),
            ..Default::default()
        };
        let tools = registry.get_tools(&filter);
        // tool_b is excluded by copilot filter even though it's in include_only.
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name(), "tool_a");
    }

    #[test]
    fn get_tool_definitions_builds_tool_structs() {
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(TestTool::new("search_catalog", "Search for tables.")));
        registry.register(Arc::new(TestTool::new("query_datasource", "Run a SQL query.")));

        let filter = ToolFilter::default();
        let definitions = registry.get_tool_definitions(&filter);
        assert_eq!(definitions.len(), 2);

        for def in &definitions {
            assert!(!def.name.is_empty());
            assert!(!def.description.is_empty());
            assert_eq!(def.parameters["type"], "object");
            assert!(def.parameters.get("properties").is_some());
        }
    }

    #[test]
    fn get_tool_definitions_respects_filter() {
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(TestTool::new("tool_a", "A")));
        registry.register(Arc::new(TestTool::new("tool_b", "B").copilot_only()));

        let filter = ToolFilter {
            exclude_copilot_only: true,
            ..Default::default()
        };
        let definitions = registry.get_tool_definitions(&filter);
        assert_eq!(definitions.len(), 1);
        assert_eq!(definitions[0].name, "tool_a");
    }

    #[test]
    fn create_default_registry_has_tools() {
        let registry = create_default_registry();
        let filter = ToolFilter::default();
        let tools = registry.get_tools(&filter);
        assert_eq!(tools.len(), 34);

        // Verify all expected tools are registered
        let expected = [
            "list_datasources",
            "query_datasource",
            "validate_sql",
            "get_table_info",
            "browse_catalog",
            "search_knowledge",
            "write_knowledge_file",
            "read_knowledge_file",
            "list_knowledge_files",
            "edit_knowledge_file",
            "get_workspace_info",
            "get_chartml_spec",
            "search_dashboards",
            "get_dashboard_info",
            "create_dashboard",
            "modify_dashboard",
            "delete_dashboard",
            "update_dashboard",
            "update_chart",
            "create_watch",
            "preview_watch",
            "update_watch",
            "search_watches",
            "delete_watch",
            "get_watch_info",
            "trigger_watch",
            "forecast_data",
            "render_chart",
            "list_analytics_sites",
            "create_analytics_site",
            "update_analytics_site",
            "delete_analytics_site",
            "browse_resources",
            "read_resource",
        ];
        for name in expected {
            assert!(
                registry.get_tool(name).is_some(),
                "Tool '{name}' not found in default registry"
            );
        }
    }

    #[test]
    fn tool_names_returns_all_names() {
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(TestTool::new("alpha", "A")));
        registry.register(Arc::new(TestTool::new("beta", "B")));

        let mut names = registry.tool_names();
        names.sort();
        assert_eq!(names, vec!["alpha", "beta"]);
    }

    #[test]
    fn constants_are_defined() {
        // Verify constants exist and have expected values.
        assert!(COPILOT_ONLY_TOOLS.contains(&"update_dashboard"));
        assert!(COPILOT_ONLY_TOOLS.contains(&"update_chart"));
        assert!(COPILOT_ONLY_TOOLS.contains(&"preview_watch"));
        assert!(MCP_ONLY_TOOLS.contains(&"render_chart"));
        assert!(FINAL_TOOL_NAMES.contains(&"write_knowledge_file"));
        assert!(WATCH_TOOLS.contains(&"browse_catalog"));
        assert!(WATCH_TOOLS.contains(&"list_knowledge_files"));
        assert!(WATCH_TOOLS.contains(&"read_knowledge_file"));
    }

    #[test]
    fn agent_tool_default_trait_methods() {
        let tool = TestTool::new("test", "desc");
        assert!(!tool.is_copilot_only());
        assert!(!tool.is_mcp_only());
        assert!(tool.annotations().is_none());
    }

    #[test]
    fn registry_default_impl() {
        let registry = ToolRegistry::default();
        assert!(registry.get_tool("anything").is_none());
    }
}

// ---------------------------------------------------------------------------
// Contract tests — cross-cutting validation of the default registry
// ---------------------------------------------------------------------------

#[cfg(test)]
mod contract_tests {
    use super::*;
    use std::collections::HashSet;

    /// Helper: create the default registry and return all tools (no filter).
    fn all_tools() -> ToolRegistry {
        create_default_registry()
    }

    // -- Registry completeness ------------------------------------------------

    #[test]
    fn registry_has_expected_tool_count() {
        let registry = all_tools();
        let tools = registry.get_tools(&ToolFilter::default());
        assert_eq!(
            tools.len(),
            34,
            "Expected 34 tools, got {}. Names: {:?}",
            tools.len(),
            tools.iter().map(|t| t.name()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn all_expected_tool_names_are_registered() {
        let expected: HashSet<&str> = [
            "list_datasources",
            "query_datasource",
            "validate_sql",
            "get_table_info",
            "browse_catalog",
            "search_knowledge",
            "write_knowledge_file",
            "read_knowledge_file",
            "list_knowledge_files",
            "edit_knowledge_file",
            "get_workspace_info",
            "get_chartml_spec",
            "search_dashboards",
            "get_dashboard_info",
            "create_dashboard",
            "modify_dashboard",
            "delete_dashboard",
            "update_dashboard",
            "update_chart",
            "create_watch",
            "preview_watch",
            "update_watch",
            "search_watches",
            "delete_watch",
            "get_watch_info",
            "trigger_watch",
            "forecast_data",
            "render_chart",
            "list_analytics_sites",
            "create_analytics_site",
            "update_analytics_site",
            "delete_analytics_site",
            "browse_resources",
            "read_resource",
        ]
        .into_iter()
        .collect();

        let registry = all_tools();
        let actual: HashSet<&str> = registry
            .get_tools(&ToolFilter::default())
            .iter()
            .map(|t| t.name())
            .collect();

        let missing: Vec<&&str> = expected.difference(&actual).collect();
        let extra: Vec<&&str> = actual.difference(&expected).collect();

        assert!(
            missing.is_empty(),
            "Missing tools: {missing:?}"
        );
        assert!(
            extra.is_empty(),
            "Unexpected extra tools: {extra:?}"
        );
    }

    #[test]
    fn no_duplicate_tool_names() {
        let registry = all_tools();
        let tools = registry.get_tools(&ToolFilter::default());
        let mut seen = HashSet::new();
        for tool in &tools {
            assert!(
                seen.insert(tool.name()),
                "Duplicate tool name: '{}'",
                tool.name()
            );
        }
    }

    // -- Schema contract ------------------------------------------------------

    #[test]
    fn every_tool_schema_is_object_with_properties() {
        let registry = all_tools();
        for tool in registry.get_tools(&ToolFilter::default()) {
            let schema = tool.parameters_schema();
            assert_eq!(
                schema["type"], "object",
                "Tool '{}' schema type is not 'object'",
                tool.name()
            );
            assert!(
                schema.get("properties").is_some(),
                "Tool '{}' schema missing 'properties' key",
                tool.name()
            );
        }
    }

    #[test]
    fn every_tool_has_non_empty_description() {
        let registry = all_tools();
        for tool in registry.get_tools(&ToolFilter::default()) {
            assert!(
                !tool.description().is_empty(),
                "Tool '{}' has an empty description",
                tool.name()
            );
            assert!(
                tool.description().len() >= 10,
                "Tool '{}' description is suspiciously short ({} chars)",
                tool.name(),
                tool.description().len()
            );
        }
    }

    // -- Annotation contract --------------------------------------------------

    #[test]
    fn every_tool_has_annotations() {
        let registry = all_tools();
        for tool in registry.get_tools(&ToolFilter::default()) {
            assert!(
                tool.annotations().is_some(),
                "Tool '{}' is missing annotations",
                tool.name()
            );
        }
    }

    #[test]
    fn every_annotation_has_at_least_one_hint() {
        let registry = all_tools();
        for tool in registry.get_tools(&ToolFilter::default()) {
            let ann = tool.annotations().expect("annotations");
            let has_hint = ann.read_only_hint.is_some() || ann.destructive_hint.is_some();
            assert!(
                has_hint,
                "Tool '{}' annotations have no readOnlyHint or destructiveHint",
                tool.name()
            );
        }
    }

    // -- MCP-only and copilot-only constants ----------------------------------

    #[test]
    fn mcp_only_tools_are_correct() {
        let registry = all_tools();
        let mut mcp_only: Vec<&str> = registry
            .get_tools(&ToolFilter::default())
            .iter()
            .filter(|t| t.is_mcp_only())
            .map(|t| t.name())
            .collect();
        mcp_only.sort();

        let mut expected = vec![
            "render_chart",
            "list_analytics_sites",
            "create_analytics_site",
            "update_analytics_site",
            "delete_analytics_site",
        ];
        expected.sort();

        assert_eq!(mcp_only, expected);
    }

    #[test]
    fn copilot_only_tools_match_constant() {
        let registry = all_tools();
        let mut copilot_only: Vec<&str> = registry
            .get_tools(&ToolFilter::default())
            .iter()
            .filter(|t| t.is_copilot_only())
            .map(|t| t.name())
            .collect();
        copilot_only.sort();

        let mut expected: Vec<&str> = COPILOT_ONLY_TOOLS.to_vec();
        expected.sort();

        assert_eq!(copilot_only, expected);
    }

    #[test]
    fn mcp_only_tools_match_constant() {
        let registry = all_tools();
        let mut mcp_only: Vec<&str> = registry
            .get_tools(&ToolFilter::default())
            .iter()
            .filter(|t| t.is_mcp_only())
            .map(|t| t.name())
            .collect();
        mcp_only.sort();

        let mut expected: Vec<&str> = MCP_ONLY_TOOLS.to_vec();
        expected.sort();

        assert_eq!(mcp_only, expected);
    }

    // -- Read-only annotation consistency -------------------------------------

    #[test]
    fn search_get_list_tools_are_read_only() {
        let read_only_names: HashSet<&str> = [
            "list_datasources",
            "get_table_info",
            "browse_catalog",
            "search_knowledge",
            "list_knowledge_files",
            "read_knowledge_file",
            "get_workspace_info",
            "get_chartml_spec",
            "search_dashboards",
            "get_dashboard_info",
            "search_watches",
            "get_watch_info",
            "preview_watch", // preview is read-only (validates but doesn't create)
            "list_analytics_sites",
            "browse_resources",
            "read_resource",
        ]
        .into_iter()
        .collect();

        let registry = all_tools();
        for tool in registry.get_tools(&ToolFilter::default()) {
            if read_only_names.contains(tool.name()) {
                let ann = tool.annotations().expect("annotations");
                assert_eq!(
                    ann.read_only_hint,
                    Some(true),
                    "Tool '{}' should be read-only but isn't",
                    tool.name()
                );
            }
        }
    }

    #[test]
    fn mutating_tools_are_not_read_only() {
        let mutating_names: HashSet<&str> = [
            "write_knowledge_file",
            "edit_knowledge_file",
            "create_dashboard",
            "modify_dashboard",
            "delete_dashboard",
            "update_dashboard",
            "update_chart",
            "create_watch",
            "update_watch",
            "delete_watch",
            "trigger_watch",
            // forecast_data is read-only (runs computation, does not mutate)
            "render_chart",
            "create_analytics_site",
            "update_analytics_site",
            "delete_analytics_site",
        ]
        .into_iter()
        .collect();

        let registry = all_tools();
        for tool in registry.get_tools(&ToolFilter::default()) {
            if mutating_names.contains(tool.name()) {
                let ann = tool.annotations().expect("annotations");
                // Destructive tools may not set read_only_hint at all (that's fine),
                // but they must NOT have read_only_hint = true.
                assert_ne!(
                    ann.read_only_hint,
                    Some(true),
                    "Mutating tool '{}' should not be read-only",
                    tool.name()
                );
            }
        }
    }

    // -- Destructive annotation -----------------------------------------------

    #[test]
    fn destructive_tools_have_destructive_hint() {
        let destructive_names: HashSet<&str> =
            ["delete_dashboard", "delete_watch", "update_analytics_site", "delete_analytics_site"]
                .into_iter()
                .collect();

        let registry = all_tools();
        for tool in registry.get_tools(&ToolFilter::default()) {
            let ann = tool.annotations().expect("annotations");
            if destructive_names.contains(tool.name()) {
                assert_eq!(
                    ann.destructive_hint,
                    Some(true),
                    "Tool '{}' should have destructiveHint=true",
                    tool.name()
                );
            } else {
                assert_ne!(
                    ann.destructive_hint,
                    Some(true),
                    "Tool '{}' should NOT have destructiveHint=true",
                    tool.name()
                );
            }
        }
    }

    // -- Required fields contract ---------------------------------------------

    #[test]
    fn search_tools_have_no_required_fields() {
        // search_knowledge requires "query" so it is excluded here.
        let search_tools: HashSet<&str> = [
            "search_dashboards",
            "search_watches",
        ]
        .into_iter()
        .collect();

        let registry = all_tools();
        for tool in registry.get_tools(&ToolFilter::default()) {
            if search_tools.contains(tool.name()) {
                let schema = tool.parameters_schema();
                let required = schema["required"]
                    .as_array()
                    .expect("required is array");
                assert!(
                    required.is_empty(),
                    "Search tool '{}' should have no required fields, got {:?}",
                    tool.name(),
                    required
                );
            }
        }
    }

    #[test]
    fn create_tools_have_required_fields() {
        let create_tools: HashSet<&str> =
            ["create_dashboard", "create_watch"].into_iter().collect();

        let registry = all_tools();
        for tool in registry.get_tools(&ToolFilter::default()) {
            if create_tools.contains(tool.name()) {
                let schema = tool.parameters_schema();
                let required = schema["required"]
                    .as_array()
                    .expect("required is array");
                assert!(
                    !required.is_empty(),
                    "Create tool '{}' should have required fields",
                    tool.name()
                );
            }
        }
    }

    // -- Tool filtering for different contexts --------------------------------

    #[test]
    fn chat_filter_excludes_copilot_and_mcp_tools() {
        let registry = all_tools();
        let filter = ToolFilter {
            exclude_copilot_only: true,
            exclude_mcp_only: true,
            ..Default::default()
        };
        let tools = registry.get_tools(&filter);

        for tool in &tools {
            assert!(
                !tool.is_copilot_only(),
                "Chat context should not include copilot-only tool '{}'",
                tool.name()
            );
            assert!(
                !tool.is_mcp_only(),
                "Chat context should not include MCP-only tool '{}'",
                tool.name()
            );
        }

        // Chat context should exclude 3 copilot + 5 MCP = 8 tools
        assert_eq!(tools.len(), 34 - 8);
    }

    #[test]
    fn copilot_filter_includes_copilot_tools_excludes_mcp() {
        let registry = all_tools();
        let filter = ToolFilter {
            exclude_copilot_only: false,
            exclude_mcp_only: true,
            ..Default::default()
        };
        let tools = registry.get_tools(&filter);

        // Should include copilot tools but not MCP tools
        let names: HashSet<&str> = tools.iter().map(|t| t.name()).collect();
        for copilot_name in COPILOT_ONLY_TOOLS {
            assert!(
                names.contains(copilot_name),
                "Copilot filter should include '{copilot_name}'"
            );
        }
        for mcp_name in MCP_ONLY_TOOLS {
            assert!(
                !names.contains(mcp_name),
                "Copilot filter should not include MCP tool '{mcp_name}'"
            );
        }
        assert_eq!(tools.len(), 34 - 5); // Only MCP excluded
    }

    #[test]
    fn mcp_filter_includes_mcp_tools_excludes_copilot() {
        let registry = all_tools();
        let filter = ToolFilter {
            exclude_copilot_only: true,
            exclude_mcp_only: false,
            ..Default::default()
        };
        let tools = registry.get_tools(&filter);

        let names: HashSet<&str> = tools.iter().map(|t| t.name()).collect();
        for mcp_name in MCP_ONLY_TOOLS {
            assert!(
                names.contains(mcp_name),
                "MCP filter should include '{mcp_name}'"
            );
        }
        for copilot_name in COPILOT_ONLY_TOOLS {
            assert!(
                !names.contains(copilot_name),
                "MCP filter should not include copilot tool '{copilot_name}'"
            );
        }
        assert_eq!(tools.len(), 34 - 3); // Only copilot excluded
    }

    #[test]
    fn watch_filter_includes_only_data_query_tools() {
        let registry = all_tools();
        let filter = ToolFilter {
            include_only: Some(WATCH_TOOLS.iter().map(|s| s.to_string()).collect()),
            ..Default::default()
        };
        let tools = registry.get_tools(&filter);
        let names: HashSet<&str> = tools.iter().map(|t| t.name()).collect();

        assert_eq!(tools.len(), WATCH_TOOLS.len());
        for tool_name in WATCH_TOOLS {
            assert!(
                names.contains(tool_name),
                "Watch filter should include '{tool_name}'"
            );
        }
    }

    #[test]
    fn trial_filter_includes_only_read_and_viz_tools() {
        let registry = all_tools();
        let filter = ToolFilter {
            include_only: Some(TRIAL_CHAT_TOOLS.iter().map(|s| s.to_string()).collect()),
            ..Default::default()
        };
        let tools = registry.get_tools(&filter);
        let names: HashSet<&str> = tools.iter().map(|t| t.name()).collect();

        assert_eq!(tools.len(), TRIAL_CHAT_TOOLS.len());
        for tool_name in TRIAL_CHAT_TOOLS {
            assert!(
                names.contains(tool_name),
                "Trial filter should include '{tool_name}'"
            );
        }
        // Trial should NOT have mutation tools
        assert!(!names.contains("create_watch"));
        assert!(!names.contains("create_dashboard"));
    }

    // -- Schema serialization -------------------------------------------------

    #[test]
    fn all_tools_schema_serializes_to_valid_json() {
        let registry = all_tools();
        for tool in registry.get_tools(&ToolFilter::default()) {
            let schema = tool.parameters_schema();
            let serialized = serde_json::to_string(&schema);
            assert!(
                serialized.is_ok(),
                "Tool '{}' schema failed to serialize: {:?}",
                tool.name(),
                serialized.err()
            );
        }
    }

    #[test]
    fn all_tool_annotations_serialize_to_valid_json() {
        let registry = all_tools();
        for tool in registry.get_tools(&ToolFilter::default()) {
            let ann = tool.annotations().expect("annotations");
            let serialized = serde_json::to_value(&ann);
            assert!(
                serialized.is_ok(),
                "Tool '{}' annotations failed to serialize: {:?}",
                tool.name(),
                serialized.err()
            );
        }
    }

    // -- Specific tool schema contracts ----------------------------------------

    #[test]
    fn search_knowledge_requires_query_field() {
        let registry = all_tools();
        let tool = registry.get_tool("search_knowledge").unwrap();
        let schema = tool.parameters_schema();
        let required = schema["required"].as_array().expect("required is array");
        assert!(
            required.iter().any(|v| v.as_str() == Some("query")),
            "search_knowledge should require 'query' parameter"
        );
    }

    // -- Tool definitions round-trip ------------------------------------------

    #[test]
    fn get_tool_definitions_produces_valid_tool_structs_for_all() {
        let registry = all_tools();
        let filter = ToolFilter::default();
        let definitions = registry.get_tool_definitions(&filter);

        assert_eq!(definitions.len(), 34);
        for def in &definitions {
            assert!(!def.name.is_empty(), "Tool definition has empty name");
            assert!(
                !def.description.is_empty(),
                "Tool '{}' definition has empty description",
                def.name
            );
            assert_eq!(
                def.parameters["type"], "object",
                "Tool '{}' definition parameters is not an object",
                def.name
            );
        }
    }
}
