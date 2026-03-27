// SPDX-License-Identifier: AGPL-3.0-or-later

//! Dashboard editor page — two-panel editor with live preview.
//!
//! Route: `/dashboard/:id/edit` (existing) or `/dashboard/new/edit` (create)
//!
//! Layout:
//! ```text
//! ┌──────────────────────────────────────────────────────────────────────────────────┐
//! │ ← Dashboard │ Title │ Source/Visual │ Chart │ Link │ Unsaved │ Copilot │ History │ Save │
//! ├──────────────────────────────┬───────────────────────────────────────────────────┤
//! │                              │                                  │
//! │   Kode Editor                │   Live Preview                   │
//! │   (Language::Markdown)       │   (MarkdownRenderer)             │
//! │                              │                                  │
//! └──────────────────────────────┴──────────────────────────────────┘
//! ```
//!
//! - Source mode: left = `CodeEditor`, right = debounced `MarkdownRenderer`
//! - Visual mode: full-width `WysiwygEditor` (rich-text WYSIWYG markdown editing)
//! - History panel via `HistoryPanel` component
//! - Cmd/Ctrl+S keyboard shortcut for save
//! - `beforeunload` warning when unsaved changes exist

use std::sync::Arc;

use leptos::prelude::*;
use leptos_router::hooks::use_params_map;

use crate::components::dashboard::{
    ChartBuilderModal, CopilotSidebar, HistoryPanel, InsertDashboardLinkModal, MarkdownRenderer,
};
use crate::components::Spinner;
use crate::server_fns::dashboards::{create_dashboard, get_dashboard, update_dashboard};

// ─── Editor mode ─────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq)]
enum EditorMode {
    Source,
    Visual,
}

// ─── SVG Icons ───────────────────────────────────────────────────────────────

/// Left-arrow icon for the "Back" link (Heroicons outline ArrowLeft).
#[component]
fn ArrowLeftIcon(#[prop(into, optional)] class: String) -> impl IntoView {
    view! {
        <svg class=class xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 24 24" stroke-width="1.5" stroke="currentColor">
            <path stroke-linecap="round" stroke-linejoin="round" d="M10.5 19.5 3 12m0 0 7.5-7.5M3 12h18" />
        </svg>
    }
}

/// Clock icon (Heroicons outline) for the History button.
#[component]
fn ClockIcon(#[prop(into, optional)] class: String) -> impl IntoView {
    view! {
        <svg class=class xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 24 24" stroke-width="1.5" stroke="currentColor">
            <path stroke-linecap="round" stroke-linejoin="round" d="M12 6v6h4.5m4.5 0a9 9 0 1 1-18 0 9 9 0 0 1 18 0Z" />
        </svg>
    }
}

/// Code bracket icon for Source mode toggle.
#[component]
fn CodeBracketIcon(#[prop(into, optional)] class: String) -> impl IntoView {
    view! {
        <svg class=class fill="none" stroke="currentColor" viewBox="0 0 24 24">
            <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M10 20l4-16m4 4l4 4-4 4M6 16l-4-4 4-4" />
        </svg>
    }
}

/// Eye icon for Visual mode toggle.
#[component]
fn EyeIcon(#[prop(into, optional)] class: String) -> impl IntoView {
    view! {
        <svg class=class fill="none" stroke="currentColor" viewBox="0 0 24 24">
            <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M15 12a3 3 0 11-6 0 3 3 0 016 0z" />
            <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M2.458 12C3.732 7.943 7.523 5 12 5c4.478 0 8.268 2.943 9.542 7-1.274 4.057-5.064 7-9.542 7-4.477 0-8.268-2.943-9.542-7z" />
        </svg>
    }
}

