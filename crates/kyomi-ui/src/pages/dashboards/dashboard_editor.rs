// SPDX-License-Identifier: AGPL-3.0-or-later

//! Dashboard editor page — two-panel editor with live preview.
//!
//! Route: `/dashboard/:id/edit`
//!
//! Left panel: Kode `CodeEditor` (Language::Markdown)
//! Right panel: `MarkdownRenderer` with debounced preview (500ms)

use std::sync::Arc;

use leptos::prelude::*;
use leptos_router::hooks::use_params_map;

use crate::components::dashboard::MarkdownRenderer;
use crate::components::Spinner;
use crate::server_fns::dashboards::{get_dashboard, update_dashboard};

/// Dashboard editor page with side-by-side code editor and live preview.
///
/// Fetches the dashboard on mount, populates the Kode editor, and provides
/// a debounced preview via `MarkdownRenderer`. Save button persists changes
/// via `update_dashboard` server function.
#[component]
pub fn DashboardEditorPage() -> impl IntoView {
    let params = use_params_map();
    let dashboard_id = Memo::new(move |_| params.get().get("id").unwrap_or_default());

    // Fetch dashboard detail, keyed on dashboard_id
    let dashboard_resource =
        Resource::new(move || dashboard_id.get(), move |id| get_dashboard(id));

    view! {
        <Suspense fallback=move || {
            view! {
                <div class="flex h-full items-center justify-center bg-muted">
                    <Spinner class="h-8 w-8 text-muted-foreground" />
                </div>
            }
        }>
            {move || {
                dashboard_resource
                    .get()
                    .map(|result| {
                        match result {
                            Err(e) => {
                                view! {
                                    <div class="flex h-full items-center justify-center bg-muted">
                                        <div class="text-center">
                                            <h2 class="text-2xl font-bold text-foreground mb-4">
                                                "Dashboard Not Found"
                                            </h2>
                                            <p class="text-muted-foreground mb-6">{e.to_string()}</p>
                                            <a
                                                href="/dashboards"
                                                class="px-6 py-3 text-white bg-primary hover:bg-primary/90 rounded-lg transition-colors inline-block"
                                            >
                                                "Back to Dashboards"
                                            </a>
                                        </div>
                                    </div>
                                }
                                    .into_any()
                            }
                            Ok(dashboard) => {
                                let did = dashboard.dashboard_id.clone();
                                let initial_title = dashboard.title.clone();
                                let initial_content = dashboard.content.clone();
                                view! {
                                    <DashboardEditorInner
                                        dashboard_id=did
                                        initial_title=initial_title
                                        initial_content=initial_content
                                    />
                                }
                                    .into_any()
                            }
                        }
                    })
            }}
        </Suspense>
    }
}

