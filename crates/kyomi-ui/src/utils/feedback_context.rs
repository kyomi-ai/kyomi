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

/// Return the captured console errors as a **pre-serialised JSON array**.
///
/// Interpolate the result into hand-built JSON unescaped (`{}`), the way
/// `collect_context` below does. Passing it through `escape_json_string`
/// would double-encode the array into a JSON *string* whose contents happen
/// to look like an array, which no consumer of `context::jsonb` can read
/// back as a list (KYO-682).
///
/// The return value is always valid JSON, and never the empty string: the
/// interceptor's `consoleErrors` buffer is initialised at the inline module's
/// top level, so the module's own evaluation — not `init` — is what
/// establishes it. Callers therefore need no empty/fallback guard, even on a
/// page where `init` was never called; the worst case is `"[]"`.
#[cfg(target_arch = "wasm32")]
pub fn get_console_errors() -> String {
    get_console_errors_js()
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

    let console_errors = get_console_errors();

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
    if ua.contains("Safari")
        && let Some(pos) = ua.find("Version/") {
            let version = &ua[pos + 8..];
            let end = version.find(' ').unwrap_or(version.len());
            return format!("Safari {}", &version[..end]);
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

#[cfg(test)]
mod tests {
    //! Source-level guards for KYO-682.
    //!
    //! `panic_overlay` is declared `#[cfg(target_arch = "wasm32")]` in
    //! `lib.rs`, so `build_panic_context` is not in the host test binary at
    //! all — a green host run is not weak evidence about it, it is no
    //! evidence (see
    //! `docs/standards/leptos-frontend-patterns/a-green-host-suite-says-nothing-about-wasm32-gated-code.md`).
    //! What *can* be pinned from the host is the shape of the JSON it emits,
    //! because that shape is a literal in the source. Same technique, and the
    //! same reason, as the KYO-436 guards in `utils::oauth_popup`.
    //!
    //! The browser-side half of the criterion — panic on dev, submit from the
    //! overlay, read the stack back out of `context::jsonb` — is covered by
    //! the follow-up `needs-build` ticket, not from here.

    use crate::test_support::extract_between;

    const PANIC_OVERLAY_SRC: &str = include_str!("../panic_overlay.rs");
    const SELF_SRC: &str = include_str!("feedback_context.rs");

    /// The window bounding `build_panic_context`'s body. Both markers are
    /// `fn` signatures, which the regression this file guards cannot delete
    /// without also failing the assertions below.
    ///
    /// The end marker is anchored on the bare name `fn apply_report_success(`
    /// — no `pub`/`async`/other qualifier — because qualifiers are churn, not
    /// identity: KYO-722 broke exactly this way when `f3ac13dd` (KYO-686)
    /// correctly made `submit_panic_report` synchronous for a reason
    /// unrelated to what this guard checks, and the old
    /// `"async fn submit_panic_report("` marker stopped matching, so
    /// `extract_between` panicked with "end marker not found" instead of the
    /// assertions below ever running. `apply_report_success` is used, not
    /// `submit_panic_report`, because it is the function that immediately
    /// follows `build_panic_context` in source order, so the slice is
    /// exactly `build_panic_context`'s body with no ambiguity from
    /// `submit_panic_report`'s own call site earlier in the file.
    fn build_panic_context_body() -> &'static str {
        extract_between(
            PANIC_OVERLAY_SRC,
            "fn build_panic_context(",
            "fn apply_report_success(",
        )
    }

    /// The defect: `console_errors` was a literal `[]` baked into the format
    /// string, so every panic report shipped an empty array on the one report
    /// type that most needs the console.
    #[test]
    fn panic_context_interpolates_the_captured_console_errors() {
        let body = build_panic_context_body();
        assert!(
            !body.contains(r#""console_errors":[]"#),
            "build_panic_context hardcodes an empty console_errors array in its format \
             string, so every panic report discards the console the interceptor already \
             captured — including the panic's own stack (KYO-682)."
        );
        assert!(
            body.contains(r#""console_errors":{}"#),
            "build_panic_context must interpolate console_errors as a format substitution \
             (KYO-682)."
        );
        assert!(
            body.contains("get_console_errors()"),
            "build_panic_context must source console_errors from \
             feedback_context::get_console_errors(), the same getter collect_context uses, \
             rather than capturing them a second way (KYO-682)."
        );
    }

    /// `get_console_errors` returns a pre-serialised JSON array. Escaping it
    /// would double-encode it into a JSON *string* that looks like an array —
    /// the failure mode KYO-682 calls out by name, and one that reads as
    /// fixed while remaining unusable to every consumer of `context::jsonb`.
    #[test]
    fn panic_context_does_not_escape_the_console_errors_array() {
        let body = build_panic_context_body();
        let escaped = body.matches("escape_json_string(").count();
        assert_eq!(
            escaped, 4,
            "exactly four of build_panic_context's fields are plain strings needing \
             escaping (url, browser, os, panic_message); found {escaped} calls. \
             console_errors is already JSON and must be interpolated raw — running it \
             through escape_json_string double-encodes the array into a string (KYO-682)."
        );
    }

    /// The invariant that lets both `get_console_errors` and
    /// `build_panic_context` skip an empty/fallback guard: `consoleErrors` is
    /// initialised by the inline module's own evaluation, above and
    /// independent of `initInterceptor`. Move that initialisation inside the
    /// init function and `getConsoleErrors()` returns `undefined` on any page
    /// that never called `init`, which is not JSON at all.
    #[test]
    fn console_error_buffer_is_initialised_at_module_scope() {
        let preamble = extract_between(
            SELF_SRC,
            "const MAX_ERRORS = 10;",
            "export function initInterceptor()",
        );
        assert!(
            preamble.contains("let consoleErrors = [];"),
            "consoleErrors must be initialised at the inline module's top level, before \
             initInterceptor. Otherwise getConsoleErrors() can return undefined rather \
             than the literal `[]`, and callers that interpolate it raw emit malformed \
             JSON (KYO-682)."
        );
    }
}