/// Chart bar icon (Heroicons outline) for the "Add Chart" button.
#[component]
fn ChartBarIcon(#[prop(into, optional)] class: String) -> impl IntoView {
    view! {
        <svg class=class xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 24 24" stroke-width="1.5" stroke="currentColor">
            <path stroke-linecap="round" stroke-linejoin="round" d="M3 13.125C3 12.504 3.504 12 4.125 12h2.25c.621 0 1.125.504 1.125 1.125v6.75C7.5 20.496 6.996 21 6.375 21h-2.25A1.125 1.125 0 0 1 3 19.875v-6.75ZM9.75 8.625c0-.621.504-1.125 1.125-1.125h2.25c.621 0 1.125.504 1.125 1.125v11.25c0 .621-.504 1.125-1.125 1.125h-2.25a1.125 1.125 0 0 1-1.125-1.125V8.625ZM16.5 4.125c0-.621.504-1.125 1.125-1.125h2.25C20.496 3 21 3.504 21 4.125v15.75c0 .621-.504 1.125-1.125 1.125h-2.25a1.125 1.125 0 0 1-1.125-1.125V4.125Z" />
        </svg>
    }
}

/// Link icon (Heroicons outline) for the "Insert Link" button.
#[component]
fn LinkIcon(#[prop(into, optional)] class: String) -> impl IntoView {
    view! {
        <svg class=class xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 24 24" stroke-width="1.5" stroke="currentColor">
            <path stroke-linecap="round" stroke-linejoin="round" d="M13.19 8.688a4.5 4.5 0 0 1 1.242 7.244l-4.5 4.5a4.5 4.5 0 0 1-6.364-6.364l1.757-1.757m13.35-.622 1.757-1.757a4.5 4.5 0 0 0-6.364-6.364l-4.5 4.5a4.5 4.5 0 0 0 1.242 7.244" />
        </svg>
    }
}

/// Sparkles icon (Heroicons outline) for the "Copilot" button.
#[component]
fn SparklesIcon(#[prop(into, optional)] class: String) -> impl IntoView {
    view! {
        <svg class=class xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 24 24" stroke-width="1.5" stroke="currentColor">
            <path stroke-linecap="round" stroke-linejoin="round" d="M9.813 15.904 9 18.75l-.813-2.846a4.5 4.5 0 0 0-3.09-3.09L2.25 12l2.846-.813a4.5 4.5 0 0 0 3.09-3.09L9 5.25l.813 2.846a4.5 4.5 0 0 0 3.09 3.09L15.75 12l-2.846.813a4.5 4.5 0 0 0-3.09 3.09ZM18.259 8.715 18 9.75l-.259-1.035a3.375 3.375 0 0 0-2.455-2.456L14.25 6l1.036-.259a3.375 3.375 0 0 0 2.455-2.456L18 2.25l.259 1.035a3.375 3.375 0 0 0 2.455 2.456L21.75 6l-1.036.259a3.375 3.375 0 0 0-2.455 2.456ZM16.894 20.567 16.5 21.75l-.394-1.183a2.25 2.25 0 0 0-1.423-1.423L13.5 18.75l1.183-.394a2.25 2.25 0 0 0 1.423-1.423l.394-1.183.394 1.183a2.25 2.25 0 0 0 1.423 1.423l1.183.394-1.183.394a2.25 2.25 0 0 0-1.423 1.423Z" />
        </svg>
    }
}

// ─── Top-level page component ────────────────────────────────────────────────

