// SPDX-License-Identifier: AGPL-3.0-or-later

//! Pure Rust implementation of the MCP Apps postMessage transport.
//!
//! Replaces `@modelcontextprotocol/ext-apps` JS SDK with ~200 lines of Rust
//! using `web-sys`. Implements JSON-RPC 2.0 over `window.postMessage()`.
//!
//! Protocol version: 2026-01-26

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::Deserialize;
use serde_json::Value;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;

// ---------------------------------------------------------------------------
// Protocol constants
// ---------------------------------------------------------------------------

const PROTOCOL_VERSION: &str = "2026-01-26";

// Methods: App → Host
const METHOD_INITIALIZE: &str = "ui/initialize";
const METHOD_INITIALIZED: &str = "ui/notifications/initialized";
const METHOD_TOOLS_CALL: &str = "tools/call";
const METHOD_OPEN_LINK: &str = "ui/open-link";
const METHOD_SIZE_CHANGED: &str = "ui/notifications/size-changed";

// Methods: Host → App (notifications)
const METHOD_TOOL_RESULT: &str = "ui/notifications/tool-result";
const METHOD_TOOL_INPUT: &str = "ui/notifications/tool-input";
const METHOD_HOST_CONTEXT_CHANGED: &str = "ui/notifications/host-context-changed";

// ---------------------------------------------------------------------------
// JSON-RPC types
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct JsonRpcMessage {
    #[serde(default, rename = "jsonrpc")]
    _jsonrpc: Option<String>,
    id: Option<u64>,
    method: Option<String>,
    params: Option<Value>,
    result: Option<Value>,
    error: Option<Value>,
}

// ---------------------------------------------------------------------------
// Host context
// ---------------------------------------------------------------------------

