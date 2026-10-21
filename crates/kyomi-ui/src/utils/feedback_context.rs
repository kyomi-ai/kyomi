// SPDX-License-Identifier: AGPL-3.0-or-later

//! Feedback context collector — passively captures console errors, failed
//! requests, and browser/OS information for feedback submissions.
//!
//! Port of `apps/frontend/src/lib/feedbackContext.js`. Browser collection is
//! WASM-only; the shared JSON builder also compiles in host tests.

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
    collect_context_with_panic(&window, None)
}

/// Collect browser context for either feedback or a panic report.
#[cfg(target_arch = "wasm32")]
pub(crate) fn collect_context_with_panic(
    window: &web_sys::Window,
    panic_message: Option<&str>,
) -> String {

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

    build_context_json(
        ContextFields {
            url: &url,
            browser: &browser,
            os: &os,
            screen_width,
            screen_height,
            console_errors: &console_errors,
        },
        panic_message,
    )
}

/// Browser values shared by ordinary feedback and panic reports. The console
/// interceptor supplies `console_errors` as a pre-serialised JSON array.
#[cfg(any(test, target_arch = "wasm32"))]
struct ContextFields<'a> {
    url: &'a str,
    browser: &'a str,
    os: &'a str,
    screen_width: u32,
    screen_height: u32,
    console_errors: &'a str,
}

