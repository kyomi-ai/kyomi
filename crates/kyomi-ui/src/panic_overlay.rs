// SPDX-License-Identifier: AGPL-3.0-or-later

//! WASM panic recovery overlay.
//!
//! After a WASM panic the Leptos reactive runtime is dead — no signals, no
//! components, no reactive state. This module builds a plain DOM recovery UI
//! via raw `web_sys` calls and injects it directly into the page body.
//!
//! The overlay uses CSS custom properties (`var(--color-*)`) defined by the
//! loaded stylesheet so that it automatically respects light/dark mode without
//! any Leptos involvement. Tailwind classes are intentionally avoided because
//! the Tailwind JIT scanner runs at build time; ad-hoc classes injected after a
//! panic would have no corresponding CSS rules.

use crate::utils::feedback_context::{escape_json_string, extract_browser, extract_os};
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;

/// Inject a full-screen panic recovery overlay into the document body.
///
/// This function is called from the custom panic hook set up in `main.rs`.
/// It must be infallible — any error here is silently swallowed so that the
/// original panic message is not masked.
///
/// Design tokens used (all via CSS custom properties so light/dark mode works):
/// - `var(--color-background)` — page background
/// - `var(--color-card)` — card surface
/// - `var(--color-foreground)` — primary text
/// - `var(--color-muted-foreground)` — secondary / description text
/// - `var(--color-border)` — card border
/// - `var(--color-primary)` — primary button background
/// - `var(--color-primary-foreground)` — primary button text
/// - `var(--color-secondary)` — secondary button background
/// - `var(--color-secondary-foreground)` — secondary button text
/// - `var(--color-overlay)` — full-screen backdrop
pub fn show_panic_recovery_overlay(panic_message: &str) {
    if let Err(_) = try_show_panic_recovery_overlay(panic_message) {
        // Best-effort: if we cannot even build the DOM overlay there is nothing
        // safe to do — the browser's built-in "page stopped responding" UI will
        // have to serve as the error surface.
    }
}

