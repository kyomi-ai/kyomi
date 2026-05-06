// SPDX-License-Identifier: AGPL-3.0-or-later

//! MCP Server endpoints — JSON-RPC over Streamable HTTP.
//!
//! Wire-compatible with Python's `routers/mcp.py` and `mcp/protocol.py`.
//!
//! ## Endpoints
//!
//! - `POST /mcp` — JSON-RPC 2.0 request/response (initialize, tools/list, tools/call, etc.)
//! - `GET  /mcp` — SSE stream for server-initiated notifications (tools/list_changed)
//! - `DELETE /mcp` — Terminate an MCP session
//!
//! ## Session Management (MCP spec 2025-03-26)
//!
//! - `initialize` creates a new session and returns `Mcp-Session-Id` header
//! - Non-initialize POST methods: missing `Mcp-Session-Id` is allowed (backwards compat),
//!   but present-and-invalid returns 404 (forces re-initialize)
//! - GET SSE stream requires valid `Mcp-Session-Id` header
//! - Server restart clears all sessions → clients re-initialize → fresh tool list
//! - Runtime tool changes → `notifications/tools/list_changed` pushed via SSE
//! - `DELETE` with session ID terminates that session
//!
//! ## Architecture
//!
//! - JWT authentication via `Authorization: Bearer <token>` header
//! - Capability gating: requires "mcp_access" (Starter/Pro/Team/Enterprise)
//! - Zero AI budget cost — client-side LLM, we just execute tools

use std::convert::Infallible;
use std::time::Duration;

use axum::{
    extract::{Request, State},
    http::{HeaderMap, StatusCode},
    middleware::{self, Next},
    response::{
        sse::{Event, KeepAlive, Sse},
        IntoResponse, Response,
    },
    routing::{delete, get, post},
    Json, Router,
};
use futures_util::stream;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::mpsc;

use kyomi_agent::tools::{create_default_registry, ToolContext, ToolFilter, ToolRegistry};
use kyomi_auth::middleware::AuthUser;
use kyomi_core::capability;

use crate::state::AppState;

// ===========================================================================
// Constants
// ===========================================================================

/// MCP protocol version — must match the spec version we implement.
const MCP_PROTOCOL_VERSION: &str = "2025-03-26";

/// MCP server name returned in initialize handshake.
const MCP_SERVER_NAME: &str = "kyomi-mcp";

/// MCP server version returned in initialize handshake.
const MCP_SERVER_VERSION: &str = "1.4.0";

/// URI for the chart viewer MCP App resource.
const CHART_UI_RESOURCE_URI: &str = "ui://kyomi/chart";

/// MIME type for MCP App HTML resources.
const MCP_APP_MIME_TYPE: &str = "text/html;profile=mcp-app";

/// Chart UI HTML embedded at compile time — build fails if file is missing.
const CHART_UI_HTML: &str =
    include_str!("../../../../apps/mcp-chart-app-wasm/chart_app.html");

/// Header name for MCP session ID (Streamable HTTP spec).
const MCP_SESSION_ID_HEADER: &str = "mcp-session-id";

/// MCP server instructions -- loaded into every agent's context on connect.
const MCP_INSTRUCTIONS: &str = "\
Kyomi is a data intelligence platform that connects to your data warehouse \
and provides AI-powered analytics. When users ask for help with Kyomi features, \
setup, or troubleshooting, read the documentation index resource at \
docs://kyomi/index to discover available topics, then read specific topic \
resources as needed. Do not load all resources -- read the index first, then \
only the sections relevant to the user's question.";

// ===========================================================================
// JSON-RPC 2.0 types
// ===========================================================================

/// JSON-RPC 2.0 request from an MCP client.
#[derive(Debug, Deserialize)]
struct JsonRpcRequest {
    id: Option<Value>,
    method: String,
    params: Option<Value>,
}

/// JSON-RPC 2.0 response to an MCP client.
#[derive(Debug, Serialize)]
struct JsonRpcResponse {
    jsonrpc: String,
    id: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<JsonRpcError>,
}

/// JSON-RPC 2.0 error object.
#[derive(Debug, Serialize)]
struct JsonRpcError {
    code: i32,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<Value>,
}

impl JsonRpcResponse {
    /// Create a success response.
    fn success(id: Option<Value>, result: Value) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            result: Some(result),
            error: None,
        }
    }

    /// Create an error response.
    fn error(id: Option<Value>, code: i32, message: impl Into<String>) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            result: None,
            error: Some(JsonRpcError {
                code,
                message: message.into(),
                data: None,
            }),
        }
    }
}

// ===========================================================================
// Router
// ===========================================================================

/// SSE heartbeat interval in seconds.
const SSE_HEARTBEAT_SECONDS: u64 = 30;

/// Build the `/mcp` router.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/", post(handle_mcp_request))
        .route("/", get(handle_mcp_sse))
        .route("/", delete(handle_mcp_delete))
        .layer(middleware::from_fn(mcp_www_authenticate_layer))
        .route(
            "/.well-known/openid-configuration",
            get(super::oauth::mcp_openid_configuration),
        )
}

