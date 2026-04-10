// SPDX-License-Identifier: AGPL-3.0-or-later

//! Dashboard copilot sidebar — conversational AI for editing dashboards.
//!
//! Provides the sidebar chrome (header, resize handle, mobile layout) around the
//! shared `CopilotChat` component. All chat internals (session lifecycle, WebSocket
//! streaming, message rendering, input) are handled by `CopilotChat`.
//!
//! Layout:
//! - Desktop: resizable inline sidebar (320-600px) on the right, with drag handle
//! - Mobile: slide-in panel with backdrop overlay

use leptos::prelude::*;
use leptos_icons::Icon;
#[cfg(feature = "hydrate")]
use wasm_bindgen::prelude::*;

use crate::components::chat::CopilotChat;
use crate::components::{Button, ButtonSize, ButtonVariant};

use super::shared::use_is_mobile;

// ─── Constants ──────────────────────────────────────────────────────────────

#[cfg(feature = "hydrate")]
const MIN_WIDTH: f64 = 320.0;
#[cfg(feature = "hydrate")]
const MAX_WIDTH: f64 = 600.0;
const DEFAULT_WIDTH: f64 = 384.0;

// ─── Main component ─────────────────────────────────────────────────────────

/// Copilot sidebar for dashboard editing.
///
/// Wraps the shared `CopilotChat` component with dashboard-specific sidebar
/// chrome: resizable desktop panel, mobile slide-in, header with close button,
/// and an "Apply to Dashboard" action on assistant messages.
#[component]
pub fn CopilotSidebar(
    /// Dashboard ID to associate the copilot session with.
    dashboard_id: String,
    /// Current dashboard content (markdown) — injected as context with messages.
    #[prop(into)]
    dashboard_content: Signal<String>,
    /// Whether the sidebar is open.
    #[prop(into)]
    open: Signal<bool>,
    /// Callback to close the sidebar.
    on_close: Callback<()>,
    /// Callback when the user clicks "Apply to Dashboard" on an AI response.
    on_apply_content: Callback<String>,
) -> impl IntoView {
    let _dashboard_id = StoredValue::new(dashboard_id);
    let is_mobile = use_is_mobile();

    // ── Panel width (desktop resize) ────────────────────────────────────
    let (panel_width, set_panel_width) = signal(DEFAULT_WIDTH);
    #[cfg(not(feature = "hydrate"))]
    let _ = set_panel_width;
    let (is_resizing, set_is_resizing) = signal(false);

    // ── Handle close ───────────────────────────────────────────────────
    // CopilotChat handles its own session lifecycle via the `active` prop.
    // When `open` becomes false, CopilotChat will delete its session.
    let handle_close = move || {
        on_close.run(());
    };

    // ── Custom WS event handler for dashboard_update ───────────────────
    let on_custom_ws = Callback::new(move |(_event_name, data): (String, serde_json::Value)| {
        if let Some(content) = data.get("content").and_then(|v| v.as_str()) {
            on_apply_content.run(content.to_string());
        }
    });

    // No per-message "Apply to Dashboard" button — the AI applies changes
    // automatically via the `dashboard_update` WS event (handled by on_custom_ws).

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

            // Shared state: holds both closures so mouseup or on_cleanup can drop them.
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

            // Store closures so they stay alive (not leaked via forget).
            *closures.borrow_mut() = Some((move_handler, up_handler));

            // Store cleanup for on_cleanup in case component unmounts mid-drag.
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
    // Both mobile and desktop layouts share this inner content.
    let panel_content = move || {
        let handle_close_clone = handle_close;

        view! {
            <div class="flex flex-col flex-1 min-w-0 h-full">
                // Header
                // React: `flex items-center justify-between px-4 py-3 border-b border-border bg-muted flex-shrink-0`
                <div class="flex items-center justify-between px-4 py-3 border-b border-border bg-muted flex-shrink-0">
                    <div class="flex items-center gap-2">
                        <Icon icon=icondata_lu::LuSparkles width="20" height="20" attr:class="text-primary" />
                        <span class="font-medium text-foreground">"Dashboard Copilot"</span>
                    </div>
                    <Button variant=ButtonVariant::GhostMuted size=ButtonSize::Icon
                        aria_label="Close copilot".to_string()
                        on:click=move |_| handle_close_clone()
                    >
                        <Icon icon=icondata_lu::LuX width="20" height="20" />
                    </Button>
                </div>

                // Chat interface — replaces duplicated chat logic with shared CopilotChat
                <CopilotChat
                    context_type="dashboard_copilot"
                    context_content=dashboard_content
                    context_label="Dashboard Content"
                    active=Signal::derive(move || open.get())
                    placeholder="Ask about your dashboard..."
                    empty_icon=std::sync::Arc::new(|| view! { <Icon icon=icondata_lu::LuSparkles width="48" height="48" /> }.into_any())
                    empty_title="Ask me anything about your dashboard!"
                    empty_description="I can help you improve charts, suggest changes, or make edits directly."
                    custom_ws_events=vec!["dashboard_update".to_string()]
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
                    // React: `fixed top-32 left-0 right-0 bottom-0 bg-[var(--color-overlay)] z-40`
                    // React: `fixed top-32 right-0 bottom-0 w-80 max-w-[85vw] z-50 bg-card flex flex-col shadow-xl`
                    let handle_close_backdrop = handle_close;
                    view! {
                        <div>
                            <div
                                class="fixed top-32 left-0 right-0 bottom-0 bg-[var(--color-overlay)] z-40"
                                on:click=move |_| handle_close_backdrop()
                            />
                            <div class="fixed top-32 right-0 bottom-0 w-80 max-w-[85vw] z-50 bg-muted flex flex-col shadow-xl transition-transform duration-slow ease-in-out">
                                {panel_content()}
                            </div>
                        </div>
                    }.into_any()
                } else {
                    // Desktop: Resizable inline sidebar
                    // React: `border-l border-border bg-card flex h-full overflow-hidden`
                    let width_style = move || format!("width: {}px", panel_width.get());

                    // Apply `select-none` during resize to prevent text selection.
                    let outer_class = move || {
                        if is_resizing.get() {
                            "border-l border-t border-border bg-muted flex h-full overflow-hidden select-none"
                        } else {
                            "border-l border-t border-border bg-muted flex h-full overflow-hidden transition-[width] duration-slow ease-in-out"
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
                                // React: `w-1 h-12 bg-border hover:bg-muted-foreground/50 rounded transition-colors`
                                <div class="w-1 h-12 bg-border hover:bg-muted-foreground/50 rounded-md transition-colors" />
                            </div>

                            {panel_content()}
                        </div>
                    }.into_any()
                }
            }}
        </Show>
    }
}