/// Internal fallible version — all DOM operations can theoretically fail.
fn try_show_panic_recovery_overlay(panic_message: &str) -> Result<(), JsValue> {
    let window = web_sys::window().ok_or("no window")?;
    let document = window.document().ok_or("no document")?;
    let body = document.body().ok_or("no body")?;

    if document.get_element_by_id("kyomi-panic-overlay").is_some() {
        return Ok(());
    }

    // ------------------------------------------------------------------
    // 1. Inject a <style> block with the fade-in keyframe and button hover
    //    transitions. Tailwind won't have generated these at runtime so we
    //    emit them directly into <head>.
    // ------------------------------------------------------------------
    if let Some(head) = document.head() {
        let style_el = document.create_element("style")?;
        style_el.set_text_content(Some(
            r"
@keyframes kyomi-overlay-fade-in {
  from { opacity: 0; transform: translateY(-8px); }
  to   { opacity: 1; transform: translateY(0); }
}
#kyomi-panic-overlay-card {
  animation: kyomi-overlay-fade-in 250ms cubic-bezier(0.16, 1, 0.3, 1) both;
}
#kyomi-panic-reload-btn:hover {
  opacity: 0.9;
}
#kyomi-panic-report-btn:hover {
  border-color: var(--color-border);
  background-color: var(--color-muted, #f5f3ef);
}
#kyomi-panic-overlay details > summary {
  cursor: pointer;
  user-select: none;
}
@media (prefers-reduced-motion: reduce) {
  #kyomi-panic-overlay-card {
    animation: none;
  }
}
",
        ));
        let _ = head.append_child(&style_el);
    }

    // ------------------------------------------------------------------
    // 2. Full-screen backdrop
    // ------------------------------------------------------------------
    let backdrop = document.create_element("div")?;
    backdrop.set_attribute("id", "kyomi-panic-overlay")?;
    backdrop.set_attribute(
        "style",
        "position:fixed;inset:0;z-index:9999;\
         background-color:var(--color-overlay,rgba(0,0,0,0.5));\
         display:flex;align-items:center;justify-content:center;\
         font-family:'DM Sans',system-ui,sans-serif;",
    )?;

    // ------------------------------------------------------------------
    // 3. Card
    // ------------------------------------------------------------------
    let card = document.create_element("div")?;
    card.set_attribute("id", "kyomi-panic-overlay-card")?;
    card.set_attribute(
        "style",
        "background-color:var(--color-card,#ffffff);\
         color:var(--color-foreground,#1c1917);\
         border:1px solid var(--color-border,#e8e5de);\
         border-radius:12px;\
         box-shadow:0 25px 50px -12px rgba(0,0,0,0.25);\
         padding:32px;\
         max-width:480px;\
         width:calc(100vw - 48px);\
         display:flex;\
         flex-direction:column;\
         align-items:center;\
         gap:16px;\
         text-align:center;",
    )?;

    // ------------------------------------------------------------------
    // 4. Animated Kyomi logo
    // ------------------------------------------------------------------
    let logo = document.create_element("img")?;
    logo.set_attribute("src", "/kyomi_animated_logo.svg")?;
    logo.set_attribute("alt", "Kyomi")?;
    logo.set_attribute(
        "style",
        "width:48px;height:48px;flex-shrink:0;",
    )?;
    card.append_child(&logo)?;

    // ------------------------------------------------------------------
    // 5. Heading
    // ------------------------------------------------------------------
    let heading = document.create_element("h2")?;
    heading.set_attribute(
        "style",
        "margin:0;\
         font-size:20px;\
         font-weight:600;\
         color:var(--color-foreground,#1c1917);",
    )?;
    heading.set_text_content(Some("Something went wrong"));
    card.append_child(&heading)?;

    // ------------------------------------------------------------------
    // 6. Explanation text
    // ------------------------------------------------------------------
    let explanation = document.create_element("p")?;
    explanation.set_attribute(
        "style",
        "margin:0;\
         font-size:14px;\
         line-height:1.6;\
         color:var(--color-muted-foreground,#6b6660);\
         max-width:380px;",
    )?;
    explanation.set_text_content(Some(
        "The application encountered an unexpected error and could not continue. \
         You can reload the page to start fresh, or send a bug report so the team \
         can investigate.",
    ));
    card.append_child(&explanation)?;

    // ------------------------------------------------------------------
    // 7. Technical Details <details> block
    // ------------------------------------------------------------------
    let details = document.create_element("details")?;
    details.set_attribute(
        "style",
        "width:100%;\
         text-align:left;\
         border:1px solid var(--color-border,#e8e5de);\
         border-radius:8px;\
         overflow:hidden;",
    )?;

    let summary = document.create_element("summary")?;
    summary.set_attribute(
        "style",
        "padding:8px 12px;\
         font-size:13px;\
         font-weight:500;\
         color:var(--color-muted-foreground,#6b6660);\
         background-color:var(--color-secondary,#f5f3ef);\
         list-style:none;\
         display:flex;\
         align-items:center;\
         gap:6px;",
    )?;
    summary.set_text_content(Some("Technical Details"));
    details.append_child(&summary)?;

    let pre = document.create_element("pre")?;
    pre.set_attribute(
        "style",
        "margin:0;\
         padding:12px;\
         font-size:11px;\
         font-family:'Geist Mono',ui-monospace,monospace;\
         white-space:pre-wrap;\
         word-break:break-all;\
         overflow-x:auto;\
         color:var(--color-foreground,#1c1917);\
         background-color:var(--color-muted,#f5f3ef);\
         max-height:160px;\
         overflow-y:auto;",
    )?;
    pre.set_text_content(Some(panic_message));
    details.append_child(&pre)?;

    card.append_child(&details)?;

    // ------------------------------------------------------------------
    // 8. Button row
    // ------------------------------------------------------------------
    let button_row = document.create_element("div")?;
    button_row.set_attribute(
        "style",
        "display:flex;\
         gap:8px;\
         flex-wrap:wrap;\
         justify-content:center;\
         width:100%;",
    )?;

    // Reload button (primary)
    let reload_btn = document.create_element("button")?;
    reload_btn.set_attribute("id", "kyomi-panic-reload-btn")?;
    reload_btn.set_attribute("type", "button")?;
    reload_btn.set_attribute(
        "style",
        "padding:10px 20px;\
         font-size:14px;\
         font-weight:600;\
         font-family:'DM Sans',system-ui,sans-serif;\
         border:none;\
         border-radius:8px;\
         cursor:pointer;\
         transition:opacity 150ms ease;\
         background-color:var(--color-primary,#d97706);\
         color:var(--color-primary-foreground,#ffffff);\
         flex:1;\
         min-width:120px;\
         max-width:200px;",
    )?;
    reload_btn.set_text_content(Some("Reload Page"));

    // Reload click handler
    let window_clone = window.clone();
    let reload_closure = Closure::<dyn Fn()>::new(move || {
        let _ = window_clone.location().reload();
    });
    reload_btn
        .dyn_ref::<web_sys::EventTarget>()
        .ok_or("reload_btn is not EventTarget")?
        .add_event_listener_with_callback("click", reload_closure.as_ref().unchecked_ref())?;
    reload_closure.forget();

    button_row.append_child(&reload_btn)?;

    // Bug report button (secondary)
    let report_btn = document.create_element("button")?;
    report_btn.set_attribute("id", "kyomi-panic-report-btn")?;
    report_btn.set_attribute("type", "button")?;
    report_btn.set_attribute(
        "style",
        "padding:10px 20px;\
         font-size:14px;\
         font-weight:600;\
         font-family:'DM Sans',system-ui,sans-serif;\
         border:1px solid var(--color-border,#e8e5de);\
         border-radius:8px;\
         cursor:pointer;\
         transition:opacity 150ms ease, background-color 150ms ease, border-color 150ms ease;\
         background-color:var(--color-secondary,#f5f3ef);\
         color:var(--color-secondary-foreground,#1c1917);\
         flex:1;\
         min-width:120px;\
         max-width:200px;",
    )?;
    report_btn.set_text_content(Some("Send Bug Report"));

    // Bug report click handler — async fetch to the feedback server function
    let panic_msg_owned = panic_message.to_string();
    let report_btn_clone = report_btn.clone();
    let window_for_report = window.clone();

    let report_closure = Closure::<dyn Fn()>::new(move || {
        let panic_msg = panic_msg_owned.clone();
        let btn = report_btn_clone.clone();
        let win = window_for_report.clone();

        wasm_bindgen_futures::spawn_local(async move {
            // Update button to "Sending..." state
            btn.set_text_content(Some("Sending\u{2026}"));
            btn.dyn_ref::<web_sys::HtmlElement>()
                .map(|el| el.style().set_property("opacity", "0.7").ok());

            match submit_panic_report(&win, &panic_msg).await {
                Ok(()) => {
                    btn.set_text_content(Some("Report Sent \u{2014} Thank you!"));
                    btn.dyn_ref::<web_sys::HtmlElement>()
                        .map(|el| el.style().set_property("opacity", "1").ok());
                }
                Err(_) => {
                    btn.set_text_content(Some("Failed to send \u{2014} try reloading"));
                    btn.dyn_ref::<web_sys::HtmlElement>()
                        .map(|el| el.style().set_property("opacity", "1").ok());
                }
            }
        });
    });

    report_btn
        .dyn_ref::<web_sys::EventTarget>()
        .ok_or("report_btn is not EventTarget")?
        .add_event_listener_with_callback("click", report_closure.as_ref().unchecked_ref())?;
    report_closure.forget();

    button_row.append_child(&report_btn)?;

    card.append_child(&button_row)?;
    backdrop.append_child(&card)?;
    body.append_child(&backdrop)?;

    Ok(())
}

