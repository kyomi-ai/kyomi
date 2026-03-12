// SPDX-License-Identifier: AGPL-3.0-or-later

//! Chart rendering tool (MCP-only).
//!
//! Provides the `render_chart` tool that accepts ChartML (YAML or JSON),
//! resolves data queries, and renders as an interactive chart (MCP Apps)
//! or PNG image.
//!
//! This tool is only exposed via MCP (`is_mcp_only() -> true`).
//!
//! ## Architecture
//!
//! 1. Parse ChartML (YAML/JSON string → serde_json::Value)
//! 2. Validate basic structure (type: chart)
//! 3. Resolve data queries → inline rows (via `chart_data_resolver`)
//! 4. Pre-resolve transforms for interactive mode (via chart-renderer /transform)
//! 5. For interactive: return `_mcp_app_data` marker → MCP protocol returns `structuredContent`
//! 6. For image: call chart-renderer /render → return `_mcp_image` marker → MCP protocol returns image content

use async_trait::async_trait;
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use serde_json::{json, Value};
use tracing;

use crate::tools::{AgentTool, ToolContext};
use crate::types::ToolAnnotations;

use super::chart_data_resolver;
use super::chart_palettes;
use super::chart_renderer::ChartRendererClient;

/// Chart context TTL in Redis — 30 days.
const CHART_CONTEXT_TTL_SECS: u64 = 30 * 24 * 60 * 60;

// ---------------------------------------------------------------------------
// RenderChartTool
// ---------------------------------------------------------------------------

/// Render ChartML as an interactive chart or PNG image.
///
/// Accepts YAML or JSON ChartML, resolves data queries, and renders
/// via the chart-renderer microservice.
pub struct RenderChartTool;

#[async_trait]
impl AgentTool for RenderChartTool {
    fn name(&self) -> &str {
        "render_chart"
    }

