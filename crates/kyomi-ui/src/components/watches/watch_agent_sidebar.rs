// SPDX-License-Identifier: AGPL-3.0-or-later

//! WatchAgentSidebar — AI chat sidebar for watch creation/editing.
//!
//! Ported from `apps/frontend/src/components/watches/WatchAgentSidebar.jsx` (296 lines).
//!
//! Features:
//! - Desktop: Resizable sidebar with drag handle (320-600px, default 420px)
//! - Mobile: Full-width slide-in panel with backdrop
//! - Header with mode-dependent title ("Create Watch" / "Edit Watch")
//! - Live CopilotChat with `watch_copilot` context type and `watch_update` WS events
//!
//! Resize pattern follows `dashboard/copilot_sidebar.rs`.

use leptos::prelude::*;
use leptos_icons::Icon;

use crate::components::chat::CopilotChat;
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
/// Uses the `CopilotChat` component with `watch_copilot` context type,
/// providing full session management, WebSocket streaming, and `watch_update`
/// event handling.
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

    // ── Resize state (desktop only) ──────────────────────────────────────
    let (panel_width, set_panel_width) = signal(DEFAULT_WIDTH);
    #[cfg(not(feature = "hydrate"))]
    let _ = set_panel_width;
    let (is_resizing, set_is_resizing) = signal(false);

    // ── Mode-dependent text ──────────────────────────────────────────────
    let is_editing = editing_watch.is_some();
    let editing_watch_name = editing_watch
        .as_ref()
        .map(|w| w.name.clone())
        .unwrap_or_default();

    let header_title = if is_editing {
        "Edit Watch"
    } else {
        "Create Watch"
    };

    let placeholder_text = StoredValue::new(if is_editing {
        "Make it run hourly instead...".to_string()
    } else {
        "Alert me when daily revenue drops more than 10%...".to_string()
    });

    let empty_title_text = StoredValue::new(if is_editing {
        format!("Editing: {editing_watch_name}")
    } else {
        "What would you like to monitor?".to_string()
    });

    let empty_description_text = StoredValue::new(if is_editing {
        "Tell me what you'd like to change.".to_string()
    } else {
        "Describe what data to watch and when to alert you.".to_string()
    });

    // ── Watch context signal for CopilotChat ─────────────────────────────
    let editing_watch_stored = StoredValue::new(editing_watch);
    let watch_context_signal = Signal::derive(move || {
        editing_watch_stored.with_value(|w| {
            w.as_ref()
                .map(|watch| serde_json::to_string(watch).unwrap_or_default())
                .unwrap_or_default()
        })
    });

    // ── Custom WS event handler ──────────────────────────────────────────
    let on_custom_ws =
        Callback::new(move |(_event_name, _data): (String, serde_json::Value)| {
            on_watch_changed.run(());
        });

    // ── Empty state icon ─────────────────────────────────────────────────
    let empty_icon_fn = StoredValue::new(std::sync::Arc::new(move || {
        view! {
            <Icon icon=icondata_lu::LuEye attr:class="w-10 h-10 text-muted-foreground/50" />
        }
        .into_any()
    }) as ChildrenFn);

    // ── Handle close ─────────────────────────────────────────────────────
    let handle_close = move || {
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
                        class="p-1 text-muted-foreground rounded-md transition-colors hover:text-foreground hover:bg-secondary"
                        aria-label="Close"
                        on:click=move |_| handle_close_clone()
                    >
                        <Icon icon=icondata_lu::LuX attr:class="w-5 h-5" />
                    </button>
                </div>

                // Chat area — live CopilotChat with watch_copilot context
                <CopilotChat
                    context_type="watch_copilot"
                    context_content=watch_context_signal
                    context_label="Watch Configuration"
                    active=Signal::derive(move || open.get())
                    placeholder=placeholder_text.get_value()
                    empty_icon=empty_icon_fn.get_value()
                    empty_title=empty_title_text.get_value()
                    empty_description=empty_description_text.get_value()
                    custom_ws_events=vec!["watch_update".to_string()]
                    on_custom_ws_event=on_custom_ws
                />
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