/// Build the context JSON for the panic report, matching the shape produced by
/// `utils::feedback_context::collect_context()`.
fn build_panic_context(window: &web_sys::Window, panic_message: &str) -> String {
    let url = window
        .location()
        .pathname()
        .unwrap_or_else(|_| String::from("/"));

    let ua = window
        .navigator()
        .user_agent()
        .unwrap_or_default();

    let browser = extract_browser(&ua);
    let os = extract_os(&ua);

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

    format!(
        r#"{{"url":"{}","browser":"{}","os":"{}","screen_width":{},"screen_height":{},"panic_message":"{}","console_errors":[]}}"#,
        escape_json_string(&url),
        escape_json_string(&browser),
        escape_json_string(&os),
        screen_width,
        screen_height,
        escape_json_string(panic_message),
    )
}

/// Submit the panic trace to the feedback server function.
///
/// Posts URL-encoded form data to `/leptos-api/submit_feedback`, matching
/// exactly what the Leptos-generated client would send. The existing session
/// cookie is still present in the browser so authentication still works.
async fn submit_panic_report(window: &web_sys::Window, panic_message: &str) -> Result<(), JsValue> {
    let context_json = build_panic_context(window, panic_message);

    // URL-encode all fields. We encode manually to avoid pulling in a URL
    // encoding library — only the characters that `encodeURIComponent` encodes
    // are special here.
    let description = format!("WASM Panic: {}", panic_message);
    let body = format!(
        "feedback_type={}&description={}&include_context={}&context={}&screenshot=",
        url_encode("bug"),
        url_encode(&description),
        url_encode("true"),
        url_encode(&context_json),
    );

    let init = web_sys::RequestInit::new();
    init.set_method("POST");
    init.set_credentials(web_sys::RequestCredentials::Include);

    let headers = web_sys::Headers::new()?;
    headers.set("Content-Type", "application/x-www-form-urlencoded")?;
    init.set_headers(&headers);
    init.set_body(&JsValue::from_str(&body));

    let request = web_sys::Request::new_with_str_and_init(
        "/leptos-api/submit_feedback",
        &init,
    )?;

    let response_promise = window.fetch_with_request(&request);
    let response = wasm_bindgen_futures::JsFuture::from(response_promise).await?;
    let response: web_sys::Response = response.dyn_into()?;

    if response.ok() {
        Ok(())
    } else {
        Err(JsValue::from_str(&format!(
            "HTTP {}",
            response.status()
        )))
    }
}

/// Minimal percent-encoder for URL form values.
///
/// Encodes characters that are not safe in `application/x-www-form-urlencoded`
/// values, matching the behaviour of JavaScript's `encodeURIComponent`.
fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for byte in s.bytes() {
        match byte {
            // Unreserved characters per RFC 3986 — safe as-is
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9'
            | b'-' | b'_' | b'.' | b'!' | b'~' | b'*' | b'\'' | b'(' | b')' => {
                out.push(byte as char);
            }
            // Everything else is percent-encoded
            b => {
                out.push('%');
                out.push(HEX_CHARS[(b >> 4) as usize] as char);
                out.push(HEX_CHARS[(b & 0xf) as usize] as char);
            }
        }
    }
    out
}

const HEX_CHARS: &[u8] = b"0123456789ABCDEF";

