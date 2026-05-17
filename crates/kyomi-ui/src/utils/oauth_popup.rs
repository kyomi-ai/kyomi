// SPDX-License-Identifier: AGPL-3.0-or-later

//! OAuth popup utility — opens centered popup windows and parses postMessage
//! responses for the OAuth connect flows.
//!
//! All public functions in this module are WASM-only; they are gated with
//! `#[cfg(target_arch = "wasm32")]` so they compile only on the browser target.
//! The `OAuthMessage` enum is always compiled so server-side code can reference
//! it if needed (e.g., for `ServerFn` result types).

// ─────────────────────────────────────────────────────────────────────────────
// Message types (always compiled — no cfg gate)
// ─────────────────────────────────────────────────────────────────────────────

/// A parsed OAuth postMessage event received from a popup window.
///
/// Variants correspond to the provider-specific message type strings sent by
/// the OAuth callback pages after the provider redirects back to Kyomi.
#[derive(Clone, Debug)]
pub enum OAuthMessage {
    GoogleSuccess { email: Option<String> },
    GoogleError { error: String },
    SnowflakeSuccess { email: Option<String> },
    SnowflakeError { error: String },
    DatabricksSuccess { email: Option<String> },
    DatabricksError { error: String },
    MicrosoftSuccess { email: Option<String> },
    MicrosoftError { error: String },
    MicrosoftEnterpriseSuccess { email: Option<String> },
    MicrosoftEnterpriseError { error: String },
    BigqueryEnterpriseSuccess { email: Option<String> },
    BigqueryEnterpriseError { error: String },
}

// ─────────────────────────────────────────────────────────────────────────────
// WASM-only functions
// ─────────────────────────────────────────────────────────────────────────────

/// Open a centered OAuth popup window.
///
/// The popup is 500×600 and is positioned in the center of the opener window.
/// The window name is `{provider_name}-oauth`.
///
/// Returns `None` if the browser blocked the popup (e.g. no user gesture) or
/// the window was immediately closed. Callers should show a user-facing
/// explanation when `None` is returned.
#[cfg(target_arch = "wasm32")]
pub fn open_oauth_popup(url: &str, provider_name: &str) -> Option<web_sys::Window> {
    let window = web_sys::window()?;

    let width: i32 = 500;
    let height: i32 = 600;

    let screen_x = window
        .screen_x()
        .ok()
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0) as i32;
    let outer_width = window
        .outer_width()
        .ok()
        .and_then(|v| v.as_f64())
        .unwrap_or(1024.0) as i32;
    let screen_y = window
        .screen_y()
        .ok()
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0) as i32;
    let outer_height = window
        .outer_height()
        .ok()
        .and_then(|v| v.as_f64())
        .unwrap_or(768.0) as i32;

    let left = screen_x + (outer_width - width) / 2;
    let top = screen_y + (outer_height - height) / 2;

    let features = format!("width={width},height={height},left={left},top={top},popup=1");
    let window_name = format!("{provider_name}-oauth");

    let popup = window
        .open_with_url_and_target_and_features(url, &window_name, &features)
        .ok()??;

    // Check immediately if the popup was blocked or closed right away
    if popup.closed().unwrap_or(true) {
        return None;
    }

    Some(popup)
}

/// Parse a `message` event into an [`OAuthMessage`] if it matches a known
/// OAuth type string and the origin matches the current window's origin.
///
/// Returns `None` for:
/// - Events whose origin does not match `window.location.origin`
/// - Events that do not have a `type` field
/// - Events whose `type` is not a recognized OAuth message type
#[cfg(target_arch = "wasm32")]
pub fn parse_oauth_message(event: &web_sys::MessageEvent) -> Option<OAuthMessage> {
    use wasm_bindgen::JsValue;

    // 1. Validate origin — only accept messages from our own origin
    let expected_origin = web_sys::window()
        .and_then(|w| w.location().origin().ok())
        .unwrap_or_default();
    if event.origin() != expected_origin {
        return None;
    }

    // 2. Extract the `type` field from the message data
    let data = event.data();
    let msg_type = js_sys::Reflect::get(&data, &JsValue::from_str("type"))
        .ok()
        .and_then(|v| v.as_string())?;

    // 3. Helper closures for extracting email and error from the data object
    let extract_email = || -> Option<String> {
        let data_obj = js_sys::Reflect::get(&data, &JsValue::from_str("data")).ok()?;
        // Try `email` first, then `provider_email`
        js_sys::Reflect::get(&data_obj, &JsValue::from_str("email"))
            .ok()
            .and_then(|v| v.as_string())
            .or_else(|| {
                js_sys::Reflect::get(&data_obj, &JsValue::from_str("provider_email"))
                    .ok()
                    .and_then(|v| v.as_string())
            })
    };

    let extract_error = || -> String {
        js_sys::Reflect::get(&data, &JsValue::from_str("error"))
            .ok()
            .and_then(|v| v.as_string())
            .unwrap_or_else(|| "Unknown OAuth error".to_string())
    };

    // 4. Match type string to enum variant
    match msg_type.as_str() {
        "GOOGLE_OAUTH_SUCCESS" => Some(OAuthMessage::GoogleSuccess {
            email: extract_email(),
        }),
        "GOOGLE_OAUTH_ERROR" => Some(OAuthMessage::GoogleError {
            error: extract_error(),
        }),
        "SNOWFLAKE_OAUTH_SUCCESS" => Some(OAuthMessage::SnowflakeSuccess {
            email: extract_email(),
        }),
        "SNOWFLAKE_OAUTH_ERROR" => Some(OAuthMessage::SnowflakeError {
            error: extract_error(),
        }),
        "DATABRICKS_OAUTH_SUCCESS" => Some(OAuthMessage::DatabricksSuccess {
            email: extract_email(),
        }),
        "DATABRICKS_OAUTH_ERROR" => Some(OAuthMessage::DatabricksError {
            error: extract_error(),
        }),
        "MICROSOFT_OAUTH_SUCCESS" => Some(OAuthMessage::MicrosoftSuccess {
            email: extract_email(),
        }),
        "MICROSOFT_OAUTH_ERROR" => Some(OAuthMessage::MicrosoftError {
            error: extract_error(),
        }),
        "MICROSOFT_ENTERPRISE_OAUTH_SUCCESS" => Some(OAuthMessage::MicrosoftEnterpriseSuccess {
            email: extract_email(),
        }),
        "MICROSOFT_ENTERPRISE_OAUTH_ERROR" => Some(OAuthMessage::MicrosoftEnterpriseError {
            error: extract_error(),
        }),
        "BIGQUERY_ENTERPRISE_OAUTH_SUCCESS" => Some(OAuthMessage::BigqueryEnterpriseSuccess {
            email: extract_email(),
        }),
        "BIGQUERY_ENTERPRISE_OAUTH_ERROR" => Some(OAuthMessage::BigqueryEnterpriseError {
            error: extract_error(),
        }),
        _ => None,
    }
}