/// Context returned by the host during initialization.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostContext {
    pub theme: Option<String>,
    pub styles: Option<HostStyles>,
    pub display_mode: Option<String>,
    pub locale: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct HostStyles {
    pub variables: Option<HashMap<String, String>>,
    pub css: Option<HostCss>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct HostCss {
    pub fonts: Option<String>,
}

// ---------------------------------------------------------------------------
// Pending request tracking
// ---------------------------------------------------------------------------

type PendingCallback = Box<dyn FnOnce(Result<Value, String>)>;
type PendingMap = Rc<RefCell<HashMap<u64, PendingCallback>>>;

// ---------------------------------------------------------------------------
// Notification callbacks
// ---------------------------------------------------------------------------

type NotificationHandler = Box<dyn Fn(Value)>;

struct Handlers {
    on_tool_result: Option<NotificationHandler>,
    on_tool_input: Option<NotificationHandler>,
    on_host_context_changed: Option<NotificationHandler>,
}

// ---------------------------------------------------------------------------
// Transport
// ---------------------------------------------------------------------------

/// MCP Apps postMessage transport — pure Rust replacement for ext-apps SDK.
pub struct McpTransport {
    next_id: AtomicU64,
    pending: PendingMap,
    handlers: Rc<RefCell<Handlers>>,
    host_context: Rc<RefCell<Option<HostContext>>>,
}

impl McpTransport {
    /// Create a new transport. Call `connect()` to initiate the handshake.
    pub fn new() -> Self {
        Self {
            next_id: AtomicU64::new(1),
            pending: Rc::new(RefCell::new(HashMap::new())),
            handlers: Rc::new(RefCell::new(Handlers {
                on_tool_result: None,
                on_tool_input: None,
                on_host_context_changed: None,
            })),
            host_context: Rc::new(RefCell::new(None)),
        }
    }

    /// Set the callback for tool result notifications from the host.
    pub fn set_on_tool_result(&self, cb: impl Fn(Value) + 'static) {
        self.handlers.borrow_mut().on_tool_result = Some(Box::new(cb));
    }

    /// Set the callback for tool input notifications from the host.
    pub fn set_on_tool_input(&self, cb: impl Fn(Value) + 'static) {
        self.handlers.borrow_mut().on_tool_input = Some(Box::new(cb));
    }

    /// Set the callback for host context change notifications.
    pub fn set_on_host_context_changed(&self, cb: impl Fn(Value) + 'static) {
        self.handlers.borrow_mut().on_host_context_changed = Some(Box::new(cb));
    }

    /// Get the host context from initialization.
    pub fn get_host_context(&self) -> Option<HostContext> {
        self.host_context.borrow().clone()
    }

    /// Start listening for messages and connect to the MCP host.
    ///
    /// Sends `ui/initialize`, waits for the host response, then sends
    /// `ui/notifications/initialized`. Returns the host context.
    pub async fn connect(
        &self,
        app_name: &str,
        app_version: &str,
    ) -> Result<HostContext, String> {
        // Start the message listener
        self.start_listener();

        // Send ui/initialize
        let init_params = serde_json::json!({
            "appInfo": {
                "name": app_name,
                "version": app_version
            },
            "appCapabilities": {},
            "protocolVersion": PROTOCOL_VERSION
        });

        let result = self.send_request(METHOD_INITIALIZE, init_params).await?;

        // Extract host context
        let host_context: HostContext = result
            .get("hostContext")
            .cloned()
            .and_then(|v| serde_json::from_value(v).ok())
            .unwrap_or_default();

        *self.host_context.borrow_mut() = Some(host_context.clone());

        // Send ui/notifications/initialized
        self.send_notification(METHOD_INITIALIZED, serde_json::json!({}));

        Ok(host_context)
    }

    /// Call a server tool via the MCP host.
    pub async fn call_server_tool(
        &self,
        name: &str,
        args: &Value,
    ) -> Result<Value, String> {
        let params = serde_json::json!({
            "name": name,
            "arguments": args
        });
        let result = self.send_request(METHOD_TOOLS_CALL, params).await?;

        // Check for tool error
        if result.get("isError").and_then(|v| v.as_bool()).unwrap_or(false) {
            let error_text = result
                .get("content")
                .and_then(|c| c.as_array())
                .and_then(|arr| arr.first())
                .and_then(|item| item.get("text"))
                .and_then(|t| t.as_str())
                .unwrap_or("Unknown tool error");
            return Err(error_text.to_string());
        }

        // Extract text content and parse as JSON
        let text = result
            .get("content")
            .and_then(|c| c.as_array())
            .and_then(|arr| arr.first())
            .and_then(|item| item.get("text"))
            .and_then(|t| t.as_str())
            .ok_or("Empty response from server tool")?;

        serde_json::from_str(text).map_err(|e| format!("Failed to parse tool result: {e}"))
    }

    /// Open a URL in the MCP host's browser context.
    pub fn open_link(&self, url: &str) {
        let params = serde_json::json!({ "url": url });
        self.send_notification(METHOD_OPEN_LINK, params);
    }

    /// Measure content size and notify the host so it can resize the iframe.
    ///
    /// Mirrors the JS SDK's `setupSizeChangedNotifications()`: temporarily sets
    /// documentElement to fit-content, measures getBoundingClientRect, restores,
    /// and posts `ui/notifications/size-changed` with `{ width, height }`.
    pub fn send_size_changed(&self) {
        let Some(window) = web_sys::window() else { return };
        let Some(document) = window.document() else { return };
        let Some(root) = document.document_element() else { return };
        let Some(html_el) = root.clone().dyn_into::<web_sys::HtmlElement>().ok() else { return };

        let style = html_el.style();
        let saved_w = style.get_property_value("width").unwrap_or_default();
        let saved_h = style.get_property_value("height").unwrap_or_default();

        let _ = style.set_property("width", "fit-content");
        let _ = style.set_property("height", "max-content");

        let rect = root.get_bounding_client_rect();
        let content_width = rect.width();
        let content_height = rect.height();

        let _ = style.set_property("width", &saved_w);
        let _ = style.set_property("height", &saved_h);

        let scrollbar_w = window
            .inner_width()
            .ok()
            .and_then(|v| v.as_f64())
            .unwrap_or(content_width)
            - root.client_width() as f64;
        let width = (content_width + scrollbar_w).ceil() as u32;
        let height = content_height.ceil() as u32;

        self.send_notification(
            METHOD_SIZE_CHANGED,
            serde_json::json!({ "width": width, "height": height }),
        );
    }

    // -----------------------------------------------------------------------
    // Internal
    // -----------------------------------------------------------------------

    fn next_request_id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }

    fn get_parent_window() -> Option<web_sys::Window> {
        let window = web_sys::window()?;
        // In an iframe, window.parent is the host window
        window.parent().ok().flatten()
    }

    fn send_message(&self, message: &Value) {
        if let Some(parent) = Self::get_parent_window() {
            let json = serde_json::to_string(message).unwrap();
            let js_value = js_sys::JSON::parse(&json).unwrap();
            let _ = parent.post_message(&js_value, "*");
        }
    }

    fn send_notification(&self, method: &str, params: Value) {
        let msg = serde_json::json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params
        });
        self.send_message(&msg);
    }

    async fn send_request(
        &self,
        method: &'static str,
        params: Value,
    ) -> Result<Value, String> {
        let id = self.next_request_id();

        let msg = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params
        });

        // Create a future that resolves when the response arrives
        let (tx, rx) = futures::channel::oneshot::channel::<Result<Value, String>>();
        self.pending.borrow_mut().insert(id, Box::new(|result| {
            let _ = tx.send(result);
        }));

        self.send_message(&msg);

        rx.await.map_err(|_| "Request cancelled".to_string())?
    }

    fn start_listener(&self) {
        let pending = self.pending.clone();
        let handlers = self.handlers.clone();

        let parent = Self::get_parent_window();

        let handler = Closure::<dyn Fn(web_sys::MessageEvent)>::new(move |event: web_sys::MessageEvent| {
            // Accept messages from any source. In claude.ai's sandbox proxy
            // architecture, messages are relayed through the proxy frame so
            // event.source may not be a Window or may not match window.parent.
            // JSON-RPC parsing below filters out irrelevant messages.
            let _ = &parent; // keep parent alive for the closure's lifetime

            let data = event.data();
            let json_str = match js_sys::JSON::stringify(&data) {
                Ok(s) => String::from(s),
                Err(_) => return,
            };

            let msg: JsonRpcMessage = match serde_json::from_str(&json_str) {
                Ok(m) => m,
                Err(e) => {
                    web_sys::console::warn_1(&format!("[kyomi-mcp] JSON-RPC parse failed: {e}").into());
                    return;
                }
            };

            if let Some(method) = msg.method.as_deref() {
                web_sys::console::log_1(&format!("[kyomi-mcp] ← {method}").into());
            } else if let Some(id) = msg.id {
                web_sys::console::log_1(&format!("[kyomi-mcp] ← response id={id}").into());
            }

            if let Some(id) = msg.id {
                if msg.method.is_none() {
                    if let Some(cb) = pending.borrow_mut().remove(&id) {
                        if let Some(error) = msg.error {
                            let error_msg = error
                                .get("message")
                                .and_then(|m| m.as_str())
                                .unwrap_or("Unknown error")
                                .to_string();
                            cb(Err(error_msg));
                        } else {
                            cb(Ok(msg.result.unwrap_or(Value::Null)));
                        }
                    }
                    return;
                }
            }

            if let Some(method) = msg.method.as_deref() {
                let params = msg.params.unwrap_or(Value::Null);
                let h = handlers.borrow();
                match method {
                    METHOD_TOOL_RESULT => {
                        if let Some(ref cb) = h.on_tool_result {
                            cb(params);
                        } else {
                            web_sys::console::warn_1(&"[kyomi-mcp] tool-result arrived but no handler registered".into());
                        }
                    }
                    METHOD_TOOL_INPUT => {
                        if let Some(ref cb) = h.on_tool_input {
                            cb(params);
                        }
                    }
                    METHOD_HOST_CONTEXT_CHANGED => {
                        if let Some(ref cb) = h.on_host_context_changed {
                            cb(params);
                        }
                    }
                    _ => {}
                }
            }
        });

        let window = web_sys::window().unwrap();
        let _ = window.add_event_listener_with_callback(
            "message",
            handler.as_ref().unchecked_ref(),
        );
        handler.forget();
    }
}

