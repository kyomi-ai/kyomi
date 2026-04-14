// SPDX-License-Identifier: AGPL-3.0-or-later

//! Chart info modal — displays datasource, optional SQL query, and full
//! ChartML YAML source, matching React's ChartInfoModal component exactly.

use leptos::prelude::*;
use phosphor_leptos::Icon;
use crate::components::modal::{Modal, ModalSize};

// ─── Helpers ────────────────────────────────────────────────────────────────

/// Extract the datasource name from a raw ChartML YAML string.
///
/// Matches React: `spec.data?.datasource || spec.data?.source || 'Not specified'`
fn extract_datasource(yaml: &str) -> String {
    serde_yaml::from_str::<serde_yaml::Value>(yaml)
        .ok()
        .and_then(|v| {
            let data = v.get("data")?;
            data.get("datasource")
                .or_else(|| data.get("source"))
                .and_then(|s| s.as_str())
                .map(|s| s.to_string())
        })
        .unwrap_or_else(|| "Not specified".to_string())
}

/// Extract the SQL query from a raw ChartML YAML string, if present.
///
/// Matches React: `spec.data?.query || null`
fn extract_query(yaml: &str) -> Option<String> {
    serde_yaml::from_str::<serde_yaml::Value>(yaml)
        .ok()
        .and_then(|v| {
            v.get("data")
                .and_then(|d| d.get("query"))
                .and_then(|q| q.as_str())
                .map(|s| s.trim().to_string())
        })
        .filter(|s| !s.is_empty())
}

// ─── Component ───────────────────────────────────────────────────────────────

/// Modal that displays chart source information.
///
/// Shows datasource, optional SQL query, and full ChartML YAML source —
/// matching React's `ChartInfoModal` component (title "Chart Info", three
/// sections, no footer close button, no line numbers).
#[component]
pub fn ChartInfoModal(
    /// Whether the modal is open.
    #[prop(into)]
    open: Signal<bool>,
    /// The raw ChartML YAML spec to display (reactive signal).
    #[prop(into)]
    yaml: Signal<String>,
    /// Callback to close the modal.
    on_close: Callback<()>,
) -> impl IntoView {
    let datasource = Memo::new(move |_| extract_datasource(&yaml.get()));
    let query = Memo::new(move |_| extract_query(&yaml.get()));

    let (copied, set_copied) = signal(false);

    let on_copy = move |_: leptos::ev::MouseEvent| {
        let text = yaml.get_untracked();
        leptos::task::spawn_local(async move {
            if let Some(window) = web_sys::window() {
                let clipboard = window.navigator().clipboard();
                let promise = clipboard.write_text(&text);
                let _ = wasm_bindgen_futures::JsFuture::from(promise).await;
                set_copied.set(true);
                gloo_timers::future::TimeoutFuture::new(2000).await;
                set_copied.set(false);
            }
        });
    };

    view! {
        <Modal
            show=open
            on_close=on_close
            title="Chart Info"
            size=ModalSize::Lg
        >
            <div class="space-y-6">
                // Datasource section — always shown
                // Matches React: <span>Datasource</span> + <p>{datasource}</p>
                <div>
                    <span class="text-sm font-medium text-foreground">"Datasource"</span>
                    <p class="mt-1 text-sm text-muted-foreground font-mono bg-muted px-3 py-2 rounded-md">
                        {move || datasource.get()}
                    </p>
                </div>

                // SQL Query section — only shown when present
                // Matches React: `{query && <CopyableCodeBlock label="SQL Query" />}`
                {move || query.get().map(|sql| {
                    let sql = StoredValue::new(sql);
                    let (sql_copied, set_sql_copied) = signal(false);
                    let on_sql_copy = move |_: leptos::ev::MouseEvent| {
                        let text = sql.get_value();
                        leptos::task::spawn_local(async move {
                            if let Some(window) = web_sys::window() {
                                let clipboard = window.navigator().clipboard();
                                let promise = clipboard.write_text(&text);
                                let _ = wasm_bindgen_futures::JsFuture::from(promise).await;
                                set_sql_copied.set(true);
                                gloo_timers::future::TimeoutFuture::new(2000).await;
                                set_sql_copied.set(false);
                            }
                        });
                    };
                    let sql_text = sql.get_value();
                    view! {
                        <div class="space-y-2">
                            <span class="text-sm font-medium text-foreground">"SQL Query"</span>
                            <div class="relative group">
                                <button
                                    on:click=on_sql_copy
                                    class="absolute top-2 right-2 p-2 rounded-md bg-secondary hover:bg-accent opacity-0 group-hover:opacity-100 transition-opacity z-10 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
                                    title=move || if sql_copied.get() { "Copied!" } else { "Copy code" }
                                >
                                    {move || if sql_copied.get() {
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
                                    <code class="font-mono bg-transparent">
                                        {sql_text}
                                    </code>
                                </pre>
                            </div>
                        </div>
                    }
                })}

                // ChartML Source section — always shown
                // Matches React: <CopyableCodeBlock label="ChartML Source" language="yaml" />
                <div class="space-y-2">
                    <span class="text-sm font-medium text-foreground">"ChartML Source"</span>
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
                            <code class="font-mono bg-transparent">
                                {move || yaml.get()}
                            </code>
                        </pre>
                    </div>
                </div>
            </div>
        </Modal>
    }
}