/// Dashboard editor page.
///
/// Extracts `:id` from URL params. If `id == "new"`, starts with an empty
/// editor and creates the dashboard on first save. Otherwise fetches the
/// existing dashboard and loads it into the editor.
#[component]
pub fn DashboardEditorPage() -> impl IntoView {
    let params = use_params_map();
    let dashboard_id = Memo::new(move |_| params.get().get("id").unwrap_or_default());

    let is_new = Memo::new(move |_| dashboard_id.get() == "new");

    // Resource created at component level, keyed on dashboard_id
    let dashboard_resource = Resource::new(
        move || dashboard_id.get(),
        get_dashboard,
    );

    view! {
        {move || {
            if is_new.get() {
                // New dashboard: skip fetch, go straight to the editor
                view! {
                    <DashboardEditorInner
                        dashboard_id=None
                        initial_title="Untitled Dashboard".to_string()
                        initial_content=String::new()
                    />
                }
                .into_any()
            } else {
                // Existing dashboard: fetch then render
                view! {
                    <Transition fallback=move || {
                        view! {
                            <div class="flex h-full items-center justify-center bg-muted">
                                <Spinner class="h-8 w-8 text-muted-foreground" />
                            </div>
                        }
                    }>
                        {move || {
                            dashboard_resource
                                .get()
                                .map(|result| match result {
                                    Err(e) => {
                                        view! {
                                            <div class="flex h-full items-center justify-center bg-muted">
                                                <div class="text-center">
                                                    <h2 class="text-lg font-semibold text-foreground mb-4">
                                                        "Dashboard Not Found"
                                                    </h2>
                                                    <p class="text-muted-foreground mb-6">{e.to_string()}</p>
                                                    <a
                                                        href="/dashboards"
                                                        class="inline-flex items-center justify-center gap-2 whitespace-nowrap rounded-md text-sm font-medium transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring h-9 px-4 py-2 bg-primary text-primary-foreground shadow hover:bg-primary/90"
                                                    >
                                                        "Back to Dashboards"
                                                    </a>
                                                </div>
                                            </div>
                                        }
                                        .into_any()
                                    }
                                    Ok(dashboard) => {
                                        view! {
                                            <DashboardEditorInner
                                                dashboard_id=Some(dashboard.dashboard_id.clone())
                                                initial_title=dashboard.title.clone()
                                                initial_content=dashboard.content.clone()
                                            />
                                        }
                                        .into_any()
                                    }
                                })
                        }}
                    </Transition>
                }
                .into_any()
            }
        }}
    }
}

// ─── Inner editor component ──────────────────────────────────────────────────

