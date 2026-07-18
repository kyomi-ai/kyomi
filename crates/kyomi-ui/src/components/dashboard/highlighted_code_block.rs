// SPDX-License-Identifier: AGPL-3.0-or-later

//! Reusable syntax-highlighted code block component.
//!
//! Renders a labeled code block with syntax highlighting via `arborium` and
//! theme-aware colors via `arborium-theme`. Includes a copy-to-clipboard
//! button that appears on hover.
//!
//! ## Hydration strategy
//!
//! SSR and the initial WASM render both produce plain escaped text (matching
//! HTML). A post-hydration `Effect` upgrades the `<code>` element to
//! highlighted HTML via direct DOM manipulation, avoiding hydration mismatches.

use leptos::prelude::*;
use phosphor_leptos::Icon;

// ── WASM-only: arborium highlighter ─────────────────────────────────────

#[cfg(target_arch = "wasm32")]
use std::cell::RefCell;

#[cfg(target_arch = "wasm32")]
thread_local! {
    static HIGHLIGHTER: RefCell<arborium::Highlighter> = RefCell::new(arborium::Highlighter::new());
}

/// Highlight `code` in `language` using arborium tree-sitter grammars.
///
/// Returns HTML with `<a-k>`, `<a-s>`, `<a-c>`, `<a-n>`, `<a-f>` custom
/// elements. Falls back to `html_escape` on error.
#[cfg(target_arch = "wasm32")]
fn highlight_html(code: &str, language: &str) -> String {
    if language.is_empty() {
        return html_escape(code);
    }
    HIGHLIGHTER.with(|h| {
        let mut highlighter = h.borrow_mut();
        match highlighter.highlight(language, code) {
            Ok(result) => result.to_string(),
            Err(_) => html_escape(code),
        }
    })
}

/// Minimal HTML escape (shared between SSR and WASM fallback).
fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

// ── Shared syntax CSS management ────────────────────────────────────────

/// Class applied to all highlighted code blocks for CSS scoping.
#[cfg(target_arch = "wasm32")]
const HCB_CLASS: &str = "hcb-highlighted";

/// ID of the shared `<style>` element in `<head>`.
#[cfg(target_arch = "wasm32")]
const HCB_STYLE_ID: &str = "hcb-syntax-style";

/// Inject or update the shared syntax-highlighting CSS in `<head>`.
///
/// Uses a single `<style>` element with `data-theme` attribute to track the
/// current theme. Only regenerates CSS when the theme changes.
#[cfg(target_arch = "wasm32")]
fn ensure_syntax_css(is_dark: bool) {
    use std::sync::atomic::{AtomicBool, Ordering};

    // Track the last-injected theme to avoid redundant DOM writes
    static LAST_DARK: AtomicBool = AtomicBool::new(false);
    static INITIALIZED: AtomicBool = AtomicBool::new(false);

    let theme_changed = !INITIALIZED.load(Ordering::Relaxed)
        || LAST_DARK.load(Ordering::Relaxed) != is_dark;

    if !theme_changed {
        return;
    }

    let theme = if is_dark {
        arborium_theme::builtin::tokyo_night()
    } else {
        arborium_theme::builtin::github_light()
    };
    let css = theme.to_css(&format!(".{HCB_CLASS}"));

    if let Some(window) = web_sys::window()
        && let Some(doc) = window.document()
        && let Some(head) = doc.head()
    {
        let style_el = match doc.get_element_by_id(HCB_STYLE_ID) {
            Some(el) => el,
            None => {
                let el = doc
                    .create_element("style")
                    .expect("create style element");
                el.set_id(HCB_STYLE_ID);
                head.append_child(&el).expect("append style to head");
                el
            }
        };
        style_el.set_text_content(Some(&css));
    }

    LAST_DARK.store(is_dark, Ordering::Relaxed);
    INITIALIZED.store(true, Ordering::Relaxed);
}

// ── Component ────────────────────────────────────────────────────────────

/// A syntax-highlighted code block with label and copy button.
///
/// - `code`: reactive signal for the source text
/// - `language`: arborium language tag (e.g. `"sql"`, `"yaml"`)
/// - `label`: section title displayed above the block
///
/// On SSR, renders plain escaped text. A post-hydration `Effect` upgrades the
/// code to highlighted HTML and injects scoped syntax CSS.
#[component]
pub fn HighlightedCodeBlock(
    #[prop(into)] code: Signal<String>,
    #[prop(into)] language: Signal<String>,
    #[prop(into)] label: Signal<String>,
) -> impl IntoView {
    let (copied, set_copied) = signal(false);

    let on_copy = move |_: leptos::ev::MouseEvent| {
        let text = code.get_untracked();
        leptos::task::spawn_local(async move {
            if let Some(window) = web_sys::window() {
                let clipboard = window.navigator().clipboard();
                let promise = clipboard.write_text(&text);
                let _ = wasm_bindgen_futures::JsFuture::from(promise).await;
                set_copied.try_set(true);
                gloo_timers::future::TimeoutFuture::new(2000).await;
                set_copied.try_set(false);
            }
        });
    };

    // Ref to the <code> element for post-hydration DOM manipulation
    let code_ref = NodeRef::<leptos::html::Code>::new();

    // Post-hydration effect: upgrade plain text → highlighted HTML.
    // Runs only on WASM after hydration is complete, avoiding mismatches.
    #[cfg(target_arch = "wasm32")]
    {
        // Theme detection — must be called during component init (not inside Effect)
        let theme_state = crate::components::theme::use_theme();

        Effect::new(move || {
            let text = code.get();
            let lang = language.get();

            // Highlight the code
            let highlighted = highlight_html(&text, &lang);

            // Set innerHTML via DOM API (bypasses Leptos hydration tracking)
            if let Some(el) = code_ref.get() {
                let html_element: &web_sys::HtmlElement = el.as_ref();
                html_element.set_inner_html(&highlighted);
            }

            // Inject / update the shared syntax CSS
            let is_dark = theme_state
                .map(|s| s.effective.get() == "dark")
                .unwrap_or(true);
            ensure_syntax_css(is_dark);
        });
    }

    // Suppress unused warning on non-wasm (language is used inside the wasm Effect)
    let _ = &language;

    view! {
        <div class="space-y-2">
            <span class="text-sm font-medium text-foreground">{move || label.get()}</span>
            <div class="relative group">
                <button
                    on:click=on_copy
                    class="absolute top-2 right-2 p-2 rounded-md bg-secondary hover:bg-accent opacity-0 group-hover:opacity-100 transition-opacity z-10 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
                    title=move || if copied.get() { "Copied!" } else { "Copy code" }
                >
                    {move || if copied.get() {
                        view! {
                            <span class="block h-4 w-4 text-success-foreground">
                                <Icon icon=phosphor_leptos::CHECK size="16px" />
                            </span>
                        }.into_any()
                    } else {
                        view! {
                            <span class="block h-4 w-4 text-muted-foreground">
                                <Icon icon=phosphor_leptos::COPY size="16px" />
                            </span>
                        }.into_any()
                    }}
                </button>
                <pre class="bg-muted rounded-md p-4 overflow-x-auto text-sm font-mono max-h-[300px] overflow-y-auto">
                    <code
                        node_ref=code_ref
                        class="font-mono bg-transparent hcb-highlighted"
                        inner_html=move || html_escape(&code.get())
                    ></code>
                </pre>
            </div>
        </div>
    }
}