/// One JSON shape for feedback and panic reports. Only plain strings are
/// escaped; hand-built JSON avoids adding a serialiser to the WASM bundle.
/// Host tests exercise this builder without a non-WASM browser collector.
#[cfg(any(test, target_arch = "wasm32"))]
fn build_context_json(fields: ContextFields<'_>, panic_message: Option<&str>) -> String {
    let panic_field = panic_message
        .map(|message| format!(r#","panic_message":"{}""#, escape_json_string(message)))
        .unwrap_or_default();
    format!(
        r#"{{"url":"{}","browser":"{}","os":"{}","screen_width":{},"screen_height":{}{},"console_errors":{}}}"#,
        escape_json_string(fields.url),
        escape_json_string(fields.browser),
        escape_json_string(fields.os),
        fields.screen_width,
        fields.screen_height,
        panic_field,
        fields.console_errors,
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
#[cfg(any(test, target_arch = "wasm32"))]
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
    //! Host tests exercise the shared JSON builder and use source guards to
    //! pin both WASM-only entry points to that builder (KYO-695). They do not
    //! execute browser collection or overlay submission.
    //!
    //! The browser-side half of the criterion — panic on dev, submit from the
    //! overlay, read the stack back out of `context::jsonb` — is covered by
    //! the follow-up `needs-build` ticket, not from here.

    use crate::test_support::extract_between;

    const PANIC_OVERLAY_SRC: &str = include_str!("../panic_overlay.rs");
    const SELF_SRC: &str = include_str!("feedback_context.rs");
    const MAIN_SRC: &str = include_str!("../main.rs");
    const LAYOUT_SRC: &str = include_str!("../components/layout.rs");

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

    /// Keep KYO-682's wiring guard meaningful after moving serialization to the
    /// shared collector: neither browser entry point may substitute an empty array.
    #[test]
    fn panic_context_interpolates_the_captured_console_errors() {
        let body = build_panic_context_body();
        assert!(
            body.contains("collect_context_with_panic(window, Some(panic_message))"),
            "panic reports must use the shared browser collector (KYO-695)"
        );
        assert!(
            !body.contains("format!("),
            "panic reports must not duplicate the JSON field list"
        );
        let feedback = extract_between(
            SELF_SRC,
            "pub fn collect_context()",
            "pub(crate) fn collect_context_with_panic(",
        );
        assert!(
            feedback.contains("collect_context_with_panic(&window, None)"),
            "feedback must use the shared browser collector (KYO-695)"
        );
        let collector = extract_between(
            SELF_SRC,
            "pub(crate) fn collect_context_with_panic(",
            "fn build_context_json(",
        );
        assert!(
            collector.contains("let console_errors = get_console_errors();"),
            "reports must source the interceptor's captured errors (KYO-682)"
        );
        assert!(
            collector.contains("console_errors: &console_errors"),
            "reports must pass real console errors to the builder (KYO-682)"
        );
        assert!(
            collector.contains("build_context_json("),
            "both report types must call the shared builder"
        );
        assert!(
            !collector.contains("format!("),
            "the collector must not duplicate the JSON field list"
        );
    }

    /// Parse the production builder's output, including quotes, backslashes,
    /// control characters and Unicode. Browser acquisition remains WASM-only.
    fn assert_context_json(panic_message: Option<&str>) {
        let errors = r#"[{"level":"error","message":"panic \"stack\"\nline","timestamp":"now"}]"#;
        let expected: serde_json::Value = serde_json::from_str(errors).unwrap();
        let fields = super::ContextFields {
            url: "/\"\\\n\u{0000}🦀",
            browser: "browser\t",
            os: "os\r",
            screen_width: 1280,
            screen_height: 720,
            console_errors: errors,
        };
        let text = super::build_context_json(fields, panic_message);
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(value["console_errors"], expected);
        assert!(value["console_errors"].is_array());
        assert_eq!(value["url"], "/\"\\\n\u{0000}🦀");
        assert_eq!(value["browser"], "browser\t");
        assert_eq!(value["os"], "os\r");
        assert_eq!(value["screen_width"], 1280);
        assert_eq!(value["screen_height"], 720);
        match panic_message {
            Some(message) => assert_eq!(value["panic_message"], message),
            None => assert!(value.get("panic_message").is_none()),
        }
        assert_eq!(
            value.as_object().unwrap().len(),
            if panic_message.is_some() { 7 } else { 6 }
        );
    }

    #[test]
    fn panic_context_does_not_escape_the_console_errors_array() {
        assert_context_json(Some("panic \"message\"\n\t\r\\\u{0001} 🦀"));
    }

    #[test]
    fn feedback_context_does_not_escape_the_console_errors_array() {
        assert_context_json(None);
    }

    #[test]
    fn feedback_context_preserves_an_empty_console_array() {
        for panic_message in [None, Some("")] {
            let fields = super::ContextFields {
                url: "",
                browser: "",
                os: "",
                screen_width: 0,
                screen_height: 0,
                console_errors: "[]",
            };
            let value: serde_json::Value =
                serde_json::from_str(&super::build_context_json(fields, panic_message)).unwrap();
            assert_eq!(value["console_errors"], serde_json::json!([]));
            match panic_message {
                Some(message) => assert_eq!(value["panic_message"], message),
                None => assert!(value.get("panic_message").is_none()),
            }
        }
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

    // Source-level guards for KYO-698.
    //
    // `init()` used to be called from `Layout`'s component body, under a
    // comment claiming it ran "once at WASM startup before any errors could
    // occur." A component body executes at render time, not at startup, so
    // any panic before `Layout` renders — notably a hydration panic, the
    // exact class the panic recovery overlay exists for — submitted a
    // feedback report with an empty `console_errors` array. `init()` now
    // installs from `main.rs`, before the panic hook, and `Layout` no
    // longer calls it at all. These guards are host-side source-text
    // checks, for the same reason as the ones above: the call is
    // `#[cfg(target_arch = "wasm32")]`-gated, so a host test can pin only
    // the shape of the source, not exercise the call.

    /// The window from `fn main()`'s opening brace up to (not including)
    /// where the panic hook is installed. Both markers are structural — the
    /// function signature and the `std::panic::set_hook` call itself — so a
    /// regression that reorders or drops the interceptor install cannot
    /// also remove either marker without failing to compile.
    fn main_before_panic_hook() -> &'static str {
        extract_between(MAIN_SRC, "fn main() {", "std::panic::set_hook(")
    }

    #[test]
    fn feedback_context_init_runs_before_the_panic_hook_is_installed() {
        let before_hook = main_before_panic_hook();
        assert!(
            before_hook.contains("feedback_context::init()"),
            "feedback_context::init() must run before std::panic::set_hook in main.rs, \
             so a panic's own console.error output is captured by the interceptor. \
             Otherwise a panic report's console_errors array misses the panic's own \
             stack trace (KYO-698)."
        );
    }

    #[test]
    fn feedback_context_init_has_exactly_one_call_site_in_main() {
        let calls = MAIN_SRC.matches("feedback_context::init()").count();
        assert_eq!(
            calls, 1,
            "main.rs must install the feedback console-error interceptor exactly once; \
             found {calls} call sites (KYO-698)."
        );
    }

    #[test]
    fn layout_no_longer_installs_the_feedback_interceptor() {
        assert!(
            !LAYOUT_SRC.contains("feedback_context::init()"),
            "feedback_context::init() must not be called from Layout's component body. \
             A component body executes at render time, not at WASM startup, so a panic \
             before Layout renders — notably a hydration panic — would submit a feedback \
             report with an empty console_errors array. main.rs is the single install \
             site (KYO-698)."
        );
    }
}
