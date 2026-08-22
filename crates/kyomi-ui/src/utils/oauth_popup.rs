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

/// Resolve the window that opened this popup, if there is one.
///
/// Returns `None` when the document was not opened by another window (a normal
/// top-level navigation), and `Some` for a genuine popup.
///
/// # Why this must not use `dyn_into` / `instanceof`
///
/// `window.opener` is a `WindowProxy` belonging to a **different JavaScript
/// realm**. `JsCast::dyn_into::<web_sys::Window>()` compiles to an `instanceof`
/// against *this* realm's `Window` constructor, and every realm has its own —
/// so the check is `false` for a live, same-origin, fully usable opener. The
/// object is fine; only the type test is wrong.
///
/// Both OAuth callback pages previously used `dyn_into` here, so the opener
/// always resolved to `None`, the popup never posted its result back, and every
/// provider's connect button hung on "Connecting..." forever (KYO-436). Measured
/// in a real browser, `window.opener` reports `[object Window]` with a callable
/// `postMessage` while `window.opener instanceof Window` is `false`.
///
/// A null check plus `unchecked_into` is the correct pattern here: nothing in
/// this flow ever assigns `window.opener` to anything other than what the
/// browser set it to when the popup was opened (a `WindowProxy`, or `null`
/// for a normal top-level navigation), so once null and undefined are
/// excluded, what's left is a `WindowProxy`. Note this is a property of how
/// Kyomi uses `opener`, not a platform guarantee — `window.opener` has an
/// unrestricted setter and can legally be reassigned to anything by page
/// script, including by the opened page itself.
#[cfg(target_arch = "wasm32")]
pub fn opener_window() -> Option<web_sys::Window> {
    use wasm_bindgen::JsCast;

    web_sys::window()
        .and_then(|w| w.opener().ok())
        .filter(|o| !o.is_null() && !o.is_undefined())
        .map(|o| o.unchecked_into::<web_sys::Window>())
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

#[cfg(test)]
mod tests {
    //! Source-level guards for KYO-436.
    //!
    //! The defect these cover is invisible to a host-target unit test: it is a
    //! JavaScript realm-boundary behaviour that only manifests in a browser.
    //! What *can* be enforced here is that neither callback page reintroduces
    //! the `instanceof`-based cast, and that both keep going through the one
    //! shared helper. See `scripts/e2e-regression/oauth-opener-realm.cjs` for
    //! the matching real-browser assertion.

    const GOOGLE_LINK_CALLBACK: &str = include_str!("../pages/auth/google_link_callback.rs");
    const DATASOURCE_OAUTH_CALLBACK: &str =
        include_str!("../pages/auth/datasource_oauth_callback.rs");

    /// `dyn_into::<web_sys::Window>()` compiles to an `instanceof` against the
    /// current realm's `Window`. `window.opener` comes from another realm, so
    /// the test is always `false` and the opener resolves to `None` — the
    /// popup then never posts its result back and the parent hangs on
    /// "Connecting..." forever, for every OAuth provider.
    #[test]
    fn callback_pages_never_instanceof_test_the_opener() {
        for (name, src) in [
            ("google_link_callback.rs", GOOGLE_LINK_CALLBACK),
            ("datasource_oauth_callback.rs", DATASOURCE_OAUTH_CALLBACK),
        ] {
            assert!(
                !src.contains("dyn_into::<web_sys::Window>"),
                "{name} casts the opener with dyn_into, which is a cross-realm `instanceof` \
                 and always fails — window.opener is a WindowProxy from another realm. Use \
                 `opener_window()` (null check + unchecked_into) instead (KYO-436)."
            );
        }
    }

    /// Both pages must resolve the opener through the single shared helper.
    /// Two hand-rolled copies of this logic are exactly how one bug came to
    /// break all five OAuth providers at once.
    #[test]
    fn callback_pages_resolve_the_opener_through_the_shared_helper() {
        for (name, src) in [
            ("google_link_callback.rs", GOOGLE_LINK_CALLBACK),
            ("datasource_oauth_callback.rs", DATASOURCE_OAUTH_CALLBACK),
        ] {
            assert!(
                src.contains("opener_window()"),
                "{name} must resolve window.opener via oauth_popup::opener_window() rather \
                 than rolling its own cast (KYO-436)."
            );
        }
    }

    /// The helper itself must keep the null/undefined guard. Dropping it would
    /// turn "not a popup" into a bogus `Some(window)` whose `postMessage` calls
    /// throw at runtime — the opposite failure, and a harder one to spot.
    #[test]
    fn opener_window_guards_null_before_casting_unchecked() {
        let src = include_str!("oauth_popup.rs");
        let helper = src
            .split_once("pub fn opener_window()")
            .expect("opener_window must exist")
            .1;
        let body = &helper[..helper.find("\n}").expect("helper must terminate")];
        assert!(
            body.contains("is_null()") && body.contains("is_undefined()"),
            "opener_window must reject null/undefined before unchecked_into — otherwise a \
             non-popup document resolves to a Some(..) that throws on use (KYO-436)."
        );
    }
}
