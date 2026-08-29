// SPDX-License-Identifier: AGPL-3.0-or-later

//! `DocumentEditTool` — `edit_knowledge_file`.

use async_trait::async_trait;

use kyomi_auth::websocket::helpers as ws_helpers;

use crate::tools::document::{apply_update, resolve_document, ApplyUpdateOutcome, ApplyUpdateParams, DocType};
use crate::tools::{AgentTool, ToolContext};
use crate::types::ToolAnnotations;

/// Targeted find/replace edit of an existing document.
///
/// Registered only for `DocType::Knowledge` (`edit_knowledge_file`) — per
/// KYO-538 binding decision 1, dashboards have no exposed targeted-edit
/// tool, and that gap stays open; adding one is a capability addition, not
/// a structural unification, and is out of scope here. The `DocType` field
/// exists so a future dashboard-targeted-edit tool (should a later ticket
/// decide to add one) can reuse this exact implementation rather than a
/// third copy; the `unimplemented!` arms below make that boundary loud
/// rather than silently wrong if `DocType::Dashboard` is ever registered
/// without such a ticket deciding its name and schema first.
///
/// The associated functions below are `pub(crate)` so
/// `tools::knowledge::EditDocumentTool` — the pre-existing unit struct
/// KYO-537's characterization tests construct as a bare value via
/// `use super::*;` — can delegate to the exact same strings/schema/logic
/// without duplicating them. See [`super::DocumentReadTool`]'s doc comment
/// for why this is a unit struct with associated functions rather than a
/// `const` of `Self`.
pub struct DocumentEditTool {
    doc_type: DocType,
}

impl DocumentEditTool {
    pub const fn new(doc_type: DocType) -> Self {
        Self { doc_type }
    }

    pub(crate) fn name_for(doc_type: DocType) -> &'static str {
        match doc_type {
            DocType::Knowledge => "edit_knowledge_file",
            DocType::Dashboard => unimplemented!(
                "dashboards have no exposed targeted-edit tool (KYO-538 binding decision 1); \
                 do not register DocumentEditTool::new(DocType::Dashboard) without a ticket \
                 deciding its name and schema first"
            ),
        }
    }

    pub(crate) fn description_for(doc_type: DocType) -> &'static str {
        match doc_type {
            DocType::Knowledge => {
                "Make a targeted edit to an existing document using string replacement. \
                 Send only the old and new text. Fails if old_text is not found or appears multiple times."
            }
            DocType::Dashboard => unimplemented!("see name_for()"),
        }
    }

    pub(crate) fn schema_for(doc_type: DocType) -> serde_json::Value {
        match doc_type {
            DocType::Knowledge => serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Document title, ID, or legacy path"
                    },
                    "old_text": {
                        "type": "string",
                        "description": "Exact string to find"
                    },
                    "new_text": {
                        "type": "string",
                        "description": "Replacement string"
                    }
                },
                "required": ["path", "old_text", "new_text"]
            }),
            DocType::Dashboard => unimplemented!("see name_for()"),
        }
    }

    pub(crate) async fn execute_for(
        _doc_type: DocType,
        args: serde_json::Value,
        ctx: &ToolContext,
    ) -> kyomi_core::Result<String> {
        let path = args["path"]
            .as_str()
            .ok_or_else(|| kyomi_core::Error::BadRequest("path is required".into()))?;
        let old_text = args["old_text"]
            .as_str()
            .ok_or_else(|| kyomi_core::Error::BadRequest("old_text is required".into()))?;
        let new_text = args["new_text"]
            .as_str()
            .ok_or_else(|| kyomi_core::Error::BadRequest("new_text is required".into()))?;

        let doc = resolve_document(&ctx.db, &ctx.workspace_id, &ctx.user_id, path)
            .await?
            .ok_or_else(|| kyomi_core::Error::NotFound(format!("Document not found: {path}")))?;

        // Verify old_text appears exactly once
        let occurrences = doc.content.matches(old_text).count();
        if occurrences == 0 {
            return Ok(serde_json::json!({
                "success": false,
                "error": "old_text not found in document content",
            })
            .to_string());
        }
        if occurrences > 1 {
            return Ok(serde_json::json!({
                "success": false,
                "error": format!("old_text appears {occurrences} times — must appear exactly once"),
            })
            .to_string());
        }

        // Apply the replacement
        let new_content = doc.content.replacen(old_text, new_text, 1);
        let content_hash = doc.content_hash.as_deref();

        let embed = ctx.embedding.wait_ready().await?;

        // Update via dashboard_service with CAS
        let outcome = apply_update(ApplyUpdateParams {
            db: &ctx.db,
            dashboard_id: &doc.dashboard_id,
            workspace_id: &ctx.workspace_id,
            user_id: &ctx.user_id,
            title: None,
            content: Some(&new_content),
            change_summary: None,
            expected_content_hash: content_hash,
        })
        .await?;

        match outcome {
            ApplyUpdateOutcome::Updated => {
                // Rechunk after edit
                kyomi_auth::dashboard_service::rechunk_document(
                    &ctx.db, embed, &doc.dashboard_id, &new_content, &ctx.workspace_id,
                )
                .await?;

                ws_helpers::broadcast_dashboard_sync(
                    &ctx.db, &ctx.ws_manager, &doc.dashboard_id, &ctx.workspace_id,
                    kyomi_types::sync::SyncActionType::Update,
                    &ctx.user_id,
                )
                .await;

                let new_hash = kyomi_auth::dashboard_service::hash_content(&new_content);
                Ok(serde_json::json!({
                    "success": true,
                    "id": doc.dashboard_id,
                    "path": path,
                    "content_hash": new_hash,
                })
                .to_string())
            }
            ApplyUpdateOutcome::NotFound => Ok(serde_json::json!({
                "success": false,
                "error": "Document not found or not updated",
            })
            .to_string()),
            ApplyUpdateOutcome::Conflict(msg) => Ok(serde_json::json!({
                "success": false,
                "error": msg,
            })
            .to_string()),
        }
    }
}

#[async_trait]
impl AgentTool for DocumentEditTool {
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
            read_only_hint: Some(false),
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
