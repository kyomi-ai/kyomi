// SPDX-License-Identifier: AGPL-3.0-or-later

//! Chart info modal — displays the raw ChartML YAML spec in a styled code block
//! with copy-to-clipboard functionality.

use leptos::prelude::*;

use crate::components::modal::{Modal, ModalSize};

/// Modal that displays the raw ChartML YAML specification for a chart.
///
/// Shows the YAML in a styled monospace code block with line numbers and a
/// copy-to-clipboard button. Reuses the project's `Modal` component for
/// consistent overlay behavior.
#[component]
pub fn ChartInfoModal(
    /// Whether the modal is open.
    #[prop(into)]
    open: Signal<bool>,
    /// The chart YAML spec to display.
    yaml: String,
    /// Callback to close the modal.
    on_close: Callback<()>,
) -> impl IntoView {
    let yaml_stored = StoredValue::new(yaml.clone());
    let yaml_for_display = yaml;

    let (copied, set_copied) = signal(false);

    let on_copy = move |_: leptos::ev::MouseEvent| {
        let text = yaml_stored.get_value();
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

    // Pre-compute numbered lines for the display.
    let numbered_lines: Vec<(usize, String)> = yaml_for_display
        .lines()
        .enumerate()
        .map(|(i, line)| (i + 1, line.to_string()))
        .collect();
    let numbered_lines = StoredValue::new(numbered_lines);

    let on_close_footer = on_close;

    view! {
        <Modal
            show=open
            on_close=on_close
            title="Chart Specification"
            size=ModalSize::Lg
            footer=std::sync::Arc::new(move || view! {
                <button
                    class="inline-flex items-center justify-center rounded-md text-sm font-medium transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring bg-secondary text-secondary-foreground hover:bg-secondary/80 h-9 px-4 py-2"
                    on:click=move |_| on_close_footer.run(())
                >
                    "Close"
                </button>
            }.into_any())
        >
            <div class="relative group">
                // Copy button — top-right of code block
                <button
                    on:click=on_copy
                    class="absolute top-2 right-2 p-1.5 rounded bg-accent hover:bg-accent/80 opacity-0 group-hover:opacity-100 transition-opacity z-10"
                    title=move || if copied.get() { "Copied!" } else { "Copy code" }
                >
                    {move || {
                        if copied.get() {
                            view! {
                                <svg class="h-4 w-4 text-green-500" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                                    <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M5 13l4 4L19 7" />
                                </svg>
                            }.into_any()
                        } else {
                            view! {
                                <svg class="h-4 w-4 text-muted-foreground" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                                    <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M8 5H6a2 2 0 00-2 2v12a2 2 0 002 2h10a2 2 0 002-2v-1M8 5a2 2 0 002 2h2a2 2 0 002-2M8 5a2 2 0 012-2h2a2 2 0 012 2m0 0h2a2 2 0 012 2v3m2 4H10m0 0l3-3m-3 3l3 3" />
                                </svg>
                            }.into_any()
                        }
                    }}
                </button>
                // Code block with line numbers
                <pre class="bg-muted rounded-lg p-4 overflow-x-auto text-sm font-mono">
                    <code class="text-foreground">
                        {numbered_lines.get_value().into_iter().map(|(num, line)| {
                            view! {
                                <div class="flex">
                                    <span class="select-none text-muted-foreground/50 text-right w-8 pr-3 shrink-0">
                                        {num}
                                    </span>
                                    <span class="whitespace-pre">{line}</span>
                                </div>
                            }
                        }).collect_view()}
                    </code>
                </pre>
            </div>
        </Modal>
    }
}