/// Inner editor component — rendered once the dashboard data is loaded.
///
/// Separated from `DashboardEditorPage` so that signals are initialized
/// with the fetched data rather than empty defaults.
#[component]
fn DashboardEditorInner(
    dashboard_id: String,
    initial_title: String,
    initial_content: String,
) -> impl IntoView {
    // ── Signals ──────────────────────────────────────────────────────────
    let (title, set_title) = signal(initial_title.clone());
    let (editor_content, set_editor_content) = signal(initial_content.clone());
    let (preview_content, set_preview_content) = signal(initial_content.clone());
    let (has_unsaved_changes, set_has_unsaved_changes) = signal(false);
    let (saving, set_saving) = signal(false);
    let (save_error, set_save_error) = signal(Option::<String>::None);
    let (original_content, set_original_content) = signal(initial_content);
    let (original_title, set_original_title) = signal(initial_title);

    // ── Debounced preview update (500ms) ─────────────────────────────────
    // Uses the same SendWrapper pattern as dashboards_list.rs for gloo_timers
    // inside StoredValue (gloo Timeout is not Send+Sync).
    #[cfg(target_arch = "wasm32")]
    let debounce_handle: StoredValue<
        Option<send_wrapper::SendWrapper<gloo_timers::callback::Timeout>>,
    > = StoredValue::new(None);

    // ── Kode on_change callback ──────────────────────────────────────────
    let on_editor_change = Arc::new(move |text: String| {
        set_editor_content.set(text.clone());

        // Check unsaved state against both original content and title
        let content_changed = text != original_content.get_untracked();
        let title_changed = title.get_untracked() != original_title.get_untracked();
        set_has_unsaved_changes.set(content_changed || title_changed);

        // Debounce preview update by 500ms
        #[cfg(target_arch = "wasm32")]
        {
            use send_wrapper::SendWrapper;

            let preview_text = text;
            // Cancel any pending timeout
            debounce_handle.update_value(|h| {
                drop(h.take());
            });

            let handle = gloo_timers::callback::Timeout::new(500, move || {
                set_preview_content.set(preview_text);
            });

            debounce_handle.set_value(Some(SendWrapper::new(handle)));
        }

        #[cfg(not(target_arch = "wasm32"))]
        {
            // On server, update preview immediately (no debounce needed)
            set_preview_content.set(text);
        }
    });

    // Clean up debounce timer on component teardown
    #[cfg(target_arch = "wasm32")]
    on_cleanup(move || {
        debounce_handle.update_value(|h| {
            drop(h.take());
        });
    });

    // ── Save handler ─────────────────────────────────────────────────────
    let dashboard_id_save = dashboard_id.clone();
    let save_handler = move |_| {
        let did = dashboard_id_save.clone();
        let current_title = title.get_untracked();
        let current_content = editor_content.get_untracked();

        set_saving.set(true);
        set_save_error.set(None);

        leptos::task::spawn_local(async move {
            let result = update_dashboard(
                did,
                Some(current_title.clone()),
                Some(current_content.clone()),
                None, // change_summary — could add a prompt later
            )
            .await;

            set_saving.set(false);

            match result {
                Ok(()) => {
                    // Update original values so unsaved indicator clears
                    set_original_content.set(current_content);
                    set_original_title.set(current_title);
                    set_has_unsaved_changes.set(false);
                }
                Err(e) => {
                    set_save_error.set(Some(e.to_string()));
                }
            }
        });
    };

    // ── Title change handler ─────────────────────────────────────────────
    let on_title_input = move |ev: leptos::ev::Event| {
        let value = event_target_value(&ev);
        set_title.set(value.clone());
        let content_changed = editor_content.get_untracked() != original_content.get_untracked();
        let title_changed = value != original_title.get_untracked();
        set_has_unsaved_changes.set(content_changed || title_changed);
    };

    // ── View href for "Back to Dashboard" ────────────────────────────────
    let view_href = format!("/dashboard/{}", dashboard_id);

    view! {
        <div class="flex flex-col h-screen bg-background">
            // ── Toolbar ──────────────────────────────────────────────────
            <div class="flex items-center gap-4 px-6 py-3 border-b border-border bg-card">
                // Title input
                <input
                    type="text"
                    class="flex-1 text-lg font-semibold bg-transparent border-none outline-none text-foreground placeholder:text-muted-foreground"
                    placeholder="Dashboard title..."
                    prop:value=move || title.get()
                    on:input=on_title_input
                />

                // Unsaved changes indicator
                {move || {
                    has_unsaved_changes
                        .get()
                        .then(|| {
                            view! {
                                <span class="text-xs text-amber-500 font-medium whitespace-nowrap">
                                    "Unsaved changes"
                                </span>
                            }
                        })
                }}

                // Save error message
                {move || {
                    save_error
                        .get()
                        .map(|err| {
                            view! {
                                <span class="text-xs text-destructive font-medium truncate max-w-48">
                                    {err}
                                </span>
                            }
                        })
                }}

                // Save button
                <button
                    class="px-4 py-2 text-sm font-medium text-white bg-primary hover:bg-primary/90 rounded-lg transition-colors disabled:opacity-50 disabled:cursor-not-allowed whitespace-nowrap"
                    on:click=save_handler
                    disabled=move || saving.get() || !has_unsaved_changes.get()
                >
                    {move || if saving.get() { "Saving..." } else { "Save" }}
                </button>

                // Back to dashboard link
                <a
                    href=view_href
                    class="px-4 py-2 text-sm font-medium text-foreground bg-card border border-border hover:bg-accent rounded-lg transition-colors whitespace-nowrap"
                >
                    "Back"
                </a>
            </div>

            // ── Two-panel editor ─────────────────────────────────────────
            <div class="flex flex-1 min-h-0 overflow-hidden">
                // Left panel: Kode editor
                <div class="flex-1 min-h-0 overflow-hidden border-r border-border">
                    <DashboardCodeEditor
                        content=editor_content
                        on_change=on_editor_change
                    />
                </div>

                // Right panel: Live preview
                <div class="flex-1 min-h-0 overflow-y-auto p-6">
                    <MarkdownRenderer content=Signal::derive(move || preview_content.get()) />
                </div>
            </div>
        </div>
    }
}

/// Wrapper component for the Kode CodeEditor that handles conditional compilation.
///
/// On WASM targets, renders the Kode `CodeEditor` component with Markdown highlighting.
/// On server (SSR), renders a placeholder since Kode requires browser DOM APIs.
#[component]
fn DashboardCodeEditor(
    content: ReadSignal<String>,
    on_change: Arc<dyn Fn(String) + Send + Sync>,
) -> impl IntoView {
    #[cfg(target_arch = "wasm32")]
    {
        use kode_leptos::{CodeEditor, Language};

        view! {
            <CodeEditor
                language=Signal::stored(Language::Markdown)
                content=Signal::derive(move || content.get())
                on_change=on_change
            />
        }
        .into_any()
    }

    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = content;
        let _ = on_change;

        view! {
            <div class="flex-1 bg-muted p-4 text-muted-foreground">
                "Loading editor..."
            </div>
        }
        .into_any()
    }
}