    fn description(&self) -> &str {
        "Render a ChartML visualization as an interactive chart or PNG image. \
         Accepts a ChartML specification as YAML or JSON string. Resolves data \
         queries and produces the final visualization. Use get_chartml_spec first \
         to understand ChartML syntax."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "chartml": {
                    "type": "string",
                    "description": "ChartML specification as YAML or JSON string"
                },
                "width": {
                    "type": "integer",
                    "description": "Chart width in pixels (default: 800)",
                    "default": 800
                },
                "height": {
                    "type": "integer",
                    "description": "Chart height in pixels (default: 400)",
                    "default": 400
                }
            },
            "required": ["chartml"]
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
        args: Value,
        ctx: &ToolContext,
    ) -> kyomi_core::Result<String> {
        // ── Parse arguments ────────────────────────────────────────────
        let chartml_str = args
            .get("chartml")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                kyomi_core::Error::BadRequest("Missing required parameter 'chartml'".into())
            })?;
        let width = args.get("width").and_then(|v| v.as_u64()).unwrap_or(800) as u32;
        let height = args.get("height").and_then(|v| v.as_u64()).unwrap_or(400) as u32;

        // Output format is determined by client capability, not agent choice.
        // Clients that support the MCP Apps extension (io.modelcontextprotocol/ui)
        // get interactive charts via structuredContent; all others get PNG images.
        let interactive = ctx.supports_mcp_apps;

        // ── Parse ChartML (YAML or JSON) ───────────────────────────────
        let spec: Value = serde_yaml::from_str(chartml_str).map_err(|e| {
            kyomi_core::Error::BadRequest(format!("Failed to parse ChartML: {e}"))
        })?;

        if !spec.is_object() {
            return Ok(json_error("ChartML spec must be an object/dict"));
        }

        if spec.get("type").and_then(|v| v.as_str()) != Some("chart") {
            return Ok(json_error("ChartML spec must have type: chart"));
        }

        // ── Resolve data queries → inline rows ─────────────────────────
        let query_ctx = ctx.query_context();
        let resolved_spec = match chart_data_resolver::resolve_chart_data(&spec, &query_ctx).await {
            Ok(s) => s,
            Err(e) => return Ok(json_error(&e)),
        };

        // ── Pre-resolve transforms for interactive mode ─────────────────
        // The MCP App doesn't have DuckDB, so we run the transform pipeline
        // on the chart-renderer and return a spec with no transform section.
        let mut resolved_spec = resolved_spec;
        if interactive {
            if let Some(transform) = resolved_spec.get("transform").cloned() {
                let renderer = ChartRendererClient::new(&ctx.config.chart_renderer_url)
                    .map_err(|e| kyomi_core::Error::Internal(format!("Failed to create chart renderer: {e}")))?;

                // Normalize unnamed source to named format for the transform endpoint
                let spec_data = resolved_spec.get("data").cloned().unwrap_or(json!({}));
                let named_data = if spec_data.is_object()
                    && !is_named_data(&spec_data)
                {
                    json!({ "source": spec_data })
                } else {
                    spec_data
                };

                match renderer.transform_data(&named_data, &transform).await {
                    Ok(transformed) => {
                        resolved_spec["data"] = transformed;
                        // Remove transform section — data is now fully resolved
                        if let Some(obj) = resolved_spec.as_object_mut() {
                            obj.remove("transform");
                        }
                    }
                    Err(e) => {
                        return Ok(json_error(&format!("Failed to apply data transforms: {e}")));
                    }
                }
            }
        }

        // ── Get user palette ────────────────────────────────────────────
        let user_palette = chart_palettes::get_user_palette(&ctx.db, &ctx.user_id).await;

        // ── Return based on output format ───────────────────────────────
        if interactive {
            // Store chart context in Redis for "Continue in Kyomi" deep-link
            let (chart_context_id, app_url) =
                store_chart_context(&resolved_spec, &spec, ctx).await;

            let mut mcp_data = json!({
                "spec": resolved_spec,
                "sourceSpec": spec,
                "palette": user_palette,
                "width": width,
                "height": height,
            });

            if let (Some(ctx_id), Some(url)) = (&chart_context_id, &app_url) {
                mcp_data["chartContextId"] = json!(ctx_id);
                mcp_data["appUrl"] = json!(url);
            }

            // Return _mcp_app_data marker — MCP protocol handler detects
            // this and returns structuredContent to the MCP App.
            Ok(json!({ "_mcp_app_data": mcp_data }).to_string())
        } else {
            // Render to PNG
            let renderer = match ChartRendererClient::new(&ctx.config.chart_renderer_url) {
                Ok(r) => r,
                Err(e) => return Ok(json_error(&format!("Failed to create chart renderer: {e}"))),
            };

            if !renderer.health_check().await {
                return Ok(json_error(
                    "Chart renderer service is not available. Please try again later.",
                ));
            }

            match renderer
                .render_chart(&resolved_spec, width, height, Some(&user_palette), None)
                .await
            {
                Ok(png_bytes) => {
                    let b64 = BASE64.encode(&png_bytes);
                    Ok(json!({
                        "_mcp_image": b64,
                        "mimeType": "image/png",
                    })
                    .to_string())
                }
                Err(e) => Ok(json_error(&format!("Failed to render chart: {e}"))),
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Return a JSON error string for tool output.
fn json_error(message: &str) -> String {
    json!({ "error": message }).to_string()
}

/// Check if data section is in named-sources format (no reserved keys).
fn is_named_data(data: &Value) -> bool {
    let reserved = ["datasource", "provider", "query", "rows", "url", "cache"];
    match data.as_object() {
        Some(obj) => !obj.keys().any(|k| reserved.contains(&k.as_str())),
        None => false,
    }
}

/// Store chart context in Redis for "Continue in Kyomi" deep-link.
///
/// Non-critical — chart still renders if this fails, just no deep-link button.
/// Returns (chart_context_id, frontend_url) if successful.
async fn store_chart_context(
    resolved_spec: &Value,
    original_spec: &Value,
    ctx: &ToolContext,
) -> (Option<String>, Option<String>) {
    let chart_context_id = uuid::Uuid::new_v4().to_string();
    let chart_title = resolved_spec
        .get("title")
        .and_then(|v| v.as_str())
        .unwrap_or("Chart");

    // Reconstruct ChartML YAML from the original (pre-resolve) spec
    // so the conversation gets the human-readable source, not resolved data
    let chart_yaml = match serde_yaml::to_string(original_spec) {
        Ok(y) => y,
        Err(e) => {
            tracing::warn!(error = %e, "Failed to serialize chart spec to YAML");
            return (None, None);
        }
    };

    let chart_markdown = format!("```chartml\n{chart_yaml}```");

    let context_data = json!({
        "spec": resolved_spec,
        "title": chart_title,
        "chartMarkdown": chart_markdown,
    });

    let context_json = match serde_json::to_string(&context_data) {
        Ok(j) => j,
        Err(e) => {
            tracing::warn!(error = %e, "Failed to serialize chart context");
            return (None, None);
        }
    };

    let kv_key = format!("chart:context:{chart_context_id}");

    match ctx.kv.set(&kv_key, &context_json, Some(CHART_CONTEXT_TTL_SECS)).await {
        Ok(()) => {
            tracing::debug!(
                id = &chart_context_id[..8],
                "Stored chart context for deep-link"
            );
            let app_url = ctx.config.frontend_url.clone();
            (Some(chart_context_id), Some(app_url))
        }
        Err(e) => {
            tracing::warn!(error = %e, "Failed to store chart context");
            (None, None)
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_chart_tool_name() {
        assert_eq!(RenderChartTool.name(), "render_chart");
    }

    #[test]
    fn render_chart_tool_description_not_empty() {
        assert!(!RenderChartTool.description().is_empty());
    }

    #[test]
    fn render_chart_tool_is_mcp_only() {
        assert!(RenderChartTool.is_mcp_only());
    }

    #[test]
    fn render_chart_tool_schema_has_required_chartml_field() {
        let schema = RenderChartTool.parameters_schema();
        let required = schema["required"].as_array().expect("required is array");
        assert!(required.contains(&json!("chartml")));
        assert_eq!(required.len(), 1);
    }

    #[test]
    fn render_chart_tool_schema_has_optional_fields() {
        let schema = RenderChartTool.parameters_schema();
        let props = schema["properties"].as_object().expect("properties is object");
        assert!(props.contains_key("chartml"));
        assert!(props.contains_key("width"));
        assert!(props.contains_key("height"));
        // No "output" parameter — format is determined by client capability
        assert!(!props.contains_key("output"));

        // Verify defaults.
        assert_eq!(props["width"]["default"], 800);
        assert_eq!(props["height"]["default"], 400);
    }

    #[test]
    fn render_chart_tool_annotations_not_read_only() {
        let ann = RenderChartTool.annotations().expect("has annotations");
        assert_eq!(ann.read_only_hint, Some(false));
        assert!(ann.destructive_hint.is_none());
    }

    #[test]
    fn render_chart_tool_is_not_copilot_only() {
        assert!(!RenderChartTool.is_copilot_only());
    }

    #[test]
    fn json_error_format() {
        let err = json_error("something broke");
        let parsed: Value = serde_json::from_str(&err).unwrap();
        assert_eq!(parsed["error"], "something broke");
    }

    #[test]
    fn is_named_data_detects_named() {
        let data = json!({
            "sales": { "datasource": "db", "query": "SELECT 1" },
            "targets": { "provider": "inline", "rows": [] }
        });
        assert!(is_named_data(&data));
    }

    #[test]
    fn is_named_data_detects_unnamed() {
        let data = json!({
            "datasource": "my-db",
            "query": "SELECT 1"
        });
        assert!(!is_named_data(&data));
    }
}