/// Response layer that adds `WWW-Authenticate` to 401 responses per RFC 6750.
///
/// MCP clients (and directories like Glama) use this header to discover OAuth
/// endpoints and distinguish "online, requires auth" from "broken/offline".
/// The `resource_metadata` parameter points to the RFC 9728 protected resource
/// metadata endpoint, which in turn references the authorization server.
async fn mcp_www_authenticate_layer(request: Request, next: Next) -> Response {
    // Capture the request's host/scheme before passing ownership to `next`.
    let base_url = {
        let headers = request.headers();
        let scheme = headers
            .get("x-forwarded-proto")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("https");
        let host = headers
            .get("host")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("app.kyomi.ai");
        format!("{scheme}://{host}")
    };

    let mut response = next.run(request).await;

    if response.status() == StatusCode::UNAUTHORIZED {
        let www_auth = format!(
            r#"Bearer realm="OAuth", resource_metadata="{base_url}/.well-known/oauth-protected-resource", error="invalid_token", error_description="Missing or invalid access token""#
        );
        if let Ok(val) = www_auth.parse() {
            response.headers_mut().insert("www-authenticate", val);
        }
    }

    response
}

// ===========================================================================
// Capability check
// ===========================================================================

/// Check that the workspace has MCP access capability.
///
/// Returns HTTP 402 (Payment Required) if the workspace tier doesn't include MCP.
/// This matches the Python behavior exactly.
fn check_mcp_capability(user: &AuthUser, self_hosted: bool) -> Result<(), Box<Response>> {
    if self_hosted {
        return Ok(());
    }
    let tier = user.workspace.subscription_tier;
    if !capability::has_capability(tier, "mcp_access") {
        let body = json!({
            "detail": "MCP access requires Starter, Pro, Team, or Enterprise plan. \
                       Please upgrade at https://kyomi.ai/pricing"
        });
        return Err(Box::new((StatusCode::PAYMENT_REQUIRED, Json(body)).into_response()));
    }
    Ok(())
}

/// MCP Apps extension identifier per the ext-apps spec (2026-01-26).
///
/// Clients that support MCP Apps declare this in their `initialize` capabilities:
/// ```json
/// "capabilities": {
///     "extensions": {
///         "io.modelcontextprotocol/ui": {
///             "mimeTypes": ["text/html;profile=mcp-app"]
///         }
///     }
/// }
/// ```
const MCP_APPS_EXTENSION_ID: &str = "io.modelcontextprotocol/ui";

/// Check if the client's initialize capabilities include MCP Apps support.
///
/// Looks for the `io.modelcontextprotocol/ui` extension capability with
/// our MIME type in the `mimeTypes` array.
fn client_supports_mcp_apps(capabilities: &Value) -> bool {
    capabilities
        .get("extensions")
        .and_then(|ext| ext.get(MCP_APPS_EXTENSION_ID))
        .and_then(|ui| ui.get("mimeTypes"))
        .and_then(|mt| mt.as_array())
        .map(|types| types.iter().any(|t| t.as_str() == Some(MCP_APP_MIME_TYPE)))
        .unwrap_or(false)
}

/// Extract workspace_id from user, or return 400.
fn get_workspace_id(user: &AuthUser) -> Result<&str, kyomi_core::Error> {
    user.workspace
        .workspace_id
        .as_deref()
        .ok_or_else(|| kyomi_core::Error::BadRequest("No workspace associated with user".into()))
}

// ===========================================================================
// POST /mcp — JSON-RPC request handler
// ===========================================================================

