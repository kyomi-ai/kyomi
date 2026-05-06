// SPDX-License-Identifier: AGPL-3.0-or-later

//! Workspace tool — get workspace info and member details.

use async_trait::async_trait;

use crate::tools::{AgentTool, ToolContext};
use crate::types::ToolAnnotations;

// ---------------------------------------------------------------------------
// Row types
// ---------------------------------------------------------------------------

#[derive(sqlx::FromRow)]
struct WorkspaceInfoRow {
    workspace_id: String,
    name: Option<String>,
}

#[derive(sqlx::FromRow)]
struct WorkspaceMemberRow {
    user_id: String,
    role: String,
    name: Option<String>,
    email: String,
}

// ---------------------------------------------------------------------------
// GetWorkspaceInfoTool
// ---------------------------------------------------------------------------

/// Get information about the current workspace including all members.
pub struct GetWorkspaceInfoTool;

#[async_trait]
impl AgentTool for GetWorkspaceInfoTool {
    fn name(&self) -> &str {
        "get_workspace_info"
    }

    fn description(&self) -> &str {
        "Get information about the current workspace including all members \
         and their emails."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {},
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
        _args: serde_json::Value,
        ctx: &ToolContext,
    ) -> kyomi_core::Result<String> {
        // Get workspace info
        let workspace_row: Option<WorkspaceInfoRow> = kyomi_core::db_fetch_optional!(
            ctx.db, WorkspaceInfoRow,
            "SELECT workspace_id, name FROM workspaces WHERE workspace_id = $1",
            &ctx.workspace_id
        )?;

        let workspace_row = workspace_row.ok_or_else(|| {
            kyomi_core::Error::NotFound("Workspace not found".into())
        })?;

        let workspace_id = workspace_row.workspace_id;
        let workspace_name = workspace_row.name
            .unwrap_or_else(|| "Unnamed Workspace".to_string());

        // Get members with user details
        let member_rows: Vec<WorkspaceMemberRow> = kyomi_core::db_fetch_all!(
            ctx.db, WorkspaceMemberRow,
            "SELECT wu.user_id, wu.role, u.name, u.email \
             FROM workspace_users wu \
             JOIN users u ON u.user_id = wu.user_id \
             WHERE wu.workspace_id = $1 AND wu.active = true \
             ORDER BY \
                CASE WHEN wu.user_id = $2 THEN 0 ELSE 1 END, \
                CASE wu.role WHEN 'owner' THEN 0 WHEN 'admin' THEN 1 ELSE 2 END, \
                u.name ASC",
            &ctx.workspace_id,
            &ctx.user_id
        )?;

        let mut current_user_email: Option<String> = None;
        let member_list: Vec<serde_json::Value> = member_rows
            .iter()
            .map(|row| {
                let is_current = row.user_id == ctx.user_id;

                if is_current {
                    current_user_email = Some(row.email.clone());
                }

                serde_json::json!({
                    "name": row.name,
                    "email": row.email,
                    "role": row.role,
                    "is_current_user": is_current,
                })
            })
            .collect();

        Ok(serde_json::json!({
            "workspace_id": workspace_id,
            "workspace_name": workspace_name,
            "members": member_list,
            "member_count": member_list.len(),
            "current_user_email": current_user_email,
        })
        .to_string())
    }
}
