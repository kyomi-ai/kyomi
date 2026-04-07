// SPDX-License-Identifier: AGPL-3.0-or-later

//! WatchAgentSidebar — AI chat sidebar for watch creation/editing.
//!
//! Ported from `apps/frontend/src/components/watches/WatchAgentSidebar.jsx` (296 lines).
//!
//! Features:
//! - Desktop: Resizable sidebar with drag handle (320-600px, default 420px)
//! - Mobile: Full-width slide-in panel with backdrop
//! - Header with mode-dependent title ("Create Watch" / "Edit Watch")
//! - Placeholder chat area with text input + send button
//!   (full ChatInterface integration is a separate concern)
//!
//! Resize pattern follows `dashboard/copilot_sidebar.rs`.

use leptos::prelude::*;
use leptos_icons::Icon;

use crate::components::dashboard::shared::use_is_mobile;
use crate::types::WatchListItem;

// ─── Constants ──────────────────────────────────────────────────────────────

#[cfg(feature = "hydrate")]
const MIN_WIDTH: f64 = 320.0;
#[cfg(feature = "hydrate")]
const MAX_WIDTH: f64 = 600.0;
const DEFAULT_WIDTH: f64 = 420.0;

// ─── Main component ─────────────────────────────────────────────────────────

/// AI-powered sidebar for creating and editing watches.
///
/// Uses a placeholder chat interface. The full `ChatInterface` integration
/// with `watch_copilot` context type will be added in a later task.
#[component]
pub fn WatchAgentSidebar(
    /// Whether the sidebar is open.
    #[prop(into)]
    open: Signal<bool>,
    /// Called when sidebar should close.
    on_close: Callback<()>,
    /// Called when a watch is created/updated by the agent.
    on_watch_changed: Callback<()>,
    /// Existing watch being edited (None = create mode).
    editing_watch: Option<WatchListItem>,
) -> impl IntoView {
    let is_mobile = use_is_mobile();
    // Will be used by the full ChatInterface integration.
    let _ = on_watch_changed;

    // ── Resize state (desktop only) ──────────────────────────────────────
    let (panel_width, set_panel_width) = signal(DEFAULT_WIDTH);
    #[cfg(not(feature = "hydrate"))]
    let _ = set_panel_width;
    let (is_resizing, set_is_resizing) = signal(false);

    // ── Chat placeholder state ───────────────────────────────────────────
    // Input value is kept for layout fidelity but sending is disabled
    // until full ChatInterface integration replaces this placeholder.
    let (input_value, set_input_value) = signal(String::new());

    let mode = if editing_watch.is_some() {
        "update"
    } else {
        "create"
    };
    let editing_watch_name = editing_watch
        .as_ref()
        .map(|w| w.name.clone())
        .unwrap_or_default();
    let _editing_watch = StoredValue::new(editing_watch);

    // ── Mode-dependent text ──────────────────────────────────────────────
    let header_title = if mode == "create" {
        "Create Watch"
    } else {
        "Edit Watch"
    };

    let empty_state_message: StoredValue<String> = StoredValue::new(if mode == "create" {
        "What would you like to monitor?".to_string()
    } else {
        format!("Editing: {}", editing_watch_name)
    });

    let empty_state_subtext = if mode == "create" {
        "Describe what data to watch and when to alert you."
    } else {
        "Tell me what you'd like to change."
    };

    let placeholder = if mode == "create" {
        "Alert me when daily revenue drops more than 10%..."
    } else {
        "Make it run hourly instead..."
    };

    // ── Handle close ─────────────────────────────────────────────────────
    let handle_close = move || {
        set_input_value.set(String::new());
        on_close.run(());
    };

    // ── Resize drag handling (desktop) ──────────────────────────────────
    // Stores active drag cleanup so on_cleanup can remove listeners if the
    // component unmounts mid-drag.

    #[cfg(feature = "hydrate")]
    let drag_cleanup: StoredValue<Option<send_wrapper::SendWrapper<Box<dyn FnOnce()>>>> =
        StoredValue::new(None);

    let handle_resize_start = move |ev: web_sys::MouseEvent| {
        ev.prevent_default();
        set_is_resizing.set(true);

        #[cfg(feature = "hydrate")]
        {
            use std::cell::RefCell;
            use std::rc::Rc;
            use wasm_bindgen::prelude::*;
            use wasm_bindgen::closure::Closure;

            let start_x = ev.client_x() as f64;
            let start_w = panel_width.get_untracked();

            let Some(window) = web_sys::window() else {
                return;
            };
            let Some(document) = window.document() else {
                return;
            };

            let move_handler =
                Closure::<dyn FnMut(web_sys::MouseEvent)>::new(move |ev: web_sys::MouseEvent| {
                    let diff = start_x - ev.client_x() as f64;
                    let new_width = (start_w + diff).clamp(MIN_WIDTH, MAX_WIDTH);
                    set_panel_width.set(new_width);
                });

            let move_ref = move_handler
                .as_ref()
                .unchecked_ref::<js_sys::Function>()
                .clone();
            let document_for_up = document.clone();
            let move_fn_for_up = move_ref.clone();

            let closures: Rc<RefCell<Option<(
                Closure<dyn FnMut(web_sys::MouseEvent)>,
                Closure<dyn FnMut()>,
            )>>> = Rc::new(RefCell::new(None));
            let closures_for_up = closures.clone();

            let up_handler = Closure::<dyn FnMut()>::new(move || {
                set_is_resizing.set(false);
                let _ = document_for_up
                    .remove_event_listener_with_callback("mousemove", &move_fn_for_up);
                if let Some((_, ref up_cb)) = *closures_for_up.borrow() {
                    let _ = document_for_up.remove_event_listener_with_callback(
                        "mouseup",
                        up_cb.as_ref().unchecked_ref(),
                    );
                }
                if let Some(body) = document_for_up.body() {
                    let _ = body.style().set_property("cursor", "");
                    let _ = body.style().set_property("user-select", "");
                }
                closures_for_up.borrow_mut().take();
                drag_cleanup.set_value(None);
            });

            let _ = document
                .add_event_listener_with_callback("mousemove", move_ref.unchecked_ref());
            let _ = document
                .add_event_listener_with_callback("mouseup", up_handler.as_ref().unchecked_ref());

            *closures.borrow_mut() = Some((move_handler, up_handler));

            let closures_for_teardown = closures;
            let document_for_teardown = document.clone();
            let move_ref_for_teardown = move_ref.clone();
            let teardown: Box<dyn FnOnce()> = Box::new(move || {
                if let Some((_, ref up_cb)) = *closures_for_teardown.borrow() {
                    let _ = document_for_teardown
                        .remove_event_listener_with_callback("mousemove", &move_ref_for_teardown);
                    let _ = document_for_teardown.remove_event_listener_with_callback(
                        "mouseup",
                        up_cb.as_ref().unchecked_ref(),
                    );
                }
                closures_for_teardown.borrow_mut().take();
            });
            drag_cleanup.set_value(Some(send_wrapper::SendWrapper::new(teardown)));

            if let Some(body) = document.body() {
                let _ = body.style().set_property("cursor", "col-resize");
                let _ = body.style().set_property("user-select", "none");
            }
        }
    };

    #[cfg(feature = "hydrate")]
    on_cleanup(move || {
        if let Some(teardown) = drag_cleanup.try_update_value(|v| v.take()).flatten() {
            teardown.take()();
        }
    });

    // ── Panel content builder ───────────────────────────────────────────
    // Shared between mobile and desktop layouts.
    let panel_content = move || {
        let handle_close_clone = handle_close;
        let empty_state_message = empty_state_message.get_value();

        view! {
            <div class=move || {
                format!(
                    "flex flex-col flex-1 min-w-0{}",
                    if is_mobile.get() { " h-full" } else { "" }
                )
            }>
                // Header
                // React: `flex items-center justify-between px-4 py-3 border-b border-border bg-muted flex-shrink-0`
                <div class="flex items-center justify-between px-4 py-3 border-b border-border bg-muted flex-shrink-0">
                    <div class="flex items-center gap-2">
                        <Icon icon=icondata_lu::LuEye attr:class="w-5 h-5 text-primary" />
                        <span class="font-medium text-foreground">
                            {header_title}
                        </span>
                    </div>
                    <button
                        class="p-1 text-muted-foreground rounded-md transition-colors hover:text-foreground hover:bg-accent"
                        aria-label="Close"
                        on:click=move |_| handle_close_clone()
                    >
                        <Icon icon=icondata_lu::LuX attr:class="w-5 h-5" />
                    </button>
                </div>

                // Chat area — empty state placeholder
                // Full ChatInterface integration will replace this with actual
                // copilot session management and WebSocket streaming.
                <div class="flex-1 overflow-y-auto p-4 space-y-4">
                    <div class="flex flex-col items-center justify-center h-full text-center px-4">
                        <Icon icon=icondata_lu::LuEye attr:class="w-12 h-12 text-muted-foreground/50 mb-3" />
                        <p class="text-muted-foreground text-sm font-medium">
                            {empty_state_message.clone()}
                        </p>
                        <p class="text-muted-foreground/70 text-xs mt-1">
                            {empty_state_subtext}
                        </p>
                    </div>
                </div>

                // Input area — rendered for layout fidelity, send disabled until
                // ChatInterface integration is complete.
                // React: `border-t border-border p-4`
                <div class="border-t border-border p-4">
                    <div class="flex items-end gap-2">
                        <textarea
                            class="flex-1 resize-none rounded-md border border-input bg-background px-3 py-2 text-sm text-foreground placeholder:text-muted-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring min-h-[40px] max-h-[120px]"
                            placeholder=placeholder
                            rows="1"
                            prop:value=move || input_value.get()
                            on:input=move |ev| {
                                set_input_value.set(event_target_value(&ev));
                            }
                        />
                        <button
                            class="inline-flex items-center justify-center rounded-md h-10 w-10 bg-primary text-primary-foreground hover:bg-primary/90 disabled:opacity-50 disabled:cursor-not-allowed transition-colors"
                            disabled=true
                            aria-label="Send message"
                            title="Chat integration coming soon"
                        >
                            <Icon icon=icondata_lu::LuSend attr:class="w-4 h-4" />
                        </button>
                    </div>
                </div>
            </div>
        }
    };

    // ── Render ───────────────────────────────────────────────────────────

    view! {
        <Show when=move || open.get()>
            {move || {
                if is_mobile.get() {
                    // Mobile: Slide-in panel with backdrop
                    // React: `fixed top-16 left-0 right-0 bottom-0 bg-[var(--color-overlay)] z-40`
                    // React: `fixed top-16 right-0 bottom-0 w-full max-w-[92vw] z-50 bg-background flex flex-col shadow-xl`
                    let handle_close_backdrop = handle_close;
                    view! {
                        <div>
                            <div
                                class="fixed top-16 left-0 right-0 bottom-0 bg-[var(--color-overlay)] z-40"
                                on:click=move |_| handle_close_backdrop()
                            />
                            <div class="fixed top-16 right-0 bottom-0 w-full max-w-[92vw] z-50 bg-background flex flex-col shadow-xl">
                                {panel_content()}
                            </div>
                        </div>
                    }.into_any()
                } else {
                    // Desktop: Resizable inline sidebar
                    // React: `border-l border-border bg-background flex h-full overflow-hidden`
                    let width_style = move || format!("width: {}px", panel_width.get());

                    let outer_class = move || {
                        if is_resizing.get() {
                            "border-l border-border bg-background flex h-full overflow-hidden select-none"
                        } else {
                            "border-l border-border bg-background flex h-full overflow-hidden"
                        }
                    };

                    view! {
                        <div
                            class=outer_class
                            style=width_style
                        >
                            // Resize Handle
                            // React: `flex items-center justify-center cursor-col-resize select-none px-1 -mr-2 relative z-10`
                            <div
                                class="flex items-center justify-center cursor-col-resize select-none px-1 -mr-2 relative z-10"
                                on:mousedown=handle_resize_start
                                aria-label="Drag to resize"
                            >
                                // React: `w-1 h-12 bg-border hover:bg-muted-foreground rounded transition-colors`
                                <div class="w-1 h-12 bg-border hover:bg-muted-foreground rounded-md transition-colors" />
                            </div>

                            {panel_content()}
                        </div>
                    }.into_any()
                }
            }}
        </Show>
    }
}
