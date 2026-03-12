// SPDX-License-Identifier: AGPL-3.0-or-later

//! Documentation resource tools — browse and read Kyomi's docs.
//!
//! These tools give the internal agent the same documentation access
//! that external MCP clients get via `resources/list` and `resources/read`.

use async_trait::async_trait;

use crate::tools::{AgentTool, ToolContext};
use crate::types::ToolAnnotations;

// ---------------------------------------------------------------------------
// BrowseResourcesTool
// ---------------------------------------------------------------------------

/// List available documentation resources.
pub struct BrowseResourcesTool;

#[async_trait]
impl AgentTool for BrowseResourcesTool {
    fn name(&self) -> &str {
        "browse_resources"
    }

    fn description(&self) -> &str {
        "List available Kyomi documentation resources. Returns a catalog of \
         docs:// URIs with titles and descriptions. Use this to discover what \
         documentation is available, then use read_resource to read specific docs."
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
        _ctx: &ToolContext,
    ) -> kyomi_core::Result<String> {
        let resources = kyomi_core::doc_resources::list_doc_resources();

        if resources.is_empty() {
            return Ok("No documentation resources are currently available.".to_string());
        }

        let mut lines = vec![format!("{} documentation resources available:\n", resources.len())];
        for doc in &resources {
            if doc.description.is_empty() {
                lines.push(format!("- {} — {}", doc.uri, doc.name));
            } else {
                lines.push(format!("- {} — {} — {}", doc.uri, doc.name, doc.description));
            }
        }
        lines.push(String::new());
        lines.push("Use read_resource with a URI to read a specific document.".to_string());

        Ok(lines.join("\n"))
    }
}

// ---------------------------------------------------------------------------
// ReadResourceTool
// ---------------------------------------------------------------------------

/// Read a specific documentation resource by URI.
pub struct ReadResourceTool;

#[async_trait]
impl AgentTool for ReadResourceTool {
    fn name(&self) -> &str {
        "read_resource"
    }

    fn description(&self) -> &str {
        "Read a specific Kyomi documentation resource by its docs:// URI. \
         Returns the full markdown content of the document. Use browse_resources \
         first to discover available URIs."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "uri": {
                    "type": "string",
                    "description": "The docs:// URI to read (e.g., docs://kyomi/chartml)"
                }
            },
            "required": ["uri"]
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
        _ctx: &ToolContext,
    ) -> kyomi_core::Result<String> {
        let uri = args
            .get("uri")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        if uri.is_empty() {
            return Err(kyomi_core::Error::BadRequest(
                "uri parameter is required".into(),
            ));
        }

        if !uri.starts_with(kyomi_core::doc_resources::DOCS_URI_PREFIX) {
            return Err(kyomi_core::Error::BadRequest(format!(
                "Invalid URI: must start with {}",
                kyomi_core::doc_resources::DOCS_URI_PREFIX
            )));
        }

        match kyomi_core::doc_resources::read_doc_resource(uri) {
            Some(content) => Ok(content),
            None => Ok(format!("Documentation not found for URI: {uri}. Use browse_resources to see available documents.")),
        }
    }
}
