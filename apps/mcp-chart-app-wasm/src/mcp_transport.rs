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

/// Deserialize `value` as `T`, naming where in it a failure sat.
///
/// `serde_json`'s error from a `Value` names the offending type but not the
/// field it sat in, so one unexpected type deep in a host payload costs the
/// whole payload with nothing to say which part the app could not read. The
/// failure is reported as `<root>.<field path>: <serde's explanation>`, where
/// `root` names the payload the value came from (`hostContext`,
/// `params.styles`, ...) so the path starts at something the host sent.
pub fn deserialize_naming_path<T: serde::de::DeserializeOwned>(
    value: &Value,
    root: &str,
) -> Result<T, String> {
    serde_path_to_error::deserialize(value).map_err(|error| {
        let path = error.path().to_string();
        let location = if path == "." {
            root.to_string()
        } else {
            format!("{root}.{path}")
        };
        format!("{location}: {}", error.inner())
    })
}

/// Resolve the `hostContext` of a `ui/initialize` result, reporting one that
/// cannot be read.
///
/// `Some` only when the host sent a context and it was read. A host that sends
/// no context at all is valid, which the protocol allows: the result is `None`
/// and nothing is reported. A context that is present but unreadable — one bad
/// field type anywhere, including an explicit `null` — is also `None`, and is
/// reported through `log_error`, because falling back to a default silently is
/// what left charts on default colours and fonts with no explanation. The app
/// still renders, without host values.
///
/// `None` rather than `HostContext::default()` so a caller can tell values the
/// host sent from the absence of any: an all-`None` default context is
/// indistinguishable from a real host that chose to send nothing in it.
///
/// `log_error` is a parameter so the decision and the message it builds are
/// tested together, natively: `connect()` passes `console::error_1`, which
/// aborts a native test process rather than recording anything, so a message
/// assembled in `connect()` could not be asserted at all. What stays outside
/// native coverage is the single forwarding call in `connect()`; the browser
/// proof is what observes that line reach the console.
fn host_context_from_initialize(
    result: &Value,
    log_error: &mut dyn FnMut(&str),
) -> Option<HostContext> {
    let value = result.get("hostContext")?;
    match deserialize_naming_path::<HostContext>(value, "hostContext") {
        Ok(host_context) => Some(host_context),
        Err(detail) => {
            log_error(&format!("[kyomi-mcp] ui/initialize host context ignored: {detail}"));
            None
        }
    }
}

/// The size to post to the host for a measured document, or `None` when the
/// measurement is not a size the host should act on.
///
/// A document measured before layout, or one with nothing in it, is `0 x 0`
/// (or has a zero side); posting that makes the host collapse the iframe to
/// nothing, and the chart then has no room to lay out and report a real size.
/// Dimensions that are not finite positive numbers are treated the same way
/// rather than being cast to an arbitrary integer.
fn size_to_report(content_width: f64, content_height: f64, scrollbar_width: f64) -> Option<(u32, u32)> {
    let width = (content_width + scrollbar_width).ceil();
    let height = content_height.ceil();
    if !(width.is_finite() && height.is_finite() && width > 0.0 && height > 0.0) {
        return None;
    }
    Some((width as u32, height as u32))
}

// ---------------------------------------------------------------------------
// Pending request tracking
// ---------------------------------------------------------------------------

type PendingCallback = Box<dyn FnOnce(Result<Value, String>)>;
type PendingMap = Rc<RefCell<HashMap<u64, PendingCallback>>>;

/// How long a request waits for the host's answer before failing. The MCP
/// TypeScript SDK's own default request timeout, which the `ext-apps` SDK this
/// transport replaces inherits.
const REQUEST_TIMEOUT_MS: i32 = 60_000;

/// Fail the request `id` because the host never answered it.
///
/// Removing the entry is what makes this safe to run after the host did answer:
/// the response path removes the entry first, so a timer that fires late finds
/// nothing and does nothing, and a request is never resolved twice.
fn expire_pending_request(pending: &PendingMap, id: u64, method: &str, timeout_ms: i32) {
    let Some(callback) = pending.borrow_mut().remove(&id) else {
        return;
    };
    callback(Err(format!(
        "{method} request timed out: the host did not answer within {} s",
        timeout_ms / 1000
    )));
}

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