/// Handle MCP JSON-RPC 2.0 requests with session management.
///
/// Dispatches to the appropriate handler based on the `method` field:
/// - `initialize` — Protocol handshake (creates session, exempt from session check)
/// - `tools/list` — Return available tools
/// - `tools/call` — Execute a tool
/// - `resources/list` — Return MCP App resources
/// - `resources/read` — Return resource content
/// - `ping` — Health check
async fn handle_mcp_request(
    State(state): State<AppState>,
    user: AuthUser,
    headers: HeaderMap,
    Json(request): Json<JsonRpcRequest>,
) -> Result<Response, Response> {
    // Check MCP capability (Starter/Pro/Team/Enterprise)
    check_mcp_capability(&user, state.config.self_hosted).map_err(|e| *e)?;

    let workspace_id = get_workspace_id(&user).map_err(IntoResponse::into_response)?;
    let msg_id = request.id.clone();
    let params = request.params.unwrap_or(Value::Null);

    tracing::info!(
        workspace_id,
        method = %request.method,
        "MCP request"
    );

    // Session validation: initialize is exempt, all others validate Mcp-Session-Id.
    //
    // If the header is missing, we allow the request (backwards compatibility with
    // clients that don't implement session management yet, e.g. Anthropic proxy).
    // If the header IS present but the session is unknown/expired, we auto-heal by
    // creating a fresh session. The workspace_id comes from the auth token so we
    // have everything needed. This prevents the Anthropic proxy from getting stuck
    // after server restarts — it doesn't reliably re-initialize on 404.
    let mut healed_session_id: Option<String> = None;
    if request.method != "initialize"
        && let Some(session_id_value) = headers.get(MCP_SESSION_ID_HEADER) {
            let session_id = session_id_value.to_str().unwrap_or("");
            if state.mcp_sessions.validate_session(session_id).await.is_none() {
                let new_session_id = state
                    .mcp_sessions
                    .create_session(workspace_id)
                    .await;
                tracing::info!(
                    workspace_id,
                    old_session_id = session_id,
                    new_session_id = %new_session_id,
                    method = %request.method,
                    "MCP session auto-healed (stale session ID replaced)"
                );
                healed_session_id = Some(new_session_id);
            }
        }

    // Create the tool registry once for the entire request (shared by tools/list and tools/call).
    let registry = create_default_registry();

    let response = match request.method.as_str() {
        "initialize" => {
            // Create a new session for this workspace
            let session_id = state.mcp_sessions.create_session(workspace_id).await;

            // Check if client supports MCP Apps via the io.modelcontextprotocol/ui
            // extension capability (ext-apps spec 2026-01-26).
            let capabilities = params.get("capabilities").cloned().unwrap_or(Value::Null);
            let supports_apps = client_supports_mcp_apps(&capabilities);
            state
                .mcp_sessions
                .set_supports_mcp_apps(&session_id, supports_apps)
                .await;

            if supports_apps {
                tracing::info!(workspace_id, session_id = %session_id, "Client supports MCP Apps");
            }

            let json_body = JsonRpcResponse::success(msg_id, handle_initialize());
            let mut resp = Json(json_body).into_response();
            resp.headers_mut().insert(
                MCP_SESSION_ID_HEADER,
                session_id.parse().expect("UUID is valid header value"),
            );
            return Ok(resp);
        }

        "tools/list" => JsonRpcResponse::success(msg_id, handle_tools_list(&registry)),

        "tools/call" => {
            // Look up MCP Apps support from the session (set during initialize).
            // If the session was auto-healed, the new session won't have the flag
            // yet — default to false (safe fallback, client can re-negotiate).
            let supports_mcp_apps = match healed_session_id
                .as_deref()
                .or_else(|| headers.get(MCP_SESSION_ID_HEADER).and_then(|v| v.to_str().ok()))
            {
                Some(sid) => state.mcp_sessions.supports_mcp_apps(sid).await,
                None => false,
            };

            let tool_ctx =
                build_tool_context(&state, &user, workspace_id, supports_mcp_apps);
            handle_tools_call(msg_id, &params, tool_ctx, &registry).await
        }

        "resources/list" => JsonRpcResponse::success(msg_id, handle_resources_list()),

        "resources/read" => {
            let result = handle_resources_read(&params);
            JsonRpcResponse::success(msg_id, result)
        }

        "ping" => JsonRpcResponse::success(msg_id, json!({})),

        // Standard MCP notifications — acknowledge silently (no response needed,
        // but JSON-RPC requires one since we parsed a request with an `id`)
        "notifications/initialized" | "notifications/cancelled" => {
            return Ok(StatusCode::ACCEPTED.into_response());
        }

        unknown => {
            tracing::warn!(method = %unknown, "Unknown MCP method");
            JsonRpcResponse::error(msg_id, -32601, format!("Method not found: {unknown}"))
        }
    };

    let mut resp = Json(response).into_response();

    // If session was auto-healed, include the new session ID so the client
    // can adopt it for future requests.
    if let Some(ref new_sid) = healed_session_id
        && let Ok(val) = new_sid.parse() {
            resp.headers_mut().insert(MCP_SESSION_ID_HEADER, val);
        }

    Ok(resp)
}

// ===========================================================================
// GET /mcp — SSE notification stream
// ===========================================================================

/// Handle GET /mcp — open an SSE stream for server-initiated notifications.
///
/// Requires a valid `Mcp-Session-Id` header (obtained from `initialize`).
/// The server pushes `notifications/tools/list_changed` through this stream
/// when the available tool list changes (e.g., billing tier change).
///
/// Returns `Mcp-Session-Id` header in the response for spec compliance.
async fn handle_mcp_sse(
    State(state): State<AppState>,
    user: AuthUser,
    headers: HeaderMap,
) -> Result<Response, Response> {
    check_mcp_capability(&user, state.config.self_hosted).map_err(|e| *e)?;

    // GET SSE requires a session ID header
    let session_id_str = headers
        .get(MCP_SESSION_ID_HEADER)
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| {
            let body = json!({"detail": "Missing Mcp-Session-Id header"});
            (StatusCode::BAD_REQUEST, Json(body)).into_response()
        })?;

    let workspace_id = get_workspace_id(&user).map_err(IntoResponse::into_response)?;

    // Auto-heal stale sessions instead of returning 404 (same rationale as POST handler)
    let session_id_str = if state.mcp_sessions.validate_session(session_id_str).await.is_none() {
        let new_id = state.mcp_sessions.create_session(workspace_id).await;
        tracing::info!(
            workspace_id,
            old_session_id = session_id_str,
            new_session_id = %new_id,
            "MCP SSE session auto-healed"
        );
        new_id
    } else {
        session_id_str.to_string()
    };

    tracing::info!(
        workspace_id,
        session_id = %session_id_str,
        "MCP SSE stream opened"
    );

    // Create an mpsc channel: server pushes notifications via tx, SSE streams from rx
    let (tx, rx) = mpsc::channel::<String>(32);
    state.mcp_sessions.set_sse_sender(&session_id_str, tx);

    // Convert the mpsc receiver into a Stream of SSE Events
    let event_stream = stream::unfold(rx, |mut rx| async move {
        let msg = rx.recv().await?;
        Some((Ok::<_, Infallible>(Event::default().data(msg)), rx))
    });

    let sse = Sse::new(event_stream)
        .keep_alive(KeepAlive::new().interval(Duration::from_secs(SSE_HEARTBEAT_SECONDS)));

    let mut resp = sse.into_response();
    resp.headers_mut().insert(
        MCP_SESSION_ID_HEADER,
        session_id_str
            .parse()
            .expect("session ID is valid header value"),
    );

    Ok(resp)
}