/// Inner editor component — rendered once data is available.
///
/// Owns all editor state: title, content, mode, unsaved tracking, save logic,
/// history panel integration, keyboard shortcuts, and beforeunload guard.
#[component]
fn DashboardEditorInner(
    /// `None` for new dashboards, `Some(id)` for existing.
    dashboard_id: Option<String>,
    initial_title: String,
    initial_content: String,
) -> impl IntoView {
    // ── Core signals ─────────────────────────────────────────────────────
    let (title, set_title) = signal(initial_title.clone());
    let (editor_content, set_editor_content) = signal(initial_content.clone());
    let (preview_content, set_preview_content) = signal(initial_content.clone());
    let (original_title, set_original_title) = signal(initial_title);
    let (original_content, set_original_content) = signal(initial_content);

    // ── Dashboard ID signal (updated after create) ───────────────────────
    let (current_dashboard_id, set_current_dashboard_id) =
        signal(dashboard_id.clone());

    // ── Editor mode ──────────────────────────────────────────────────────
    let (mode, set_mode) = signal(EditorMode::Visual);

    // ── Save state ───────────────────────────────────────────────────────
    let (saving, set_saving) = signal(false);
    let (save_error, set_save_error) = signal(Option::<String>::None);

    // ── History panel ────────────────────────────────────────────────────
    let (history_open, set_history_open) = signal(false);
    let (history_preview_content, set_history_preview_content) =
        signal(Option::<String>::None);

    // ── Chart builder modal ──────────────────────────────────────────────
    let (chart_builder_open, set_chart_builder_open) = signal(false);

    // ── Insert link modal ────────────────────────────────────────────────
    let (insert_link_open, set_insert_link_open) = signal(false);

    // ── Copilot sidebar ──────────────────────────────────────────────────
    let (copilot_open, set_copilot_open) = signal(false);

    // ── Unsaved changes (derived) ────────────────────────────────────────
    let has_unsaved_changes = Memo::new(move |_| {
        title.get() != original_title.get()
            || editor_content.get() != original_content.get()
    });

    // ── Debounced preview update (500ms) ─────────────────────────────────
    #[cfg(target_arch = "wasm32")]
    let debounce_handle: StoredValue<
        Option<send_wrapper::SendWrapper<gloo_timers::callback::Timeout>>,
    > = StoredValue::new(None);

    let on_editor_change = Arc::new(move |text: String| {
        set_save_error.set(None);
        set_editor_content.set(text.clone());

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
    let trigger_save = move || {
        if saving.get_untracked() || !has_unsaved_changes.get_untracked() {
            return;
        }

        let current_title = title.get_untracked();
        let current_content = editor_content.get_untracked();
        let existing_id = current_dashboard_id.get_untracked();

        set_saving.set(true);
        set_save_error.set(None);

        leptos::task::spawn_local(async move {
            if let Some(did) = existing_id {
                // Update existing dashboard
                match update_dashboard(
                    did,
                    Some(current_title.clone()),
                    Some(current_content.clone()),
                    None,
                )
                .await
                {
                    Ok(()) => {
                        set_original_title.set(current_title);
                        set_original_content.set(current_content);
                        set_saving.set(false);
                    }
                    Err(e) => {
                        set_save_error.set(Some(e.to_string()));
                        set_saving.set(false);
                    }
                }
            } else {
                // Create new dashboard
                match create_dashboard(
                    current_title.clone(),
                    Some(current_content.clone()),
                )
                .await
                {
                    Ok(new_id) => {
                        set_current_dashboard_id.set(Some(new_id.clone()));
                        set_original_title.set(current_title);
                        set_original_content.set(current_content);
                        set_saving.set(false);

                        // Update URL without full navigation
                        #[cfg(target_arch = "wasm32")]
                        {
                            let url = format!("/dashboard/{new_id}/edit");
                            if let Some(window) = web_sys::window() {
                                let _ = window
                                    .history()
                                    .ok()
                                    .and_then(|h| {
                                        h.replace_state_with_url(
                                            &wasm_bindgen::JsValue::NULL,
                                            "",
                                            Some(&url),
                                        )
                                        .ok()
                                    });
                            }
                        }
                    }
                    Err(e) => {
                        set_save_error.set(Some(e.to_string()));
                        set_saving.set(false);
                    }
                }
            }
        });
    };

    // Clone for button handler
    let save_on_click = {
        move |_: leptos::ev::MouseEvent| {
            trigger_save();
        }
    };

    // ── Keyboard shortcut: Cmd/Ctrl+S ────────────────────────────────────
    #[cfg(target_arch = "wasm32")]
    {
        use wasm_bindgen::prelude::*;
        use wasm_bindgen::closure::Closure;

        let trigger_save_kb = trigger_save.clone();

        let handler = Closure::<dyn Fn(web_sys::KeyboardEvent)>::new(
            move |ev: web_sys::KeyboardEvent| {
                if ev.key() == "s" && (ev.meta_key() || ev.ctrl_key()) {
                    ev.prevent_default();
                    trigger_save_kb();
                }
            },
        );

        if let Some(window) = web_sys::window() {
            let _ = window.add_event_listener_with_callback(
                "keydown",
                handler.as_ref().unchecked_ref(),
            );
        }

        // Store for cleanup
        let handler_stored =
            StoredValue::new(Some(send_wrapper::SendWrapper::new(handler)));

        on_cleanup(move || {
            handler_stored.update_value(|h| {
                if let Some(cb) = h.take() {
                    if let Some(window) = web_sys::window() {
                        let _ = window.remove_event_listener_with_callback(
                            "keydown",
                            cb.as_ref().unchecked_ref(),
                        );
                    }
                }
            });
        });
    }

    // ── beforeunload warning ─────────────────────────────────────────────
    #[cfg(target_arch = "wasm32")]
    {
        use wasm_bindgen::prelude::*;
        use wasm_bindgen::closure::Closure;

        let handler = Closure::<dyn Fn(web_sys::BeforeUnloadEvent)>::new(
            move |ev: web_sys::BeforeUnloadEvent| {
                if has_unsaved_changes.get_untracked() {
                    ev.prevent_default();
                    ev.set_return_value("");
                }
            },
        );

        if let Some(window) = web_sys::window() {
            let _ = window.add_event_listener_with_callback(
                "beforeunload",
                handler.as_ref().unchecked_ref(),
            );
        }

        let handler_stored =
            StoredValue::new(Some(send_wrapper::SendWrapper::new(handler)));

        on_cleanup(move || {
            handler_stored.update_value(|h| {
                if let Some(cb) = h.take() {
                    if let Some(window) = web_sys::window() {
                        let _ = window.remove_event_listener_with_callback(
                            "beforeunload",
                            cb.as_ref().unchecked_ref(),
                        );
                    }
                }
            });
        });
    }

    // ── Title input handler ──────────────────────────────────────────────
    let on_title_input = move |ev: leptos::ev::Event| {
        set_title.set(event_target_value(&ev));
    };

    // ── History panel callbacks ──────────────────────────────────────────
    let on_history_preview = Callback::new(move |content: Option<String>| {
        set_history_preview_content.set(content);
    });

    let on_history_restore = Callback::new(move |()| {
        // Cancel any pending debounce so restored content isn't overwritten
        #[cfg(target_arch = "wasm32")]
        debounce_handle.update_value(|h| { drop(h.take()); });

        // Refetch the dashboard to get restored content
        let did = current_dashboard_id.get_untracked();
        set_history_preview_content.set(None);
        set_history_open.set(false);

        if let Some(did) = did {
            leptos::task::spawn_local(async move {
                if let Ok(dashboard) = get_dashboard(did).await {
                    set_title.set(dashboard.title.clone());
                    set_editor_content.set(dashboard.content.clone());
                    set_preview_content.set(dashboard.content.clone());
                    set_original_title.set(dashboard.title);
                    set_original_content.set(dashboard.content);
                }
            });
        }
    });

    let on_history_close = Callback::new(move |()| {
        set_history_open.set(false);
        set_history_preview_content.set(None);
    });

    // ── Back href ────────────────────────────────────────────────────────
    let back_href = Memo::new(move |_| {
        match current_dashboard_id.get() {
            Some(id) => format!("/dashboard/{id}"),
            None => "/dashboards".to_string(),
        }
    });

    // ── Derived: is existing dashboard (for history button visibility) ───
    let is_existing = Memo::new(move |_| current_dashboard_id.get().is_some());

    // ── Preview content: history preview takes priority ──────────────────
    let effective_preview = Signal::derive(move || {
        history_preview_content
            .get()
            .unwrap_or_else(|| preview_content.get())
    });

    let is_previewing_version = Signal::derive(move || {
        history_preview_content.get().is_some()
    });

    view! {
        <div class="flex flex-col h-screen bg-background">
            // ── Toolbar ──────────────────────────────────────────────────
            <div class="flex items-center gap-3 px-4 py-2 border-b border-border bg-card min-h-[52px]">
                // Back link
                <a
                    href=move || back_href.get()
                    class="flex items-center gap-1 text-sm text-muted-foreground hover:text-foreground transition-colors whitespace-nowrap"
                >
                    <ArrowLeftIcon class="w-4 h-4" />
                    <span class="hidden sm:inline">"Dashboard"</span>
                </a>

                // Title input
                <input
                    type="text"
                    class="flex-1 min-w-0 text-lg font-semibold bg-transparent border-none outline-none text-foreground placeholder:text-muted-foreground"
                    placeholder="Dashboard title..."
                    prop:value=move || title.get()
                    on:input=on_title_input
                />

                // Mode toggle: Source / Visual
                <div class="flex items-center bg-accent rounded-md p-0.5">
                    <button
                        class=move || {
                            let base = "px-1.5 sm:px-2 py-1 text-xs font-medium rounded transition-colors flex items-center gap-1";
                            if mode.get() == EditorMode::Source {
                                format!("{base} bg-card text-foreground shadow")
                            } else {
                                format!("{base} text-muted-foreground hover:text-foreground")
                            }
                        }
                        on:click=move |_| set_mode.set(EditorMode::Source)
                        aria-label="Source editor"
                    >
                        <CodeBracketIcon class="w-3.5 h-3.5" />
                        <span class="hidden sm:inline">"Source"</span>
                    </button>
                    <button
                        class=move || {
                            let base = "px-1.5 sm:px-2 py-1 text-xs font-medium rounded transition-colors flex items-center gap-1";
                            if mode.get() == EditorMode::Visual {
                                format!("{base} bg-card text-foreground shadow")
                            } else {
                                format!("{base} text-muted-foreground hover:text-foreground")
                            }
                        }
                        on:click=move |_| set_mode.set(EditorMode::Visual)
                        aria-label="Visual editor"
                    >
                        <EyeIcon class="w-3.5 h-3.5" />
                        <span class="hidden sm:inline">"Visual"</span>
                    </button>
                </div>

                // Add Chart button
                <button
                    class="flex items-center gap-1.5 px-2 md:px-3 py-1.5 text-sm font-medium rounded-lg transition-colors bg-accent text-foreground hover:bg-accent/80"
                    on:click=move |_| set_chart_builder_open.set(true)
                    aria-label="Add chart"
                >
                    <ChartBarIcon class="w-4 h-4 flex-shrink-0" />
                    <span class="hidden sm:inline">"Chart"</span>
                </button>

                // Insert Link button
                <button
                    class="flex items-center gap-1.5 px-2 md:px-3 py-1.5 text-sm font-medium rounded-lg transition-colors bg-accent text-foreground hover:bg-accent/80"
                    on:click=move |_| set_insert_link_open.set(true)
                    aria-label="Insert link"
                >
                    <LinkIcon class="w-4 h-4 flex-shrink-0" />
                    <span class="hidden sm:inline">"Link"</span>
                </button>

                // Unsaved indicator
                {move || {
                    has_unsaved_changes.get().then(|| {
                        view! {
                            <div class="flex items-center gap-1.5 whitespace-nowrap">
                                <span class="w-2 h-2 rounded-full bg-warning-foreground" />
                                <span class="text-xs text-warning-foreground font-medium">"Unsaved"</span>
                            </div>
                        }
                    })
                }}

                // Save error
                {move || {
                    save_error.get().map(|err| {
                        view! {
                            <span class="text-xs text-destructive font-medium truncate max-w-48">
                                {err}
                            </span>
                        }
                    })
                }}

                // Copilot button (only for existing dashboards)
                {move || {
                    is_existing.get().then(|| {
                        view! {
                            <button
                                class=move || {
                                    let base = "flex items-center gap-2 px-2 md:px-3 py-1.5 text-sm font-medium rounded-lg transition-colors";
                                    if copilot_open.get() {
                                        format!("{base} bg-primary/10 text-primary")
                                    } else {
                                        format!("{base} bg-accent text-foreground hover:bg-accent/80")
                                    }
                                }
                                on:click=move |_| {
                                    if copilot_open.get() {
                                        set_copilot_open.set(false);
                                    } else {
                                        // Close history panel (mutual exclusion)
                                        set_history_open.set(false);
                                        set_history_preview_content.set(None);
                                        set_copilot_open.set(true);
                                    }
                                }
                                aria-label="Toggle copilot"
                            >
                                <SparklesIcon class="w-4 h-4 flex-shrink-0" />
                                <span class="hidden sm:inline">"Copilot"</span>
                            </button>
                        }
                    })
                }}

                // History button (only for existing dashboards)
                {move || {
                    is_existing.get().then(|| {
                        view! {
                            <button
                                class=move || {
                                    let base = "flex items-center gap-2 px-2 md:px-3 py-1.5 text-sm font-medium rounded-lg transition-colors";
                                    if history_open.get() {
                                        format!("{base} bg-primary/10 text-primary")
                                    } else {
                                        format!("{base} bg-accent text-foreground hover:bg-accent/80")
                                    }
                                }
                                on:click=move |_| {
                                    if history_open.get() {
                                        set_history_open.set(false);
                                        set_history_preview_content.set(None);
                                    } else {
                                        // Close copilot panel (mutual exclusion)
                                        set_copilot_open.set(false);
                                        set_history_open.set(true);
                                    }
                                }
                                aria-label="Toggle version history"
                            >
                                <ClockIcon class="w-4 h-4 flex-shrink-0" />
                                <span class="hidden sm:inline">"History"</span>
                            </button>
                        }
                    })
                }}

                // Save button
                <button
                    class="inline-flex items-center justify-center gap-2 whitespace-nowrap rounded-md text-sm font-medium transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:pointer-events-none disabled:opacity-50 h-9 px-4 py-2 bg-primary text-primary-foreground shadow hover:bg-primary/90"
                    on:click=save_on_click
                    disabled=move || saving.get() || !has_unsaved_changes.get()
                >
                    {move || {
                        if saving.get() {
                            view! {
                                <Spinner class="w-4 h-4" />
                            }.into_any()
                        } else {
                            view! {
                                // Save icon (Heroicons outline)
                                <svg class="w-4 h-4 flex-shrink-0" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                                    <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M8 7H5a2 2 0 00-2 2v9a2 2 0 002 2h14a2 2 0 002-2V9a2 2 0 00-2-2h-3m-1 4l-3 3m0 0l-3-3m3 3V4" />
                                </svg>
                            }.into_any()
                        }
                    }}
                    <span class="hidden sm:inline">
                        {move || if saving.get() { "Saving..." } else { "Save" }}
                    </span>
                </button>
            </div>

            // ── Two-panel editor + optional history panel ────────────────
            <div class="flex flex-1 min-h-0 overflow-hidden">
                // Main editor area (switches between Source and Visual)
                {move || {
                    if mode.get() == EditorMode::Source {
                        // Source mode: code editor | live preview
                        view! {
                            <div class="flex flex-1 min-h-0 overflow-hidden">
                                // Left panel: Kode editor
                                <div class="flex-1 min-h-0 overflow-hidden border-r border-border">
                                    <DashboardCodeEditor
                                        content=editor_content
                                        on_change=on_editor_change.clone()
                                    />
                                </div>

                                // Right panel: Live preview
                                <div class="flex-1 min-h-0 overflow-y-auto flex flex-col">
                                    // Yellow banner when previewing a history version
                                    {move || {
                                        is_previewing_version.get().then(|| {
                                            view! {
                                                <div class="flex items-center gap-2 p-3 border-b border-warning-border bg-warning flex-shrink-0">
                                                    <span class="text-sm font-medium text-warning-foreground">
                                                        "Previewing historical version"
                                                    </span>
                                                    <span class="text-xs text-warning-foreground">"Read-only"</span>
                                                </div>
                                            }
                                        })
                                    }}

                                    <div class="p-6 max-w-4xl mx-auto flex-1">
                                        <MarkdownRenderer content=effective_preview />
                                    </div>
                                </div>
                            </div>
                        }
                        .into_any()
                    } else {
                        // Visual mode: WYSIWYG editor (full width, no preview panel)
                        view! {
                            <div class="flex flex-1 min-h-0 overflow-hidden">
                                <div class="flex-1 min-h-0 overflow-hidden">
                                    <DashboardWysiwygEditor
                                        content=editor_content
                                        on_change=on_editor_change.clone()
                                    />
                                </div>
                            </div>
                        }
                        .into_any()
                    }
                }}

                // History panel (only for existing dashboards)
                {move || {
                    current_dashboard_id.get().map(|did| {
                        view! {
                            <HistoryPanel
                                dashboard_id=did
                                open=Signal::derive(move || history_open.get())
                                on_close=on_history_close
                                on_preview=on_history_preview
                                on_restore=on_history_restore
                            />
                        }
                    })
                }}

                // Copilot sidebar (only for existing dashboards)
                {move || {
                    current_dashboard_id.get().map(|did| {
                        let on_copilot_close = Callback::new(move |()| {
                            set_copilot_open.set(false);
                        });
                        let on_apply_content = Callback::new(move |content: String| {
                            // Cancel pending debounce to prevent stale preview overwrite
                            #[cfg(target_arch = "wasm32")]
                            debounce_handle.update_value(|h| { drop(h.take()); });
                            set_editor_content.set(content.clone());
                            set_preview_content.set(content);
                        });
                        view! {
                            <CopilotSidebar
                                dashboard_id=did
                                dashboard_content=Signal::derive(move || editor_content.get())
                                open=Signal::derive(move || copilot_open.get())
                                on_close=on_copilot_close
                                on_apply_content=on_apply_content
                            />
                        }
                    })
                }}
            </div>

            // ── Modals (rendered outside the layout flow) ─────────────────
            <ChartBuilderModal
                open=Signal::derive(move || chart_builder_open.get())
                on_close=Callback::new(move |()| set_chart_builder_open.set(false))
                on_insert=Callback::new(move |yaml: String| {
                    // Wrap in ```chartml fence so MarkdownRenderer recognizes it
                    let current = editor_content.get_untracked();
                    let separator = if current.is_empty() || current.ends_with('\n') { "" } else { "\n\n" };
                    let fenced = format!("```chartml\n{yaml}```");
                    let new_content = format!("{current}{separator}{fenced}\n");
                    set_editor_content.set(new_content.clone());
                    set_preview_content.set(new_content);
                    set_chart_builder_open.set(false);
                })
            />

            <InsertDashboardLinkModal
                open=Signal::derive(move || insert_link_open.get())
                on_close=Callback::new(move |()| set_insert_link_open.set(false))
                on_insert=Callback::new(move |link: String| {
                    // Append markdown link to editor content
                    let current = editor_content.get_untracked();
                    let separator = if current.is_empty() || current.ends_with('\n') { "" } else { "\n\n" };
                    let new_content = format!("{current}{separator}{link}\n");
                    set_editor_content.set(new_content.clone());
                    set_preview_content.set(new_content);
                    set_insert_link_open.set(false);
                })
            />
        </div>
    }
}

// ─── Kode CodeEditor wrapper ─────────────────────────────────────────────────

/// Wrapper for the Kode `CodeEditor` that handles conditional compilation.
///
/// On WASM targets, renders the Kode `CodeEditor` with Markdown highlighting.
/// On server (SSR), renders a placeholder since Kode requires browser DOM APIs.
#[component]
fn DashboardCodeEditor(
    #[prop(into)]
    content: Signal<String>,
    on_change: Arc<dyn Fn(String) + Send + Sync>,
) -> impl IntoView {
    #[cfg(target_arch = "wasm32")]
    {
        use kode_leptos::{CodeEditor, Language};

        let editor_theme = crate::pages::sql_editor::code_editor::use_editor_theme();

        view! {
            <CodeEditor
                language=Signal::stored(Language::Markdown)
                content=content
                theme=editor_theme
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

/// WYSIWYG markdown editor wrapper — used for Visual mode.
///
/// On WASM targets, renders the Kode `WysiwygEditor` with rich-text editing.
/// On server (SSR), renders a placeholder since Kode requires browser DOM APIs.
#[component]
fn DashboardWysiwygEditor(
    #[prop(into)]
    content: Signal<String>,
    on_change: Arc<dyn Fn(String) + Send + Sync>,
) -> impl IntoView {
    #[cfg(target_arch = "wasm32")]
    {
        use kode_leptos::TreeWysiwygEditor;

        view! {
            <TreeWysiwygEditor
                content=content
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