impl Default for McpTransport {
    fn default() -> Self {
        Self::new()
    }
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

    /// The host context the host sent during initialization.
    ///
    /// `None` until `connect()` has completed, and afterwards when the host sent
    /// no context or one this build could not read, so `Some` always means the
    /// values in it came from the host. Never a stand-in default: code that acts
    /// on a host value must not mistake the absence of one for the host's choice.
    pub fn get_host_context(&self) -> Option<HostContext> {
        self.host_context.borrow().clone()
    }

    /// Start listening for messages and connect to the MCP host.
    ///
    /// Sends `ui/initialize`, waits for the host response, then sends
    /// `ui/notifications/initialized`. Returns the host context, or `None` when
    /// the host sent none or one that could not be read (see
    /// [`Self::get_host_context`]).
    pub async fn connect(
        &self,
        app_name: &str,
        app_version: &str,
    ) -> Result<Option<HostContext>, String> {
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

        // Extract host context. A host that sends none is valid; one that sends
        // a context this build cannot read is reported, not silently replaced.
        let mut log_error = |message: &str| web_sys::console::error_1(&message.into());
        let host_context = host_context_from_initialize(&result, &mut log_error);

        *self.host_context.borrow_mut() = host_context.clone();

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

        // Layout has not run yet, or the document has no content: there is no
        // size to report, and the host would collapse the frame to it. The
        // ResizeObserver reports again once layout gives the document a size.
        let Some((width, height)) = size_to_report(content_width, content_height, scrollbar_w)
        else {
            return;
        };

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

        let window = web_sys::window().ok_or("No window to schedule the request timeout")?;

        // Create a future that resolves when the response arrives
        let (tx, rx) = futures::channel::oneshot::channel::<Result<Value, String>>();
        self.pending.borrow_mut().insert(id, Box::new(|result| {
            let _ = tx.send(result);
        }));

        // A host that never answers must not leave this request, and its
        // pending entry, waiting forever. The timer fails the request and
        // removes the entry; a response that arrives first removes the entry
        // itself, which turns the timer into a no-op.
        let pending = self.pending.clone();
        let on_timeout = Closure::once(move || {
            expire_pending_request(&pending, id, method, REQUEST_TIMEOUT_MS);
        });
        let scheduled = window.set_timeout_with_callback_and_timeout_and_arguments_0(
            on_timeout.as_ref().unchecked_ref(),
            REQUEST_TIMEOUT_MS,
        );
        if let Err(error) = scheduled {
            self.pending.borrow_mut().remove(&id);
            return Err(format!("Could not schedule the {method} request timeout: {error:?}"));
        }
        on_timeout.forget();

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

/// Element id for the host-provided font CSS block, so re-applying styles
/// replaces it instead of stacking a new `<style>` for every host context
/// change.
const HOST_FONTS_STYLE_ID: &str = "kyomi-host-fonts";

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
                    let style_el = match document.get_element_by_id(HOST_FONTS_STYLE_ID) {
                        Some(existing) => existing,
                        None => match document.create_element("style") {
                            Ok(el) => {
                                el.set_id(HOST_FONTS_STYLE_ID);
                                if let Some(head) = document.head() {
                                    let _ = head.append_child(&el);
                                }
                                el
                            }
                            Err(_) => return,
                        },
                    };
                    style_el.set_text_content(Some(fonts));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Run the production reporting path and return the context plus every
    /// message it asked to log. This is the same function `connect()` calls, so
    /// a version that stops logging a context it cannot read fails these tests.
    fn initialize_with(result: &Value) -> (Option<HostContext>, Vec<String>) {
        let mut logged = Vec::new();
        let mut log_error = |message: &str| logged.push(message.to_string());
        let context = host_context_from_initialize(result, &mut log_error);
        (context, logged)
    }

    #[test]
    fn valid_host_context_is_read() {
        let result = serde_json::json!({
            "protocolVersion": PROTOCOL_VERSION,
            "hostContext": {
                "theme": "dark",
                "displayMode": "inline",
                "locale": "en-GB",
                "styles": {
                    "variables": { "--color-background-primary": "#111111" },
                    "css": { "fonts": "@font-face { font-family: Inter; }" }
                }
            }
        });

        let (context, logged) = initialize_with(&result);

        assert_eq!(
            logged,
            Vec::<String>::new(),
            "a context in the protocol's shape must be read without a report"
        );
        let context = context.expect("a readable context is the host's context");
        assert_eq!(context.theme.as_deref(), Some("dark"));
        assert_eq!(context.display_mode.as_deref(), Some("inline"));
        assert_eq!(context.locale.as_deref(), Some("en-GB"));
        let styles = context.styles.expect("styles were sent");
        assert_eq!(
            styles.variables.expect("variables were sent")["--color-background-primary"],
            "#111111"
        );
        assert_eq!(
            styles.css.expect("css was sent").fonts.as_deref(),
            Some("@font-face { font-family: Inter; }")
        );
    }

    #[test]
    fn missing_host_context_is_not_an_error() {
        let result = serde_json::json!({ "protocolVersion": PROTOCOL_VERSION });

        let (context, logged) = initialize_with(&result);

        assert_eq!(
            logged,
            Vec::<String>::new(),
            "a host that sends no context must not be reported as one that failed"
        );
        assert!(
            context.is_none(),
            "a host that sends no context must not be given a default one that looks like its own"
        );
    }

    #[test]
    fn malformed_host_context_reports_the_field_that_failed() {
        // One unexpected type deep inside `styles` is enough to cost the whole
        // context, so the report has to name that field.
        let result = serde_json::json!({
            "hostContext": {
                "theme": "dark",
                "styles": { "variables": { "--color-background-primary": 17 } }
            }
        });

        let (context, logged) = initialize_with(&result);

        assert_eq!(
            logged.len(),
            1,
            "an unreadable context must produce exactly one report, got: {logged:?}"
        );
        let message = &logged[0];
        assert!(
            message.starts_with("[kyomi-mcp] ui/initialize host context ignored: "),
            "the report must carry the app's console prefix, got: {message}"
        );
        assert!(
            message.contains("styles.variables.--color-background-primary"),
            "the report must name the field that failed, got: {message}"
        );
        assert!(
            message.contains("invalid type: integer `17`, expected a string"),
            "the report must carry serde's own explanation, got: {message}"
        );
        assert!(
            context.is_none(),
            "the unreadable context must not be half-applied or replaced by a default"
        );
    }

    #[test]
    fn null_host_context_is_reported_not_treated_as_absent() {
        let result = serde_json::json!({ "hostContext": null });

        let (context, logged) = initialize_with(&result);

        assert_eq!(
            logged.len(),
            1,
            "an explicit null context is malformed and must be reported, got: {logged:?}"
        );
        let message = &logged[0];
        assert!(
            message.starts_with("[kyomi-mcp] ui/initialize host context ignored: "),
            "the report must carry the app's console prefix, got: {message}"
        );
        assert!(
            message.contains("expected struct HostContext"),
            "the report must say what the host sent instead, got: {message}"
        );
        assert!(
            context.is_none(),
            "the null context must not be replaced by a default that looks like the host's"
        );
    }

    #[test]
    fn a_host_that_sends_an_empty_context_is_distinguishable_from_one_that_sent_none() {
        // `{}` is a real host context that happens to set nothing; it must not
        // be confused with the absence of a context.
        let sent = serde_json::json!({ "hostContext": {} });
        let absent = serde_json::json!({});

        assert!(initialize_with(&sent).0.is_some());
        assert!(initialize_with(&absent).0.is_none());
    }

    #[test]
    fn deserialize_naming_path_names_the_root_when_the_whole_value_is_wrong() {
        let error = deserialize_naming_path::<HostContext>(&serde_json::json!(5), "params")
            .expect_err("a number is not a host context");
        assert!(
            error.starts_with("params: "),
            "a failure at the top must name the root alone, got: {error}"
        );
    }

    // -----------------------------------------------------------------------
    // Request timeout
    // -----------------------------------------------------------------------

    /// Results delivered to a pending request's callback, in delivery order.
    type Outcomes = Rc<RefCell<Vec<Result<Value, String>>>>;

    /// A pending entry whose delivered result is recorded in `outcomes`.
    fn pending_with_recorder(id: u64) -> (PendingMap, Outcomes) {
        let outcomes = Rc::new(RefCell::new(Vec::new()));
        let pending: PendingMap = Rc::new(RefCell::new(HashMap::new()));
        let recorder = outcomes.clone();
        pending.borrow_mut().insert(
            id,
            Box::new(move |result| recorder.borrow_mut().push(result)),
        );
        (pending, outcomes)
    }

    #[test]
    fn an_unanswered_request_fails_with_a_clear_error_and_leaves_no_pending_entry() {
        let (pending, outcomes) = pending_with_recorder(7);

        expire_pending_request(&pending, 7, METHOD_TOOLS_CALL, 60_000);

        assert!(
            pending.borrow().is_empty(),
            "the timed-out request's pending entry must be removed"
        );
        let outcomes = outcomes.borrow();
        assert_eq!(outcomes.len(), 1, "the request must be failed exactly once");
        let error = outcomes[0].as_ref().expect_err("a timeout is a failure");
        assert!(
            error.contains("tools/call") && error.contains("timed out") && error.contains("60 s"),
            "the error must name the request and the wait, got: {error}"
        );
    }

    #[test]
    fn a_timer_that_fires_after_the_host_answered_does_nothing() {
        let (pending, outcomes) = pending_with_recorder(3);
        // The response path removes the entry before delivering the answer.
        let callback = pending.borrow_mut().remove(&3).expect("entry was inserted");
        callback(Ok(serde_json::json!({ "ok": true })));

        expire_pending_request(&pending, 3, METHOD_TOOLS_CALL, 60_000);

        let outcomes = outcomes.borrow();
        assert_eq!(outcomes.len(), 1, "the answered request must not be failed again");
        assert!(outcomes[0].is_ok());
    }

    #[test]
    fn a_timeout_only_fails_the_request_it_belongs_to() {
        let (pending, first) = pending_with_recorder(1);
        let second = Rc::new(RefCell::new(Vec::new()));
        let recorder = second.clone();
        pending.borrow_mut().insert(
            2,
            Box::new(move |result| recorder.borrow_mut().push(result)),
        );

        expire_pending_request(&pending, 1, METHOD_INITIALIZE, 60_000);

        assert_eq!(first.borrow().len(), 1);
        assert!(second.borrow().is_empty(), "another request must keep waiting");
        assert!(pending.borrow().contains_key(&2));
    }

    // -----------------------------------------------------------------------
    // Size reporting
    // -----------------------------------------------------------------------

    #[test]
    fn a_document_with_no_layout_reports_no_size() {
        assert_eq!(size_to_report(0.0, 0.0, 0.0), None);
    }

    #[test]
    fn a_document_with_a_zero_side_reports_no_size() {
        assert_eq!(size_to_report(640.0, 0.0, 0.0), None);
        assert_eq!(size_to_report(0.0, 480.0, 0.0), None);
    }

    #[test]
    fn a_measurement_that_is_not_a_finite_size_reports_no_size() {
        assert_eq!(size_to_report(f64::NAN, 480.0, 0.0), None);
        assert_eq!(size_to_report(640.0, f64::INFINITY, 0.0), None);
        assert_eq!(size_to_report(-1.0, 480.0, 0.0), None);
    }

    #[test]
    fn a_laid_out_document_reports_its_size_rounded_up_with_the_scrollbar() {
        assert_eq!(size_to_report(640.2, 480.4, 15.0), Some((656, 481)));
    }
}