// ===========================================================================
// DELETE /mcp — Session termination
// ===========================================================================

/// Handle DELETE /mcp — terminate an MCP session.
///
/// Extracts `Mcp-Session-Id` header and removes the session.
async fn handle_mcp_delete(
    State(state): State<AppState>,
    user: AuthUser,
    headers: HeaderMap,
) -> Result<StatusCode, Response> {
    check_mcp_capability(&user, state.config.self_hosted).map_err(|e| *e)?;

    if let Some(session_id_value) = headers.get(MCP_SESSION_ID_HEADER) {
        let session_id = session_id_value.to_str().unwrap_or("");
        state.mcp_sessions.remove_session(session_id).await;
    }

    Ok(StatusCode::OK)
}

// ===========================================================================
// Protocol handlers
// ===========================================================================

/// Handle `initialize` — protocol handshake.
fn handle_initialize() -> Value {
    json!({
        "protocolVersion": MCP_PROTOCOL_VERSION,
        "capabilities": {
            "tools": {
                "listChanged": true
            },
            "resources": {
                "listChanged": false
            }
        },
        "serverInfo": {
            "name": MCP_SERVER_NAME,
            "version": MCP_SERVER_VERSION
        },
        "instructions": MCP_INSTRUCTIONS
    })
}

/// Handle `tools/list` — return all available tools in MCP format.
fn handle_tools_list(registry: &ToolRegistry) -> Value {
    // MCP filter: exclude copilot-only, include MCP-only
    let filter = ToolFilter {
        exclude_copilot_only: true,
        exclude_mcp_only: false,
        include_only: None,
    };

    let tools = registry.get_tools(&filter);
    let mcp_tools: Vec<Value> = tools
        .iter()
        .map(|tool| {
            let mut tool_def = json!({
                "name": tool.name(),
                "description": tool.description(),
                "inputSchema": tool.parameters_schema(),
            });

            // Add MCP tool annotations if present
            if let Some(annotations) = tool.annotations()
                && let Ok(ann_value) = serde_json::to_value(&annotations) {
                    tool_def["annotations"] = ann_value;
                }

            // Add MCP Apps UI resource reference for render_chart
            if tool.name() == "render_chart" {
                tool_def["_meta"] = json!({
                    "ui": {
                        "resourceUri": CHART_UI_RESOURCE_URI
                    }
                });
            }

            tool_def
        })
        .collect();

    json!({ "tools": mcp_tools })
}

/// Handle `tools/call` — execute a tool and return the result.
async fn handle_tools_call(
    msg_id: Option<Value>,
    params: &Value,
    tool_ctx: ToolContext,
    registry: &ToolRegistry,
) -> JsonRpcResponse {
    let tool_name = params
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    let arguments = params
        .get("arguments")
        .cloned()
        .unwrap_or(Value::Object(serde_json::Map::new()));

    if tool_name.is_empty() {
        return JsonRpcResponse::error(msg_id, -32602, "Missing tool name");
    }

    let tool = match registry.get_tool(tool_name) {
        Some(t) => t,
        None => {
            return JsonRpcResponse::error(
                msg_id,
                -32602,
                format!("Unknown tool: {tool_name}"),
            );
        }
    };

    tracing::info!(tool = %tool_name, "MCP tools/call executing");

    match tool.execute(arguments, &tool_ctx).await {
        Ok(result) => {
            tracing::info!(tool = %tool_name, result_len = result.len(), "MCP tool execution succeeded");
            format_tool_result(msg_id, &result)
        }
        Err(e) => {
            tracing::error!(tool = %tool_name, error = %e, "MCP tool execution error");
            JsonRpcResponse::success(
                msg_id,
                json!({
                    "content": [{ "type": "text", "text": format!("Error: {e}") }],
                    "isError": true
                }),
            )
        }
    }
}