/// Build and send a success OAuth postMessage to the opener window.
///
/// `msg_type` is the full type string (e.g. `"GOOGLE_OAUTH_SUCCESS"`).
/// `email` is the linked account email if available — written into both the
/// `email` and `provider_email` fields of the `data` sub-object so consumers
/// can use either key name.
#[cfg(target_arch = "wasm32")]
pub fn send_oauth_success_to_opener(opener: &web_sys::Window, msg_type: &str, email: Option<&str>) {
    let origin = web_sys::window()
        .and_then(|w| w.location().origin().ok())
        .unwrap_or_default();

    let msg = js_sys::Object::new();
    js_sys::Reflect::set(&msg, &"type".into(), &msg_type.into()).ok();

    let data = js_sys::Object::new();
    if let Some(e) = email {
        js_sys::Reflect::set(&data, &"email".into(), &e.into()).ok();
        js_sys::Reflect::set(&data, &"provider_email".into(), &e.into()).ok();
    }
    js_sys::Reflect::set(&msg, &"data".into(), &data).ok();

    opener.post_message(&msg, &origin).ok();
}

/// Build and send an error OAuth postMessage to the opener window.
///
/// `msg_type` is the full type string (e.g. `"GOOGLE_OAUTH_ERROR"`).
/// `error` is the human-readable error message.
#[cfg(target_arch = "wasm32")]
pub fn send_oauth_error_to_opener(opener: &web_sys::Window, msg_type: &str, error: &str) {
    let origin = web_sys::window()
        .and_then(|w| w.location().origin().ok())
        .unwrap_or_default();

    let msg = js_sys::Object::new();
    js_sys::Reflect::set(&msg, &"type".into(), &msg_type.into()).ok();
    js_sys::Reflect::set(&msg, &"error".into(), &error.into()).ok();

    opener.post_message(&msg, &origin).ok();
}

/// Install a `message` event listener on `window` for OAuth popup events.
///
/// The provided `callback` is called each time a recognized [`OAuthMessage`]
/// arrives (unrecognized messages and wrong-origin messages are silently
/// ignored). Returns a `FnOnce()` cleanup closure — call it (e.g. in
/// `on_cleanup`) to remove the listener and release the backing JS closure.
///
/// # Usage
/// ```rust,ignore
/// let cleanup = install_oauth_listener(move |msg| {
///     match msg {
///         OAuthMessage::GoogleSuccess { email } => { /* handle */ }
///         _ => {}
///     }
/// });
/// on_cleanup(move || cleanup());
/// ```
#[cfg(target_arch = "wasm32")]
pub fn install_oauth_listener(
    callback: impl Fn(OAuthMessage) + 'static,
) -> impl FnOnce() {
    use wasm_bindgen::prelude::*;
    use wasm_bindgen::JsCast;

    let closure = Closure::<dyn Fn(web_sys::MessageEvent)>::new(
        move |event: web_sys::MessageEvent| {
            if let Some(msg) = parse_oauth_message(&event) {
                callback(msg);
            }
        },
    );

    // Register the listener — store the function reference so we can remove it
    let listener_fn = closure.as_ref().unchecked_ref::<js_sys::Function>().clone();

    if let Some(window) = web_sys::window() {
        let _ = window.add_event_listener_with_callback("message", &listener_fn);
    }

    // Return a cleanup closure that removes the listener and drops the Closure.
    // Capturing `closure` by value keeps the JS function alive until cleanup.
    move || {
        if let Some(window) = web_sys::window() {
            let _ = window.remove_event_listener_with_callback("message", &listener_fn);
        }
        drop(closure);
    }
}
