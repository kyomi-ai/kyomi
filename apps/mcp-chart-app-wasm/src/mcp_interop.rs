// SPDX-License-Identifier: AGPL-3.0-or-later

//! MCP interop layer — connects the Rust MCP transport to the Leptos app state.
//!
//! Initializes the transport, registers notification handlers that update
//! reactive signals, and provides public functions for outgoing requests.

use std::cell::RefCell;
use std::rc::Rc;

use leptos::prelude::Set;
use serde::Deserialize;

use crate::app::AppState;
use crate::mcp_transport::{self, McpTransport};

// ---------------------------------------------------------------------------
// Transport singleton
// ---------------------------------------------------------------------------

thread_local! {
    static TRANSPORT: RefCell<Option<Rc<McpTransport>>> = const { RefCell::new(None) };
    static APP_STATE: RefCell<Option<AppState>> = const { RefCell::new(None) };
}

fn with_transport<F: FnOnce(&McpTransport) -> R, R>(f: F) -> Option<R> {
    TRANSPORT.with(|t| t.borrow().as_ref().map(|t| f(t)))
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Call an MCP server tool and return the parsed JSON result.
pub async fn call_server_tool(
    name: &str,
    args: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    let transport = TRANSPORT.with(|t| t.borrow().clone())
        .ok_or("Transport not initialized")?;
    transport.call_server_tool(name, args).await
}

/// Open a URL in the MCP host.
pub fn open_link(url: &str) {
    with_transport(|t| t.open_link(url));
}

// ---------------------------------------------------------------------------
// Initialization
// ---------------------------------------------------------------------------

/// Deserialized tool result payload from the MCP host.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ToolResultPayload {
    spec: Option<serde_json::Value>,
    source_spec: Option<serde_json::Value>,
    palette: Option<Vec<String>>,
    chart_context_id: Option<String>,
    app_url: Option<String>,
}

/// Deserialized host context change payload.
#[derive(Deserialize)]
struct HostContextPayload {
    theme: Option<String>,
}

fn with_state<F: FnOnce(AppState)>(f: F) {
    APP_STATE.with(|s| {
        if let Some(state) = *s.borrow() {
            f(state);
        }
    });
}

/// Initialize the MCP transport and connect to the host.
///
/// Sets up notification handlers that update the app state signals.
/// Must be called before `mount_to_body`.
pub async fn initialize(state: AppState) -> Result<(), String> {
    APP_STATE.with(|s| {
        *s.borrow_mut() = Some(state);
    });

    let transport = Rc::new(McpTransport::new());

    // Register tool result handler
    transport.set_on_tool_result(move |params| {
        let payload: ToolResultPayload = match serde_json::from_value(params) {
            Ok(p) => p,
            Err(e) => {
                web_sys::console::error_1(&format!("Failed to parse tool result: {e}").into());
                with_state(|state| {
                    state.error.set(Some(format!("Failed to parse chart data: {e}")));
                });
                return;
            }
        };

        with_state(|state| {
            if let Some(spec) = payload.spec {
                state.spec.set(Some(spec.clone()));
                state.source_spec.set(payload.source_spec.or(Some(spec)));
                state.palette.set(payload.palette);
                state.chart_context_id.set(payload.chart_context_id);
                state.app_url.set(payload.app_url);
                state.info_panel_open.set(false);
                state.dashboard_panel_open.set(false);
                state.error.set(None);
            } else {
                state.error.set(Some("No chart specification in tool result".to_string()));
            }
        });
    });

    // Register host context change handler
    transport.set_on_host_context_changed(move |params| {
        let payload: HostContextPayload = match serde_json::from_value(params) {
            Ok(p) => p,
            Err(_) => return,
        };

        if let Some(new_theme) = payload.theme {
            mcp_transport::apply_theme(&new_theme);
            with_state(|state| {
                state.theme.set(new_theme);
            });
        }
    });

    // Store transport
    TRANSPORT.with(|t| {
        *t.borrow_mut() = Some(transport.clone());
    });

    // Connect to host
    let host_ctx = transport.connect("Kyomi Chart Viewer", "2.0.0").await?;

    // Apply initial theme and styles
    if let Some(ref theme) = host_ctx.theme {
        mcp_transport::apply_theme(theme);
        with_state(|state| {
            state.theme.set(theme.clone());
        });
    }
    if let Some(ref styles) = host_ctx.styles {
        mcp_transport::apply_host_styles(styles);
    }

    Ok(())
}