/// Format a tool result string into the appropriate MCP response.
///
/// Detects special markers in the JSON result:
/// - `_mcp_app_data` → returns `structuredContent` for MCP Apps (interactive charts)
/// - `_mcp_image` → returns image content type with base64 data
/// - Otherwise → returns plain text content
///
/// Matches Python's `protocol.py` content type detection.
fn format_tool_result(msg_id: Option<Value>, result: &str) -> JsonRpcResponse {
    // Try to parse as JSON to detect special markers
    if let Ok(parsed) = serde_json::from_str::<Value>(result) {
        // MCP Apps: structuredContent for interactive charts
        if let Some(app_data) = parsed.get("_mcp_app_data") {
            let title = app_data
                .get("spec")
                .and_then(|s| s.get("title"))
                .and_then(|t| t.as_str())
                .unwrap_or("Visualization");

            return JsonRpcResponse::success(
                msg_id,
                json!({
                    "content": [{ "type": "text", "text": format!("Chart: {title}") }],
                    "structuredContent": app_data,
                    "isError": false
                }),
            );
        }

        // Image content: base64-encoded PNG
        if let Some(image_b64) = parsed.get("_mcp_image").and_then(|v| v.as_str()) {
            let mime_type = parsed
                .get("mimeType")
                .and_then(|v| v.as_str())
                .unwrap_or("image/png");

            return JsonRpcResponse::success(
                msg_id,
                json!({
                    "content": [{
                        "type": "image",
                        "data": image_b64,
                        "mimeType": mime_type
                    }],
                    "isError": false
                }),
            );
        }
    }

    // Default: plain text content
    JsonRpcResponse::success(
        msg_id,
        json!({
            "content": [{ "type": "text", "text": result }],
            "isError": false
        }),
    )
}

/// Handle `resources/list` — return available MCP App resources and documentation.
fn handle_resources_list() -> Value {
    let mut resources = vec![json!({
        "uri": CHART_UI_RESOURCE_URI,
        "name": "Kyomi Chart Viewer",
        "description": "Interactive chart visualization powered by ChartML",
        "mimeType": MCP_APP_MIME_TYPE
    })];

    for doc in kyomi_core::doc_resources::list_doc_resources() {
        resources.push(json!({
            "uri": doc.uri,
            "name": doc.name,
            "description": doc.description,
            "mimeType": doc.mime_type,
        }));
    }

    json!({ "resources": resources })
}

/// Handle `resources/read` — return resource content.
fn handle_resources_read(params: &Value) -> Value {
    let uri = params
        .get("uri")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    if uri == CHART_UI_RESOURCE_URI {
        return json!({
            "contents": [{
                "uri": uri,
                "mimeType": MCP_APP_MIME_TYPE,
                "text": CHART_UI_HTML
            }]
        });
    }

    // Documentation resources
    if uri.starts_with(kyomi_core::doc_resources::DOCS_URI_PREFIX) {
        if let Some(content) = kyomi_core::doc_resources::read_doc_resource(uri) {
            return json!({
                "contents": [{
                    "uri": uri,
                    "mimeType": "text/markdown",
                    "text": content
                }]
            });
        }

        return json!({
            "contents": [],
            "error": format!("Documentation not found: {uri}")
        });
    }

    json!({
        "contents": [],
        "error": format!("Unknown resource: {uri}")
    })
}

// ===========================================================================
// Helpers
// ===========================================================================

