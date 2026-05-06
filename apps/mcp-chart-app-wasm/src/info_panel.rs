// SPDX-License-Identifier: AGPL-3.0-or-later

//! Info panel showing datasource, SQL query, and ChartML source YAML.

use leptos::prelude::*;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::spawn_local;

use crate::app::AppState;

/// Copy text to clipboard, return true on success.
async fn copy_to_clipboard(text: &str) -> bool {
    let window = web_sys::window().unwrap();
    let navigator = window.navigator();
    let clipboard = navigator.clipboard();
    let promise = clipboard.write_text(text);
    wasm_bindgen_futures::JsFuture::from(promise).await.is_ok()
}

/// Set a timeout using web_sys (avoids gloo-timers dependency).
fn set_timeout(ms: i32, f: impl FnOnce() + 'static) {
    let cb = Closure::once_into_js(f);
    let _ = web_sys::window()
        .unwrap()
        .set_timeout_with_callback_and_timeout_and_arguments_0(
            cb.unchecked_ref(),
            ms,
        );
}

/// A code section with label and copy button.
#[component]
fn CodeSection(
    label: &'static str,
    code: String,
) -> impl IntoView {
    let (copy_label, set_copy_label) = signal("Copy".to_string());
    let (copied_class, set_copied_class) = signal(String::new());
    let code_for_copy = code.clone();

    let on_copy = move |_| {
        let text = code_for_copy.clone();
        spawn_local(async move {
            if copy_to_clipboard(&text).await {
                set_copy_label.set("Copied!".to_string());
                set_copied_class.set(" copied".to_string());
                set_timeout(2000, move || {
                    set_copy_label.set("Copy".to_string());
                    set_copied_class.set(String::new());
                });
            } else {
                set_copy_label.set("Failed".to_string());
                set_timeout(2000, move || {
                    set_copy_label.set("Copy".to_string());
                });
            }
        });
    };

    view! {
        <div class="info-section">
            <div class="info-label-row">
                <span class="info-label">{label}</span>
                <button
                    class=move || format!("info-copy-btn{}", copied_class.get())
                    on:click=on_copy
                >
                    {move || copy_label.get()}
                </button>
            </div>
            <pre class="info-code">{code}</pre>
        </div>
    }
}

#[component]
pub fn InfoPanel() -> impl IntoView {
    let state = expect_context::<AppState>();

    let source_spec = move || state.source_spec.get();

    let datasource = move || {
        source_spec()
            .as_ref()
            .and_then(|s| s.get("data"))
            .and_then(|d| {
                d.get("datasource")
                    .or_else(|| d.get("source"))
                    .and_then(|v| v.as_str())
                    .map(String::from)
            })
            .unwrap_or_else(|| "Not specified".to_string())
    };

    let query = move || {
        source_spec()
            .as_ref()
            .and_then(|s| s.get("data"))
            .and_then(|d| d.get("query"))
            .and_then(|q| q.as_str())
            .map(|s| s.trim().to_string())
    };

    let chartml_yaml = move || {
        source_spec().map(|s| {
            serde_yaml::to_string(&s).unwrap_or_else(|_| "# Error serializing spec".to_string())
        })
    };

    view! {
        <div class="chart-info-panel">
            // Datasource
            <div class="info-section">
                <span class="info-label">"Datasource"</span>
                <span class="info-value">{datasource}</span>
            </div>

            // SQL Query (only if present)
            {move || query().map(|q| view! {
                <CodeSection label="SQL Query" code=q />
            })}

            // ChartML Source
            {move || chartml_yaml().map(|yaml| view! {
                <CodeSection label="ChartML Source" code=yaml />
            })}
        </div>
    }
}
