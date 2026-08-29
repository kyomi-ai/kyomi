// SPDX-License-Identifier: AGPL-3.0-or-later

//! `DocumentDeleteTool` — `delete_dashboard`.

use async_trait::async_trait;

use kyomi_auth::websocket::helpers as ws_helpers;

use crate::tools::document::DocType;
use crate::tools::{AgentTool, ToolContext};
use crate::types::ToolAnnotations;

/// Delete a document. Registered only for `DocType::Dashboard`
/// (`delete_dashboard`) — per KYO-538 binding decision 1, knowledge has no
/// delete tool, and that gap stays open; adding one is a capability
/// addition, out of scope here. See [`super::DocumentEditTool`]'s doc
/// comment for why the `DocType` field and `unimplemented!` arms exist
/// despite only one variant ever being registered today, and
/// [`super::DocumentReadTool`]'s doc comment for why the associated
/// functions below are `pub(crate)` rather than folded into the trait
/// methods directly.
pub struct DocumentDeleteTool {
    doc_type: DocType,
}

impl DocumentDeleteTool {
    pub const fn new(doc_type: DocType) -> Self {
        Self { doc_type }
    }

    pub(crate) fn name_for(doc_type: DocType) -> &'static str {
        match doc_type {
            DocType::Dashboard => "delete_dashboard",
            DocType::Knowledge => unimplemented!(
                "knowledge documents have no delete tool (KYO-538 binding decision 1); \
                 do not register DocumentDeleteTool::new(DocType::Knowledge) without a ticket \
                 deciding its name, schema, and whether it should exist at all"
            ),
        }
    }

    pub(crate) fn description_for(doc_type: DocType) -> &'static str {
        match doc_type {
            DocType::Dashboard => {
                "Delete a dashboard. You must be the owner to delete. This action cannot be undone."
            }
            DocType::Knowledge => unimplemented!("see name_for()"),
        }
    }

    pub(crate) fn schema_for(doc_type: DocType) -> serde_json::Value {
        match doc_type {
            DocType::Dashboard => serde_json::json!({
                "type": "object",
                "properties": {
                    "dashboard_id": {
                        "type": "string",
                        "description": "Dashboard ID to delete"
                    }
                },
                "required": ["dashboard_id"]
            }),
            DocType::Knowledge => unimplemented!("see name_for()"),
        }
    }

    pub(crate) async fn execute_for(
        _doc_type: DocType,
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

#[async_trait]
impl AgentTool for DocumentDeleteTool {
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
            destructive_hint: Some(true),
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