/// Build a ToolContext from the request state and authenticated user.
fn build_tool_context(
    state: &AppState,
    user: &AuthUser,
    workspace_id: &str,
    supports_mcp_apps: bool,
) -> ToolContext {
    let user_display_name = user
        .name
        .clone()
        .unwrap_or_else(|| user.email.clone());

    ToolContext {
        db: state.db.clone(),
        kv: state.kv.clone(),
        user_id: user.user_id.clone(),
        workspace_id: workspace_id.to_string(),
        encryption_key: state.encryption_key.clone(),
        embedding: state.embedding.clone(),
        ws_manager: state.ws_manager.clone(),
        config: state.config.clone(),
        session_id: None, // MCP calls are not in a chat session
        supports_mcp_apps,
        workspace_roles: user.workspace.workspace_roles.clone(),
        connect_registry: Some(state.connect_registry.clone()),
        platforms: state.platforms.clone(),
        user_display_name,
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // JSON-RPC types
    // -----------------------------------------------------------------------

    #[test]
    fn jsonrpc_request_deserializes() {
        let json = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/list",
            "params": {}
        });

        let req: JsonRpcRequest = serde_json::from_value(json).unwrap();
        assert_eq!(req.method, "tools/list");
        assert_eq!(req.id, Some(json!(1)));
    }

    #[test]
    fn jsonrpc_request_optional_id() {
        let json = json!({
            "jsonrpc": "2.0",
            "method": "ping"
        });

        let req: JsonRpcRequest = serde_json::from_value(json).unwrap();
        assert_eq!(req.method, "ping");
        assert!(req.id.is_none());
        assert!(req.params.is_none());
    }

    #[test]
    fn jsonrpc_request_string_id() {
        let json = json!({
            "jsonrpc": "2.0",
            "id": "abc-123",
            "method": "initialize"
        });

        let req: JsonRpcRequest = serde_json::from_value(json).unwrap();
        assert_eq!(req.id, Some(json!("abc-123")));
    }

    #[test]
    fn jsonrpc_success_response_shape() {
        let resp = JsonRpcResponse::success(Some(json!(1)), json!({"tools": []}));
        let serialized = serde_json::to_value(&resp).unwrap();

        assert_eq!(serialized["jsonrpc"], "2.0");
        assert_eq!(serialized["id"], 1);
        assert!(serialized.get("result").is_some());
        assert!(serialized.get("error").is_none());
    }

    #[test]
    fn jsonrpc_error_response_shape() {
        let resp = JsonRpcResponse::error(Some(json!(2)), -32601, "Method not found");
        let serialized = serde_json::to_value(&resp).unwrap();

        assert_eq!(serialized["jsonrpc"], "2.0");
        assert_eq!(serialized["id"], 2);
        assert!(serialized.get("result").is_none());
        assert_eq!(serialized["error"]["code"], -32601);
        assert_eq!(serialized["error"]["message"], "Method not found");
    }

    #[test]
    fn jsonrpc_error_skips_null_data() {
        let resp = JsonRpcResponse::error(None, -32602, "Invalid params");
        let serialized = serde_json::to_value(&resp).unwrap();

        assert!(serialized["error"].get("data").is_none());
    }

    #[test]
    fn jsonrpc_response_null_id_serialized() {
        // JSON-RPC 2.0 allows null id for notifications
        let resp = JsonRpcResponse::success(None, json!({}));
        let serialized = serde_json::to_value(&resp).unwrap();

        // id should be present as null (not absent)
        assert!(serialized.get("id").is_some());
        assert!(serialized["id"].is_null());
    }

    // -----------------------------------------------------------------------
    // Protocol handler responses
    // -----------------------------------------------------------------------

    #[test]
    fn initialize_response_shape() {
        let result = handle_initialize();

        assert_eq!(result["protocolVersion"], MCP_PROTOCOL_VERSION);
        assert_eq!(result["serverInfo"]["name"], MCP_SERVER_NAME);
        assert_eq!(result["serverInfo"]["version"], MCP_SERVER_VERSION);
        // Tools advertise listChanged (notifications via GET SSE stream)
        assert!(result["capabilities"]["tools"].is_object());
        assert_eq!(result["capabilities"]["tools"]["listChanged"], true);
        assert_eq!(result["capabilities"]["resources"]["listChanged"], false);
    }

    #[test]
    fn tools_list_returns_non_empty() {
        let registry = create_default_registry();
        let result = handle_tools_list(&registry);
        let tools = result["tools"].as_array().unwrap();

        assert!(!tools.is_empty(), "tools/list should return tools");

        // MCP should exclude copilot-only tools but include MCP-only tools
        let names: Vec<&str> = tools
            .iter()
            .filter_map(|t| t["name"].as_str())
            .collect();

        // render_chart (MCP-only) should be included
        assert!(
            names.contains(&"render_chart"),
            "render_chart should be in MCP tools"
        );

        // update_dashboard (copilot-only) should NOT be included
        assert!(
            !names.contains(&"update_dashboard"),
            "update_dashboard should not be in MCP tools"
        );
    }

    #[test]
    fn tools_list_has_correct_shape() {
        let registry = create_default_registry();
        let result = handle_tools_list(&registry);
        let tools = result["tools"].as_array().unwrap();

        for tool in tools {
            assert!(tool["name"].is_string(), "tool missing name");
            assert!(tool["description"].is_string(), "tool missing description");
            assert!(tool["inputSchema"].is_object(), "tool missing inputSchema");
        }
    }

    #[test]
    fn tools_list_render_chart_has_meta() {
        let registry = create_default_registry();
        let result = handle_tools_list(&registry);
        let tools = result["tools"].as_array().unwrap();

        let render_chart = tools
            .iter()
            .find(|t| t["name"] == "render_chart")
            .expect("render_chart should be in tools list");

        assert_eq!(
            render_chart["_meta"]["ui"]["resourceUri"],
            CHART_UI_RESOURCE_URI
        );
    }

    #[test]
    fn tools_list_has_annotations() {
        let registry = create_default_registry();
        let result = handle_tools_list(&registry);
        let tools = result["tools"].as_array().unwrap();

        for tool in tools {
            assert!(
                tool.get("annotations").is_some(),
                "Tool '{}' should have annotations",
                tool["name"]
            );
        }
    }

    #[test]
    fn resources_list_has_chart_viewer() {
        let result = handle_resources_list();
        let resources = result["resources"].as_array().unwrap();

        // At minimum we have the chart viewer; doc resources may add more
        assert!(
            !resources.is_empty(),
            "resources/list should return at least the chart viewer"
        );
        assert_eq!(resources[0]["uri"], CHART_UI_RESOURCE_URI);
        assert_eq!(resources[0]["name"], "Kyomi Chart Viewer");
        assert_eq!(resources[0]["mimeType"], MCP_APP_MIME_TYPE);
    }

    #[test]
    fn resources_read_unknown_uri() {
        let params = json!({"uri": "ui://unknown/resource"});
        let result = handle_resources_read(&params);

        let contents = result["contents"].as_array().unwrap();
        assert!(contents.is_empty());
        assert!(result["error"].is_string());
    }

    #[test]
    fn resources_read_chart_embedded() {
        let params = json!({"uri": CHART_UI_RESOURCE_URI});
        let result = handle_resources_read(&params);

        let contents = result["contents"].as_array().unwrap();
        assert_eq!(contents.len(), 1);
        assert_eq!(contents[0]["mimeType"], MCP_APP_MIME_TYPE);
        assert!(contents[0]["text"].as_str().unwrap().contains("<html"));
    }

    // -----------------------------------------------------------------------
    // Protocol constants
    // -----------------------------------------------------------------------

    #[test]
    fn constants_are_correct() {
        assert_eq!(MCP_PROTOCOL_VERSION, "2025-03-26");
        assert_eq!(MCP_SERVER_NAME, "kyomi-mcp");
        assert_eq!(MCP_SERVER_VERSION, "1.4.0");
        assert_eq!(CHART_UI_RESOURCE_URI, "ui://kyomi/chart");
        assert_eq!(MCP_APP_MIME_TYPE, "text/html;profile=mcp-app");
        assert_eq!(MCP_SESSION_ID_HEADER, "mcp-session-id");
    }

    // -----------------------------------------------------------------------
    // MCP Apps extension capability detection
    // -----------------------------------------------------------------------

    #[test]
    fn detects_mcp_apps_support_from_extension_capability() {
        let caps = json!({
            "extensions": {
                "io.modelcontextprotocol/ui": {
                    "mimeTypes": ["text/html;profile=mcp-app"]
                }
            }
        });
        assert!(client_supports_mcp_apps(&caps));
    }

    #[test]
    fn no_mcp_apps_without_extension() {
        let caps = json!({
            "roots": { "listChanged": true },
            "sampling": {}
        });
        assert!(!client_supports_mcp_apps(&caps));
    }

    #[test]
    fn no_mcp_apps_with_wrong_mime_type() {
        let caps = json!({
            "extensions": {
                "io.modelcontextprotocol/ui": {
                    "mimeTypes": ["text/plain"]
                }
            }
        });
        assert!(!client_supports_mcp_apps(&caps));
    }

    #[test]
    fn no_mcp_apps_with_empty_capabilities() {
        assert!(!client_supports_mcp_apps(&json!({})));
        assert!(!client_supports_mcp_apps(&Value::Null));
    }

    // -----------------------------------------------------------------------
    // Capability checking
    // -----------------------------------------------------------------------

    #[test]
    fn mcp_capability_denied_for_free_tier() {
        let user = test_auth_user("free");
        let result = check_mcp_capability(&user, false);
        assert!(result.is_err(), "Free tier should not have MCP access");
    }

    #[test]
    fn mcp_capability_allowed_for_starter() {
        let user = test_auth_user("starter");
        let result = check_mcp_capability(&user, false);
        assert!(result.is_ok());
    }

    #[test]
    fn mcp_capability_allowed_for_pro() {
        let user = test_auth_user("pro");
        let result = check_mcp_capability(&user, false);
        assert!(result.is_ok());
    }

    #[test]
    fn mcp_capability_allowed_for_team() {
        let user = test_auth_user("team");
        let result = check_mcp_capability(&user, false);
        assert!(result.is_ok());
    }

    #[test]
    fn mcp_capability_allowed_for_enterprise() {
        let user = test_auth_user("enterprise");
        let result = check_mcp_capability(&user, false);
        assert!(result.is_ok());
    }

    // -----------------------------------------------------------------------
    // tools/call response shapes
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn tools_call_unknown_tool_returns_error() {
        let params = json!({"name": "nonexistent_tool", "arguments": {}});
        let response = check_unknown_tool_error(&params);
        assert_eq!(response.error.as_ref().unwrap().code, -32602);
        assert!(response
            .error
            .as_ref()
            .unwrap()
            .message
            .contains("nonexistent_tool"));
    }

    #[tokio::test]
    async fn tools_call_missing_name_returns_error() {
        let params = json!({"arguments": {}});
        let response = check_unknown_tool_error(&params);
        assert_eq!(response.error.as_ref().unwrap().code, -32602);
    }

    /// Test helper that validates tool lookup error without needing a ToolContext.
    fn check_unknown_tool_error(params: &Value) -> JsonRpcResponse {
        let tool_name = params
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        if tool_name.is_empty() {
            return JsonRpcResponse::error(Some(json!(1)), -32602, "Missing tool name");
        }

        let registry = create_default_registry();
        if registry.get_tool(tool_name).is_none() {
            return JsonRpcResponse::error(
                Some(json!(1)),
                -32602,
                format!("Unknown tool: {tool_name}"),
            );
        }

        JsonRpcResponse::success(Some(json!(1)), json!({}))
    }

    // -----------------------------------------------------------------------
    // format_tool_result
    // -----------------------------------------------------------------------

    #[test]
    fn format_tool_result_plain_text() {
        let result = r#"{"tools": ["a", "b"]}"#;
        let resp = format_tool_result(Some(json!(1)), result);
        let val = resp.result.unwrap();
        assert_eq!(val["isError"], false);
        assert_eq!(val["content"][0]["type"], "text");
        assert_eq!(val["content"][0]["text"], result);
        // No structuredContent for plain JSON
        assert!(val.get("structuredContent").is_none());
    }

    #[test]
    fn format_tool_result_mcp_app_data() {
        let result = json!({
            "_mcp_app_data": {
                "spec": { "type": "chart", "title": "Revenue" },
                "palette": ["#1A75C9"],
                "width": 800,
                "height": 400,
            }
        })
        .to_string();

        let resp = format_tool_result(Some(json!(2)), &result);
        let val = resp.result.unwrap();
        assert_eq!(val["isError"], false);
        // Text content has chart title
        assert_eq!(val["content"][0]["type"], "text");
        assert!(val["content"][0]["text"].as_str().unwrap().contains("Revenue"));
        // structuredContent carries the app data
        assert!(val.get("structuredContent").is_some());
        assert_eq!(val["structuredContent"]["spec"]["title"], "Revenue");
    }

    #[test]
    fn format_tool_result_mcp_image() {
        let result = json!({
            "_mcp_image": "iVBORw0KGgo=",
            "mimeType": "image/png",
        })
        .to_string();

        let resp = format_tool_result(Some(json!(3)), &result);
        let val = resp.result.unwrap();
        assert_eq!(val["isError"], false);
        assert_eq!(val["content"][0]["type"], "image");
        assert_eq!(val["content"][0]["data"], "iVBORw0KGgo=");
        assert_eq!(val["content"][0]["mimeType"], "image/png");
    }

    #[test]
    fn format_tool_result_non_json_string() {
        let result = "Just a plain string, not JSON";
        let resp = format_tool_result(Some(json!(4)), result);
        let val = resp.result.unwrap();
        assert_eq!(val["content"][0]["type"], "text");
        assert_eq!(val["content"][0]["text"], result);
    }

    // -----------------------------------------------------------------------
    // MCP tool filter contract
    // -----------------------------------------------------------------------

    #[test]
    fn mcp_filter_matches_expected_count() {
        let registry = create_default_registry();
        let filter = ToolFilter {
            exclude_copilot_only: true,
            exclude_mcp_only: false,
            include_only: None,
        };
        let tools = registry.get_tools(&filter);

        // 28 total - 3 copilot-only = 25 MCP tools
        assert_eq!(
            tools.len(),
            25,
            "MCP should have 25 tools (28 total - 3 copilot-only)"
        );
    }

    // -----------------------------------------------------------------------
    // Documentation resources
    // -----------------------------------------------------------------------

    #[test]
    fn initialize_includes_instructions() {
        let result = handle_initialize();
        assert!(result.get("instructions").is_some());
        let instructions = result["instructions"].as_str().unwrap();
        assert!(instructions.contains("docs://kyomi/index"));
    }

    #[test]
    fn resources_list_includes_chart_viewer() {
        let result = handle_resources_list();
        let resources = result["resources"].as_array().unwrap();
        assert!(resources.iter().any(|r| r["uri"] == CHART_UI_RESOURCE_URI));
    }

    #[test]
    fn resources_read_docs_not_found() {
        let params = json!({"uri": "docs://kyomi/nonexistent-page"});
        let result = handle_resources_read(&params);

        let contents = result["contents"].as_array().unwrap();
        assert!(contents.is_empty());
        assert!(result["error"]
            .as_str()
            .unwrap()
            .contains("Documentation not found"));
    }

    // -----------------------------------------------------------------------
    // Test helpers
    // -----------------------------------------------------------------------

    /// Create a test AuthUser with the given subscription tier.
    fn test_auth_user(tier: &str) -> AuthUser {
        use kyomi_auth::middleware::WorkspaceContext;

        let subscription_tier = match tier {
            "free" => kyomi_core::SubscriptionTier::Free,
            "basic" => kyomi_core::SubscriptionTier::Basic,
            "starter" => kyomi_core::SubscriptionTier::Starter,
            "pro" => kyomi_core::SubscriptionTier::Pro,
            "team" => kyomi_core::SubscriptionTier::Team,
            "enterprise" => kyomi_core::SubscriptionTier::Enterprise,
            _ => kyomi_core::SubscriptionTier::Free,
        };

        AuthUser {
            user_id: "user-test".to_string(),
            email: "test@test.com".to_string(),
            name: Some("Test User".to_string()),
            roles: vec![],
            active: true,
            verified: true,
            workspace: WorkspaceContext {
                workspace_id: Some("ws-test".to_string()),
                workspace_name: Some("Test Workspace".to_string()),
                workspace_roles: vec![],
                workspace_status: Some(kyomi_core::WorkspaceStatus::Active),
                subscription_tier,
                subscription_status: kyomi_core::enums::SubscriptionStatus::Active,
                trial_ends_at: None,
                is_owner: true,
            },
            token_exp: None,
            token_jti: None,
        }
    }
}
