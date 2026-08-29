// SPDX-License-Identifier: AGPL-3.0-or-later

//! `DocumentReadTool` — `read_knowledge_file` / `get_dashboard_info`.

use async_trait::async_trait;

use crate::tools::document::{read_document, DocType};
use crate::tools::{AgentTool, ToolContext};
use crate::types::ToolAnnotations;

/// Read a single document. Registered twice by KYO-538 (once per
/// [`DocType`]), producing `read_knowledge_file` and `get_dashboard_info`
/// respectively — see [`read_document`] for the one real behavioural fork
/// between the two.
///
/// The associated functions below (`name_for`, `description_for`, etc.) are
/// `pub(crate)` rather than folded straight into the trait methods so that
/// `tools::knowledge::ReadDocumentTool` and `tools::dashboard::GetDashboardInfoTool`
/// — the pre-existing unit structs KYO-537's characterization tests
/// construct as bare values via `use super::*;` — can delegate to the exact
/// same strings/schema/logic without duplicating them, while still being
/// real unit structs themselves (a `const` of a struct-with-fields hits
/// `non_upper_case_globals`, which this codebase's lint policy forbids
/// suppressing).
pub struct DocumentReadTool {
    doc_type: DocType,
}

impl DocumentReadTool {
    pub const fn new(doc_type: DocType) -> Self {
        Self { doc_type }
    }

    pub(crate) fn name_for(doc_type: DocType) -> &'static str {
        match doc_type {
            DocType::Knowledge => "read_knowledge_file",
            DocType::Dashboard => "get_dashboard_info",
        }
    }

    pub(crate) fn description_for(doc_type: DocType) -> &'static str {
        match doc_type {
            DocType::Knowledge => {
                "Read a specific document by title or ID. Returns the full markdown content. \
                 Use this when you know which document to look at (from search results)."
            }
            DocType::Dashboard => {
                "Get detailed information about a specific dashboard including full content."
            }
        }
    }

    pub(crate) fn schema_for(doc_type: DocType) -> serde_json::Value {
        match doc_type {
            DocType::Knowledge => serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Document title, ID, or legacy path (e.g. 'Revenue/Metrics.md')"
                    }
                },
                "required": ["path"]
            }),
            DocType::Dashboard => serde_json::json!({
                "type": "object",
                "properties": {
                    "dashboard_id": {
                        "type": "string",
                        "description": "The dashboard ID to retrieve"
                    }
                },
                "required": ["dashboard_id"]
            }),
        }
    }

    pub(crate) async fn execute_for(
        doc_type: DocType,
        args: serde_json::Value,
        ctx: &ToolContext,
    ) -> kyomi_core::Result<String> {
        match doc_type {
            DocType::Knowledge => {
                let path = args["path"]
                    .as_str()
                    .ok_or_else(|| kyomi_core::Error::BadRequest("path is required".into()))?;

                let doc =
                    read_document(&ctx.db, &ctx.workspace_id, &ctx.user_id, DocType::Knowledge, path)
                        .await?;
                match doc {
                    Some(d) => Ok(serde_json::json!({
                        "id": d.dashboard_id,
                        "path": path,
                        "name": d.title,
                        "doc_type": d.doc_type().as_str(),
                        "content": d.content,
                        "content_hash": d.content_hash,
                        "updated_at": d.updated_at.to_rfc3339(),
                    })
                    .to_string()),
                    None => Ok(serde_json::json!({
                        "error": format!("Document not found: {path}"),
                    })
                    .to_string()),
                }
            }
            DocType::Dashboard => {
                let dashboard_id = args
                    .get("dashboard_id")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| {
                        kyomi_core::Error::BadRequest(
                            "Missing required parameter 'dashboard_id'".into(),
                        )
                    })?;

                let dashboard = read_document(
                    &ctx.db,
                    &ctx.workspace_id,
                    &ctx.user_id,
                    DocType::Dashboard,
                    dashboard_id,
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
    }
}

#[async_trait]
impl AgentTool for DocumentReadTool {
    fn name(&self) -> &str {
        Self::name_for(self.doc_type)
    }

    fn description(&self) -> &str {
        Self::description_for(self.doc_type)
    }

    fn parameters_schema(&self) -> serde_json::Value {
        Self::schema_for(self.doc_type)
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
        Self::execute_for(self.doc_type, args, ctx).await
    }
}
