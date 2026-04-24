// SPDX-License-Identifier: AGPL-3.0-or-later

//! Feedback context collector — passively captures console errors, failed
//! requests, and browser/OS information for feedback submissions.
//!
//! Port of `apps/frontend/src/lib/feedbackContext.js`. All code is WASM-only
//! since it relies on browser APIs (console, navigator, window).

#[cfg(target_arch = "wasm32")]
use wasm_bindgen::prelude::*;

/// Inline JS module that monkey-patches `console.error` to capture the last
/// 10 errors.
///
/// Exposed functions:
/// - `initInterceptor()` — patches console.error once
/// - `getConsoleErrors()` — returns JSON string of captured errors
/// - `clearContext()` — clears errors (preserves init)
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(inline_js = r#"
const MAX_ERRORS = 10;
let consoleErrors = [];
let initialized = false;

export function initInterceptor() {
    if (initialized) return;
    const orig = console.error;
    console.error = function(...args) {
        const message = args.map(a => {
            if (a instanceof Error) return `${a.name}: ${a.message}`;
            if (typeof a === 'object') {
                try { return JSON.stringify(a); } catch { return String(a); }
            }
            return String(a);
        }).join(' ');
        consoleErrors.push({
            level: 'error',
            message,
            timestamp: new Date().toISOString(),
        });
        if (consoleErrors.length > MAX_ERRORS) consoleErrors.shift();
        orig.apply(console, args);
    };
    initialized = true;
}

export function getConsoleErrors() {
    return JSON.stringify(consoleErrors);
}

export function clearContext() {
    consoleErrors = [];
}
"#)]
extern "C" {
    #[wasm_bindgen(js_name = "initInterceptor")]
    fn init_interceptor();

    #[wasm_bindgen(js_name = "getConsoleErrors")]
    fn get_console_errors_js() -> String;

    #[wasm_bindgen(js_name = "clearContext")]
    fn clear_context_js();
}

/// Initialise the console.error interceptor. Safe to call multiple times —
/// only the first call patches `console.error`.
#[cfg(target_arch = "wasm32")]
pub fn init() {
    init_interceptor();
}

/// Collect the full context blob for a feedback submission.
///
/// Returns a JSON string containing:
/// - `url` — current page URL path
/// - `browser` — user agent string
/// - `os` — extracted OS from user agent
/// - `screen_width` / `screen_height`
/// - `console_errors` — last 10 captured errors
#[cfg(target_arch = "wasm32")]
pub fn collect_context() -> String {
    let window = web_sys::window().expect("window");

    let url = window
        .location()
        .pathname()
        .unwrap_or_else(|_| String::from("/"));

    let ua = window
        .navigator()
        .user_agent()
        .unwrap_or_default();

    let os = extract_os(&ua);
    let browser = extract_browser(&ua);

    let screen_width = window
        .inner_width()
        .ok()
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0) as u32;

    let screen_height = window
        .inner_height()
        .ok()
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0) as u32;

    let console_errors = get_console_errors_js();

    // Build JSON manually to avoid pulling in serde_json on the WASM side.
    // The values are either pre-serialised JSON arrays (from JS) or simple
    // strings that we escape.
    format!(
        r#"{{"url":"{}","browser":"{}","os":"{}","screen_width":{},"screen_height":{},"console_errors":{}}}"#,
        escape_json_string(&url),
        escape_json_string(&browser),
        escape_json_string(&os),
        screen_width,
        screen_height,
        console_errors,
    )
}

/// Clear captured errors and failed requests after a successful submission.
#[cfg(target_arch = "wasm32")]
pub fn clear() {
    clear_context_js();
}

/// Extract browser name + version from user agent string.
#[cfg(target_arch = "wasm32")]
pub fn extract_browser(ua: &str) -> String {
    // Edge (must check before Chrome since Edge also contains "Chrome")
    if let Some(pos) = ua.find("Edg/") {
        let version = &ua[pos + 4..];
        let end = version.find(' ').unwrap_or(version.len());
        return format!("Edge {}", &version[..end]);
    }
    // Chrome
    if let Some(pos) = ua.find("Chrome/") {
        let version = &ua[pos + 7..];
        let end = version.find(' ').unwrap_or(version.len());
        return format!("Chrome {}", &version[..end]);
    }
    // Firefox
    if let Some(pos) = ua.find("Firefox/") {
        let version = &ua[pos + 8..];
        let end = version.find(' ').unwrap_or(version.len());
        return format!("Firefox {}", &version[..end]);
    }
    // Safari
    if ua.contains("Safari") {
        if let Some(pos) = ua.find("Version/") {
            let version = &ua[pos + 8..];
            let end = version.find(' ').unwrap_or(version.len());
            return format!("Safari {}", &version[..end]);
        }
    }
    "Unknown Browser".to_string()
}

/// Extract OS name from user agent string.
#[cfg(target_arch = "wasm32")]
pub fn extract_os(ua: &str) -> String {
    if ua.contains("Mac OS X") {
        if let Some(pos) = ua.find("Mac OS X ") {
            let rest = &ua[pos + 9..];
            let end = rest.find(|c: char| !c.is_ascii_digit() && c != '_' && c != '.')
                .unwrap_or(rest.len());
            let version = rest[..end].replace('_', ".");
            return format!("macOS {version}");
        }
        return "macOS".to_string();
    }
    if ua.contains("Windows") {
        if ua.contains("Windows NT 10.0") {
            return "Windows 10/11".to_string();
        }
        return "Windows".to_string();
    }
    if ua.contains("Android") {
        if let Some(pos) = ua.find("Android ") {
            let rest = &ua[pos + 8..];
            let end = rest.find(|c: char| !c.is_ascii_digit() && c != '.')
                .unwrap_or(rest.len());
            return format!("Android {}", &rest[..end]);
        }
        return "Android".to_string();
    }
    if ua.contains("Linux") {
        return "Linux".to_string();
    }
    if ua.contains("iPhone") || ua.contains("iPad") {
        if let Some(pos) = ua.find("OS ") {
            let rest = &ua[pos + 3..];
            let end = rest.find(|c: char| !c.is_ascii_digit() && c != '_')
                .unwrap_or(rest.len());
            return format!("iOS {}", rest[..end].replace('_', "."));
        }
        return "iOS".to_string();
    }
    "Unknown OS".to_string()
}

/// Minimal JSON string escaper (for embedding values in hand-built JSON).
#[cfg(target_arch = "wasm32")]
pub fn escape_json_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out
}
