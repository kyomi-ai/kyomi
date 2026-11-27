// SPDX-License-Identifier: AGPL-3.0-or-later

//! MCP interop layer — connects the Rust MCP transport to the Leptos app state.
//!
//! Initializes the transport, registers notification handlers that update
//! reactive signals, and provides public functions for outgoing requests.

use std::cell::RefCell;
use std::rc::Rc;

use leptos::prelude::Set;
use serde::Deserialize;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;

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
///
/// The host sends a partial `HostContext` (MCP Apps protocol), so either half
/// can be present on its own. `styles` is kept as raw JSON so a styles payload
/// this build cannot parse does not cost the theme update beside it.
#[derive(Debug, Deserialize)]
struct HostContextPayload {
    theme: Option<String>,
    styles: Option<serde_json::Value>,
}

/// Read a `ui/notifications/host-context-changed` payload.
///
/// The error is the whole warning line, naming the field that could not be read
/// (a bad value deep in the payload otherwise costs the update with no hint of
/// where), so the caller only has to log it. Built here, not in the handler, so
/// the message is tested natively: the console call aborts a native process.
fn parse_host_context_changed(params: &serde_json::Value) -> Result<HostContextPayload, String> {
    mcp_transport::deserialize_naming_path(params, "params")
        .map_err(|detail| format!("[kyomi-mcp] host context ignored: {detail}"))
}

/// Read the `styles` of a host context change, naming the field that could not
/// be read (see [`parse_host_context_changed`]).
fn parse_host_styles(styles: &serde_json::Value) -> Result<mcp_transport::HostStyles, String> {
    mcp_transport::deserialize_naming_path(styles, "params.styles")
        .map_err(|detail| format!("[kyomi-mcp] host styles ignored: {detail}"))
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
        // claude.ai wraps structuredContent inside the tool result envelope:
        // { content: [...], isError: false, structuredContent: { spec, palette, ... } }
        let payload_value = params
            .get("structuredContent")
            .cloned()
            .unwrap_or(params.clone());
        let payload: ToolResultPayload = match serde_json::from_value(payload_value) {
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
        let payload = match parse_host_context_changed(&params) {
            Ok(p) => p,
            Err(warning) => {
                web_sys::console::warn_1(&warning.into());
                return;
            }
        };

        if let Some(new_theme) = payload.theme {
            mcp_transport::apply_theme(&new_theme);
            with_state(|state| {
                state.theme.set(new_theme);
            });
        }

        // A host that publishes plain per-theme values has to re-send them when
        // the theme changes; `light-dark()` values re-resolve on their own. The
        // table surface is taken from them, so apply what the host sends.
        if let Some(styles) = payload.styles {
            match parse_host_styles(&styles) {
                Ok(styles) => mcp_transport::apply_host_styles(&styles),
                Err(warning) => web_sys::console::warn_1(&warning.into()),
            }
        }
    });

    // Store transport
    TRANSPORT.with(|t| {
        *t.borrow_mut() = Some(transport.clone());
    });

    // Connect to host
    let host_ctx = transport.connect("Kyomi Chart Viewer", "2.0.0").await?;

    // Apply initial theme and styles. A host that sent no readable context has
    // nothing to apply, and the app keeps its own defaults.
    if let Some(host_ctx) = host_ctx {
        if let Some(ref theme) = host_ctx.theme {
            mcp_transport::apply_theme(theme);
            with_state(|state| {
                state.theme.set(theme.clone());
            });
        }
        if let Some(ref styles) = host_ctx.styles {
            mcp_transport::apply_host_styles(styles);
        }
    }

    setup_size_notifications(transport);

    Ok(())
}

/// Observe document size changes and notify the host so it can resize the iframe.
fn setup_size_notifications(transport: Rc<McpTransport>) {
    transport.send_size_changed();

    let throttled = Rc::new(RefCell::new(false));
    let callback = {
        let throttled = throttled.clone();
        Closure::<dyn Fn(js_sys::Array)>::new(move |_entries: js_sys::Array| {
            if *throttled.borrow() {
                return;
            }
            *throttled.borrow_mut() = true;

            let throttled = throttled.clone();
            let cb = Closure::once(move || {
                *throttled.borrow_mut() = false;
                with_transport(|t| t.send_size_changed());
            });
            if let Some(w) = web_sys::window() {
                let _ = w.request_animation_frame(cb.as_ref().unchecked_ref());
            }
            cb.forget();
        })
    };

    if let Ok(observer) = web_sys::ResizeObserver::new(callback.as_ref().unchecked_ref()) {
        if let Some(root) = web_sys::window()
            .and_then(|w| w.document())
            .and_then(|d| d.document_element())
        {
            observer.observe(&root);
        }
        if let Some(body) = web_sys::window()
            .and_then(|w| w.document())
            .and_then(|d| d.body())
        {
            observer.observe(body.as_ref());
        }
        Box::leak(Box::new(observer));
    }
    callback.forget();
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_host_context_change_with_a_bad_theme_names_the_theme() {
        let warning = parse_host_context_changed(&json!({ "theme": 3 }))
            .expect_err("a numeric theme is unreadable");

        assert!(
            warning.starts_with("[kyomi-mcp] host context ignored: params.theme: "),
            "the warning must carry the console prefix and the field, got: {warning}"
        );
        assert!(
            warning.contains("invalid type: integer `3`"),
            "the warning must carry serde's explanation, got: {warning}"
        );
    }

    #[test]
    fn a_host_context_change_is_read_when_well_formed() {
        let payload = parse_host_context_changed(&json!({
            "theme": "dark",
            "styles": { "variables": { "--color-background-primary": "#111" } }
        }))
        .expect("a well-formed change is read");

        assert_eq!(payload.theme.as_deref(), Some("dark"));
        assert!(payload.styles.is_some());
    }

    #[test]
    fn bad_host_styles_name_the_variable_that_could_not_be_read() {
        let warning = parse_host_styles(&json!({
            "variables": { "--color-background-primary": 17 }
        }))
            .expect_err("a numeric variable value is unreadable");

        assert!(
            warning.starts_with(
                "[kyomi-mcp] host styles ignored: params.styles.variables.--color-background-primary: "
            ),
            "the warning must carry the console prefix and the full path, got: {warning}"
        );
        assert!(
            warning.contains("invalid type: integer `17`, expected a string"),
            "the warning must carry serde's explanation, got: {warning}"
        );
    }

    #[test]
    fn host_styles_of_the_wrong_shape_name_the_styles_payload() {
        let warning = parse_host_styles(&json!("light-dark"))
            .expect_err("a string is not a styles object");

        assert!(
            warning.starts_with("[kyomi-mcp] host styles ignored: params.styles: "),
            "a failure at the top of styles must name styles itself, got: {warning}"
        );
    }
}
