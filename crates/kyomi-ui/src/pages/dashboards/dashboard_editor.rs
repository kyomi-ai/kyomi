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
//! │   (language=markdown)        │   (MarkdownRenderer)             │
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
use phosphor_leptos::Icon;
use leptos_router::hooks::use_params_map;

use crate::components::dashboard::{
    ChartBuilderModal, ChartInfoModal, CopilotSidebar, HistoryPanel, InsertDashboardLinkModal,
    MarkdownRenderer, markdown_renderer::kyomi_palette,
};
use crate::components::{Button, ButtonLink, ButtonSize, ButtonVariant, ToggleButton, Spinner};
use crate::server_fns::context::UserContext;
use crate::server_fns::dashboards::{create_dashboard, get_dashboard, update_dashboard};

// ─── Editor mode ─────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq)]
enum EditorMode {
    Source,
    Visual,
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

    let location = leptos_router::hooks::use_location();
    let is_knowledge = Memo::new(move |_| location.pathname.get().starts_with("/knowledge"));

    // Resource created at component level, keyed on dashboard_id
    let dashboard_resource = Resource::new(
        move || dashboard_id.get(),
        get_dashboard,
    );

    view! {
        {move || {
            if is_new.get() {
                // New dashboard / document: skip fetch, go straight to the editor.
                // Title placeholder is noun-specific so the user sees the right word.
                let initial_title = if is_knowledge.get() { "Untitled Document" } else { "Untitled Dashboard" };
                view! {
                    <DashboardEditorInner
                        dashboard_id=None
                        initial_title=initial_title.to_string()
                        initial_content=String::new()
                    />
                }
                .into_any()
            } else {
                // Existing dashboard: fetch then render
                view! {
                    <Transition fallback=move || {
                        view! {
                            <div class="flex h-full items-center justify-center bg-background">
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
                                            <div class="flex h-full items-center justify-center bg-background">
                                                <div class="text-center">
                                                    <h2 class="text-lg font-semibold text-foreground mb-4">
                                                        {if is_knowledge.get() { "Knowledge Document Not Found" } else { "Dashboard Not Found" }}
                                                    </h2>
                                                    <p class="text-muted-foreground mb-6">{e.to_string()}</p>
                                                    <ButtonLink href=(if is_knowledge.get() { "/knowledge" } else { "/dashboards" }).to_string()>
                                                        {if is_knowledge.get() { "Back to Knowledge" } else { "Back to Dashboards" }}
                                                    </ButtonLink>
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
/// Source/Visual mode toggle pill — rendered inside the editor area, not the header.
/// Matches React's `modeToggle` which appears in the toolbar row.
fn editor_mode_toggle(mode: ReadSignal<EditorMode>, set_mode: WriteSignal<EditorMode>) -> impl IntoView {
    view! {
        <div class="flex items-center bg-muted rounded-md p-0.5 flex-shrink-0 font-sans">
            <button
                class=move || {
                    let base = "px-1.5 sm:px-2 py-1 text-xs font-medium rounded-md transition-colors flex items-center gap-1";
                    if mode.get() == EditorMode::Source {
                        format!("{base} bg-card text-foreground shadow-sm")
                    } else {
                        format!("{base} text-muted-foreground hover:text-foreground")
                    }
                }
                on:click=move |_| set_mode.set(EditorMode::Source)
                aria-label="Source editor"
            >
                <Icon icon=phosphor_leptos::CODE attr:class="w-3.5 h-3.5" />
                <span class="hidden sm:inline">"Source"</span>
            </button>
            <button
                class=move || {
                    let base = "px-1.5 sm:px-2 py-1 text-xs font-medium rounded-md transition-colors flex items-center gap-1";
                    if mode.get() == EditorMode::Visual {
                        format!("{base} bg-card text-foreground shadow-sm")
                    } else {
                        format!("{base} text-muted-foreground hover:text-foreground")
                    }
                }
                on:click=move |_| set_mode.set(EditorMode::Visual)
                aria-label="Visual editor"
            >
                <Icon icon=phosphor_leptos::EYE attr:class="w-3.5 h-3.5" />
                <span class="hidden sm:inline">"Visual"</span>
            </button>
        </div>
    }
}

#[component]
fn DashboardEditorInner(
    /// `None` for new dashboards, `Some(id)` for existing.
    dashboard_id: Option<String>,
    initial_title: String,
    initial_content: String,
) -> impl IntoView {
    // This page is mounted at both /dashboard/:id/edit and /knowledge/:id/edit.
    // Derive section-specific labels and URL bases from the current pathname
    // so back/save navigation stays inside the right section.
    let location = leptos_router::hooks::use_location();
    let is_knowledge = Memo::new(move |_| location.pathname.get().starts_with("/knowledge"));
    let list_href = move || if is_knowledge.get() { "/knowledge" } else { "/dashboards" };
    let base_path = move || if is_knowledge.get() { "/knowledge" } else { "/dashboard" };

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
    let (save_success, set_save_success) = signal(false);

    // ── History panel ────────────────────────────────────────────────────
    let (history_open, set_history_open) = signal(false);
    let (history_preview_content, set_history_preview_content) =
        signal(Option::<String>::None);

    // ── Chart builder modal ──────────────────────────────────────────────
    let (chart_builder_open, set_chart_builder_open) = signal(false);
    let (edit_chart_yaml, set_edit_chart_yaml) = signal(Option::<String>::None);

    // ── Insert link modal ────────────────────────────────────────────────
    let (insert_link_open, set_insert_link_open) = signal(false);

    // ── Chart info modal ────────────────────────────────────────────────
    let (chart_info_open, set_chart_info_open) = signal(false);
    let (chart_info_yaml, set_chart_info_yaml) = signal(String::new());

    // ── Inject content at cursor (used by modals to insert at cursor position) ──
    let inject = RwSignal::new(Option::<kode_leptos::InjectCommand>::None);

    // ── Copilot sidebar ──────────────────────────────────────────────────
    let (copilot_open, set_copilot_open) = signal(false);

    // ── User context (for chart palette) ────────────────────────────────
    // Provided by the parent Layout.
    let user_ctx_resource =
        expect_context::<LocalResource<Result<UserContext, ServerFnError>>>();
    let chart_palette = Memo::new(move |_| {
        user_ctx_resource
            .get()
            .and_then(|r| r.ok())
            .map(|ctx| ctx.chart_palette)
    });

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
                        set_save_success.set(true);
                        #[cfg(target_arch = "wasm32")]
                        gloo_timers::callback::Timeout::new(2000, move || {
                            set_save_success.set(false);
                        }).forget();
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
                        set_save_success.set(true);
                        #[cfg(target_arch = "wasm32")]
                        gloo_timers::callback::Timeout::new(2000, move || {
                            set_save_success.set(false);
                        }).forget();

                        // Update URL without full navigation
                        #[cfg(target_arch = "wasm32")]
                        {
                            let url = format!("{}/{new_id}/edit", base_path());
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

    // ── Chart info event listener (from WYSIWYG extension) ────────────
    #[cfg(target_arch = "wasm32")]
    {
        use wasm_bindgen::prelude::*;
        use wasm_bindgen::closure::Closure;

        let handler = Closure::<dyn Fn(web_sys::CustomEvent)>::new(
            move |ev: web_sys::CustomEvent| {
                if let Some(yaml) = ev.detail().as_string() {
                    set_chart_info_yaml.set(yaml);
                    set_chart_info_open.set(true);
                }
            },
        );

        if let Some(window) = web_sys::window() {
            let _ = window.add_event_listener_with_callback(
                "chart-info-request",
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
                            "chart-info-request",
                            cb.as_ref().unchecked_ref(),
                        );
                    }
                }
            });
        });
    }

    // ── Chart edit event listener (from WYSIWYG extension + source preview) ──
    #[cfg(target_arch = "wasm32")]
    {
        use wasm_bindgen::prelude::*;
        use wasm_bindgen::closure::Closure;

        let handler = Closure::<dyn Fn(web_sys::CustomEvent)>::new(
            move |ev: web_sys::CustomEvent| {
                if let Some(yaml) = ev.detail().as_string() {
                    set_edit_chart_yaml.set(Some(yaml.trim().to_string()));
                    set_chart_builder_open.set(true);
                }
            },
        );

        if let Some(window) = web_sys::window() {
            let _ = window.add_event_listener_with_callback(
                "chart-edit-request",
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
                            "chart-edit-request",
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
            Some(id) => format!("{}/{id}", base_path()),
            None => list_href().to_string(),
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

    // Editor content: show historical version when previewing, current content otherwise
    let effective_editor_content = Signal::derive(move || {
        history_preview_content
            .get()
            .unwrap_or_else(|| editor_content.get())
    });

    let is_previewing_version = Signal::derive(move || {
        history_preview_content.get().is_some()
    });

    view! {
        <div class="flex flex-col h-full bg-background overflow-hidden @container">
            // ── Header bar — matches viewer: page-header pattern ────────
            <div class="page-header h-16 px-4 md:px-6 flex-shrink-0 flex items-center justify-between">
                // Left: back button + title input
                <div class="flex items-center gap-4 flex-1 min-w-0 overflow-hidden">
                    <ButtonLink href=list_href().to_string() variant=ButtonVariant::Ghost size=ButtonSize::Icon
                        class="flex-shrink-0 text-muted-foreground hover:text-foreground"
                        aria_label=(if is_knowledge.get() { "Back to knowledge" } else { "Back to dashboards" }).to_string()>
                        <Icon icon=phosphor_leptos::CARET_LEFT size="18px" />
                    </ButtonLink>

                    <div class="min-w-0 flex-1">
                        <input
                            type="text"
                            class="text-3xl font-display text-foreground bg-transparent border-none outline-none w-full placeholder:text-muted-foreground truncate"
                            placeholder=if is_knowledge.get() { "Untitled Document" } else { "Untitled Dashboard" }
                            prop:value=move || title.get()
                            on:input=on_title_input
                        />
                    </div>
                </div>

                // Right: action buttons — matches viewer responsive toolbar pattern
                <div class="flex items-center gap-1 @6xl:gap-2 flex-shrink-0">
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

                    // History button (only for existing dashboards)
                    {move || {
                        is_existing.get().then(|| {
                            view! {
                                <div class="hidden @3xl:flex">
                                    <ToggleButton
                                        variant=Signal::derive(move || if history_open.get() { ButtonVariant::Active } else { ButtonVariant::Secondary })
                                        size=ButtonSize::Sm
                                        aria_label="Toggle version history"
                                        on:click=move |_| {
                                            if history_open.get() {
                                                set_history_open.set(false);
                                                set_history_preview_content.set(None);
                                            } else {
                                                set_copilot_open.set(false);
                                                set_history_open.set(true);
                                            }
                                        }
                                    >
                                        <Icon icon=phosphor_leptos::CLOCK size="14px" />
                                        <span class="hidden @6xl:inline whitespace-nowrap">"History"</span>
                                    </ToggleButton>
                                </div>
                            }
                        })
                    }}

                    // Copilot button (only for existing dashboards)
                    {move || {
                        is_existing.get().then(|| {
                            view! {
                                <div class="hidden @3xl:flex">
                                    <ToggleButton
                                        variant=Signal::derive(move || if copilot_open.get() { ButtonVariant::Active } else { ButtonVariant::Secondary })
                                        size=ButtonSize::Sm
                                        aria_label="Toggle copilot"
                                        on:click=move |_| {
                                            if copilot_open.get() {
                                                set_copilot_open.set(false);
                                            } else {
                                                set_history_open.set(false);
                                                set_history_preview_content.set(None);
                                                set_copilot_open.set(true);
                                            }
                                        }
                                    >
                                        <Icon icon=phosphor_leptos::SPARKLE size="14px" />
                                        <span class="hidden @6xl:inline whitespace-nowrap">"Copilot"</span>
                                    </ToggleButton>
                                </div>
                            }
                        })
                    }}

                    // Close button — navigates back to dashboard view or list
                    <Button variant=ButtonVariant::Secondary size=ButtonSize::Sm
                        aria_label="Close editor".to_string()
                        on:click=move |_| {
                            let href = back_href.get();
                            let nav = leptos_router::hooks::use_navigate();
                            nav(&href, leptos_router::NavigateOptions::default());
                        }
                    >
                        <Icon icon=phosphor_leptos::X size="14px" />
                        <span class="hidden @6xl:inline whitespace-nowrap">"Close"</span>
                    </Button>

                    // Save button
                    <Button
                        size=ButtonSize::Sm
                        disabled=Signal::derive(move || saving.get() || !has_unsaved_changes.get())
                        on:click=save_on_click
                    >
                        {move || {
                            if saving.get() {
                                view! { <Spinner class="w-4 h-4" /> }.into_any()
                            } else if save_success.get() {
                                view! { <Icon icon=phosphor_leptos::CHECK size="14px" /> }.into_any()
                            } else {
                                view! { <Icon icon=phosphor_leptos::FLOPPY_DISK size="14px" /> }.into_any()
                            }
                        }}
                        <span class="hidden @6xl:inline whitespace-nowrap">
                            {move || if saving.get() { "Saving..." } else if save_success.get() { "Saved!" } else { "Save" }}
                        </span>
                    </Button>
                </div>
            </div>

            // ── Content area — no card wrapper, matches viewer bg-background ──
            <div class="flex-1 overflow-hidden flex">
                <div class="flex-1 overflow-hidden flex flex-col bg-background">
                // Main editor area (switches between Source and Visual)
                {move || {
                    if mode.get() == EditorMode::Source {
                        // Source mode: sub-toolbar + code editor + live preview
                        view! {
                            <div class="flex flex-col flex-1 min-h-0">
                                // Sub-toolbar: "Markdown" label + mode toggle
                                // Yellow bg when previewing version (matches React DashboardEditor.jsx:849)
                                <div class=move || {
                                    if is_previewing_version.get() {
                                        "flex items-center gap-2 p-3 border-b border-warning-border bg-warning flex-shrink-0"
                                    } else {
                                        "flex items-center gap-2 p-3 border-b border-border bg-muted flex-shrink-0"
                                    }
                                }>
                                    {move || {
                                        if is_previewing_version.get() {
                                            view! {
                                                <>
                                                    <span class="text-sm font-medium text-warning-foreground">"Previewing historical version"</span>
                                                    <span class="text-xs text-warning-foreground">"Read-only"</span>
                                                </>
                                            }.into_any()
                                        } else {
                                            view! {
                                                <span class="text-xs text-muted-foreground">"Markdown"</span>
                                            }.into_any()
                                        }
                                    }}
                                    <div class="flex-1" />
                                    {editor_mode_toggle(mode, set_mode)}
                                </div>
                                // Editor panels
                                <div class="flex flex-1 min-h-0 overflow-hidden">
                                    // Left panel: Kode editor (read-only when previewing history)
                                    <div class="flex-1 min-h-0 overflow-hidden border-r border-border">
                                        <DashboardCodeEditor
                                            content=effective_editor_content
                                            on_change=on_editor_change.clone()
                                        />
                                    </div>

                                    // Right panel: Live preview — wrapped in dashboard-content
                                    // for chart container styling (border, bg, header bar)
                                    <div class="flex-1 min-h-0 overflow-y-auto flex flex-col">
                                        <div class="dashboard-content p-6 flex-1">
                                            <MarkdownRenderer
                                                content=effective_preview
                                                chart_palette=chart_palette.get().unwrap_or_else(|| "kyomi".to_string())
                                                on_chart_info=Callback::new(move |yaml: String| {
                                                    set_chart_info_yaml.set(yaml);
                                                    set_chart_info_open.set(true);
                                                })
                                                on_edit_chart=Callback::new(move |(block_index, _array_index): (usize, usize)| {
                                                    // Extract the YAML at block_index from editor content
                                                    let content = editor_content.get_untracked();
                                                    let re = regex::Regex::new(r"(?s)```chartml\s*\n(.*?)```").unwrap();
                                                    if let Some(cap) = re.captures_iter(&content).nth(block_index) {
                                                        let yaml = cap.get(1).map_or("", |m| m.as_str()).trim().to_string();
                                                        set_edit_chart_yaml.set(Some(yaml));
                                                        set_chart_builder_open.set(true);
                                                    }
                                                })
                                            />
                                        </div>
                                    </div>
                                </div>
                            </div>
                        }
                        .into_any()
                    } else {
                        // Visual mode: toolbar matches React — H1-H3 | B I <> | lists | Add Chart, Link | spacer | Source/Visual
                        let set_chart_open = set_chart_builder_open;
                        let set_link_open = set_insert_link_open;

                        use kode_leptos::{BuiltinButton, CustomToolbarButton, ToolbarItem};

                        let items = vec![
                            // Headings
                            ToolbarItem::Builtin(BuiltinButton::H1),
                            ToolbarItem::Builtin(BuiltinButton::H2),
                            ToolbarItem::Builtin(BuiltinButton::H3),
                            ToolbarItem::Separator,
                            // Formatting
                            ToolbarItem::Builtin(BuiltinButton::Bold),
                            ToolbarItem::Builtin(BuiltinButton::Italic),
                            ToolbarItem::BuiltinWithView(BuiltinButton::InlineCode, view! {
                                // Lucide Code icon
                                <svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                                    <path d="m16 18 6-6-6-6" /><path d="m8 6-6 6 6 6" />
                                </svg>
                            }.into_any()),
                            ToolbarItem::Separator,
                            // Lists + blocks
                            ToolbarItem::BuiltinWithView(BuiltinButton::BulletList, view! {
                                // Lucide List icon
                                <svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                                    <path d="M3 12h.01" /><path d="M3 18h.01" /><path d="M3 6h.01" />
                                    <path d="M8 12h13" /><path d="M8 18h13" /><path d="M8 6h13" />
                                </svg>
                            }.into_any()),
                            ToolbarItem::BuiltinWithView(BuiltinButton::OrderedList, view! {
                                // Lucide ListOrdered icon
                                <svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                                    <path d="M10 12h11" /><path d="M10 18h11" /><path d="M10 6h11" />
                                    <path d="M4 10h2" /><path d="M4 6h1v4" /><path d="M6 18H4c0-1 2-2 2-3s-1-1.5-2-1" />
                                </svg>
                            }.into_any()),
                            ToolbarItem::BuiltinWithView(BuiltinButton::Blockquote, view! {
                                // Lucide TextQuote icon
                                <svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                                    <path d="M17 6H3" /><path d="M21 12H8" /><path d="M21 18H8" /><path d="M3 12v6" />
                                </svg>
                            }.into_any()),
                            ToolbarItem::BuiltinWithView(BuiltinButton::CodeBlock, view! {
                                // Lucide FileCode icon
                                <svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                                    <path d="M10 12.5 8 15l2 2.5" /><path d="m14 12.5 2 2.5-2 2.5" />
                                    <path d="M14 2v4a2 2 0 0 0 2 2h4" /><path d="M15 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V7z" />
                                </svg>
                            }.into_any()),
                            ToolbarItem::Separator,
                            // Add Chart — uses kode-toolbar-button class to match builtins
                            ToolbarItem::Custom(CustomToolbarButton {
                                label: view! {
                                    <Icon icon=phosphor_leptos::CHART_BAR size="16px" />
                                    <span class="hidden sm:inline">"Add Chart"</span>
                                }.into_any(),
                                title: "Add Chart".to_string(),
                                on_click: Arc::new(move || set_chart_open.set(true)),
                                class: Some("kode-toolbar-button flex items-center gap-1".to_string()),
                            }),
                            // Link — uses kode-toolbar-button class to match builtins
                            ToolbarItem::Custom(CustomToolbarButton {
                                label: view! {
                                    <Icon icon=phosphor_leptos::LINK size="16px" />
                                    <span class="hidden sm:inline">"Link"</span>
                                }.into_any(),
                                title: if is_knowledge.get_untracked() { "Link to Document".to_string() } else { "Link to Dashboard".to_string() },
                                on_click: Arc::new(move || set_link_open.set(true)),
                                class: Some("kode-toolbar-button flex items-center gap-1".to_string()),
                            }),
                            // Push mode toggle to far right
                            ToolbarItem::Spacer,
                            // Source/Visual mode toggle
                            ToolbarItem::Slot(editor_mode_toggle(mode, set_mode).into_any()),
                        ];

                        view! {
                            <div class="flex flex-1 min-h-0 overflow-hidden">
                                <div class="flex-1 min-h-0 overflow-hidden">
                                    <DashboardWysiwygEditor
                                        content=effective_editor_content
                                        on_change=on_editor_change.clone()
                                        chart_colors={
                                            let is_dark = crate::components::theme::use_theme()
                                                .map(|s| s.effective.get() == "dark")
                                                .unwrap_or(false);
                                            kyomi_palette(&chart_palette.get().unwrap_or_else(|| "kyomi".to_string()), is_dark)
                                        }
                                        toolbar_items=items
                                        inject=inject
                                    />
                                </div>
                            </div>
                        }
                        .into_any()
                    }
                }}
                </div>

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
            // Chart builder modal — Show/hide based on open signal to avoid
            // disposal panics from reactive re-rendering.
            <Show when=move || chart_builder_open.get()>
                {move || {
                    let existing = edit_chart_yaml.get_untracked();
                    let close_cb = Callback::new(move |()| {
                        set_chart_builder_open.set(false);
                        set_edit_chart_yaml.set(None);
                    });
                    let insert_cb = Callback::new(move |yaml: String| {
                        let current = editor_content.get_untracked();
                        let new_fenced = format!("```chartml\n{yaml}\n```");

                        let new_content = if let Some(old_yaml) = edit_chart_yaml.get_untracked() {
                            let old_fenced = format!("```chartml\n{old_yaml}\n```");
                            current.replace(&old_fenced, &new_fenced)
                        } else {
                            let separator = if current.is_empty() || current.ends_with('\n') { "" } else { "\n\n" };
                            format!("{current}{separator}{new_fenced}\n")
                        };

                        set_editor_content.set(new_content.clone());
                        set_preview_content.set(new_content);
                        set_chart_builder_open.set(false);
                        set_edit_chart_yaml.set(None);
                    });
                    let open_sig = Signal::derive(move || chart_builder_open.get());
                    match existing {
                        Some(yaml) => view! {
                            <ChartBuilderModal open=open_sig existing_yaml=yaml on_close=close_cb on_insert=insert_cb />
                        }.into_any(),
                        None => view! {
                            <ChartBuilderModal open=open_sig on_close=close_cb on_insert=insert_cb />
                        }.into_any(),
                    }
                }}
            </Show>

            <InsertDashboardLinkModal
                open=Signal::derive(move || insert_link_open.get())
                on_close=Callback::new(move |()| set_insert_link_open.set(false))
                on_insert=Callback::new(move |link: String| {
                    // Parse the markdown link [text](url) and insert at cursor
                    if let (Some(text), Some(url)) = (
                        link.strip_prefix('[').and_then(|s| s.split("](").next()),
                        link.split("](").nth(1).and_then(|s| s.strip_suffix(')')),
                    ) {
                        inject.set(Some(kode_leptos::InjectCommand::Link {
                            text: text.to_string(),
                            url: url.to_string(),
                        }));
                    }
                    set_insert_link_open.set(false);
                })
            />

            <ChartInfoModal
                open=Signal::derive(move || chart_info_open.get())
                yaml=chart_info_yaml
                on_close=Callback::new(move |()| set_chart_info_open.set(false))
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
        use crate::components::dashboard::chartml_completion::chartml_markdown_completion_provider;

        let editor_theme = crate::pages::sql_editor::code_editor::use_editor_theme();
        let completion_providers = Signal::stored(vec![chartml_markdown_completion_provider()]);

        view! {
            <CodeEditor
                language=Signal::stored(Language::new_static("markdown"))
                content=content
                theme=editor_theme
                on_change=on_change
                completion_providers=completion_providers
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
    #[prop(optional)]
    chart_colors: Option<Vec<String>>,
    #[prop(optional)]
    toolbar_items: Option<Vec<kode_leptos::ToolbarItem>>,
    /// Signal for injecting text at the cursor position from outside the editor.
    #[prop(optional)]
    inject: Option<RwSignal<Option<kode_leptos::InjectCommand>>>,
) -> impl IntoView {
    #[cfg(target_arch = "wasm32")]
    {
        use std::sync::Arc;
        use kode_leptos::TreeWysiwygEditor;
        use crate::components::dashboard::chartml_extension::ChartMLExtension;

        let editor_theme = crate::pages::sql_editor::code_editor::use_editor_theme();

        let is_dark = crate::components::theme::use_theme()
            .map(|s| s.effective.get_untracked() == "dark")
            .unwrap_or(false);
        let kyomi_chart_theme =
            crate::components::dashboard::markdown_renderer::kyomi_theme(is_dark);
        let extension = match chart_colors {
            Some(colors) => {
                ChartMLExtension::with_colors_and_theme(colors, kyomi_chart_theme)
            }
            None => ChartMLExtension::new(),
        };
        let extensions: Vec<Arc<dyn kode_leptos::extension::Extension>> = vec![
            Arc::new(extension),
        ];


        match (toolbar_items, inject) {
            (Some(items), Some(inj)) => view! {
                <TreeWysiwygEditor
                    content=content on_change=on_change theme=editor_theme
                    extensions=extensions container_max_width="100%"
                    toolbar_items=items inject=inj
                />
            }.into_any(),
            (Some(items), None) => view! {
                <TreeWysiwygEditor
                    content=content on_change=on_change theme=editor_theme
                    extensions=extensions container_max_width="100%"
                    toolbar_items=items
                />
            }.into_any(),
            (None, Some(inj)) => view! {
                <TreeWysiwygEditor
                    content=content on_change=on_change theme=editor_theme
                    extensions=extensions container_max_width="100%"
                    inject=inj
                />
            }.into_any(),
            (None, None) => view! {
                <TreeWysiwygEditor
                    content=content on_change=on_change theme=editor_theme
                    extensions=extensions container_max_width="100%"
                />
            }.into_any(),
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = content;
        let _ = on_change;
        let _ = chart_colors;
        let _ = toolbar_items;
        let _ = inject;

        view! {
            <div class="flex-1 bg-muted p-4 text-muted-foreground">
                "Loading editor..."
            </div>
        }
        .into_any()
    }
}