// ---------------------------------------------------------------------------
// Theme helpers
// ---------------------------------------------------------------------------

/// Apply theme to the document (sets data-theme, color-scheme, and .dark class).
pub fn apply_theme(theme: &str) {
    if let Some(root) = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.document_element())
    {
        let _ = root.set_attribute("data-theme", theme);
        if let Some(style) = root.dyn_ref::<web_sys::HtmlElement>() {
            let _ = style.style().set_property("color-scheme", theme);
        }
        let class_list = root.class_list();
        if theme == "dark" {
            let _ = class_list.add_1("dark");
        } else {
            let _ = class_list.remove_1("dark");
        }
    }
}

/// Apply host style CSS variables to the document root.
pub fn apply_host_styles(styles: &HostStyles) {
    if let Some(root) = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.document_element())
        .and_then(|el| el.dyn_into::<web_sys::HtmlElement>().ok())
    {
        if let Some(ref vars) = styles.variables {
            let style = root.style();
            for (key, value) in vars {
                let _ = style.set_property(key, value);
            }
        }
    }

    // Inject font CSS if provided
    if let Some(ref css) = styles.css {
        if let Some(ref fonts) = css.fonts {
            if !fonts.is_empty() {
                if let Some(document) = web_sys::window().and_then(|w| w.document()) {
                    if let Ok(style_el) = document.create_element("style") {
                        style_el.set_text_content(Some(fonts));
                        if let Some(head) = document.head() {
                            let _ = head.append_child(&style_el);
                        }
                    }
                }
            }
        }
    }
}
