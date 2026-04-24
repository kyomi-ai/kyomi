// SPDX-License-Identifier: AGPL-3.0-or-later

//! Shared right-side panel — Editorial Margin pattern per DESIGN.md.
//!
//! One component drives copilot, collections, and catalog sidebars. Handles:
//! - Desktop resizable inline panel with width animation
//! - Mobile overlay with backdrop, slide-in from right
//! - Editorial Margin styling: bg-background (desktop) continues the page surface,
//!   separated only by a 1px border-l. Mobile falls back to bg-muted so the
//!   overlay reads as a sheet.
//! - Simple header (§ mark + Instrument Serif title) or a custom header slot for
//!   tabbed/complex chrome. Close button is always rendered by the panel itself.
//! - Escape-to-close + mobile backdrop click
//! - Resize drag with document-level listeners + on_cleanup teardown

use leptos::prelude::*;
use phosphor_leptos::{Icon, IconWeight};
#[cfg(feature = "hydrate")]
use wasm_bindgen::prelude::*;

use crate::components::dashboard::shared::use_is_mobile;
use crate::components::{Button, ButtonSize, ButtonVariant};

// ─── Defaults ───────────────────────────────────────────────────────────────

const DEFAULT_MIN_WIDTH: f64 = 280.0;
const DEFAULT_MAX_WIDTH: f64 = 600.0;

// ─── Type aliases ────────────────────────────────────────────────────────────

/// A `StoredValue` holding an optional once-callable teardown function,
/// wrapped in `SendWrapper` for cross-thread Leptos storage.
#[cfg(feature = "hydrate")]
type CleanupSlot = StoredValue<Option<send_wrapper::SendWrapper<Box<dyn FnOnce()>>>>;

/// Pair of WASM closures kept alive until the drag interaction completes.
#[cfg(feature = "hydrate")]
type DragClosures = std::rc::Rc<
    std::cell::RefCell<
        Option<(
            wasm_bindgen::closure::Closure<dyn FnMut(web_sys::MouseEvent)>,
            wasm_bindgen::closure::Closure<dyn FnMut()>,
        )>,
    >,
>;

// ─── Component ──────────────────────────────────────────────────────────────

/// Right-side docked panel with Editorial Margin styling.
///
/// Always mounted — width (desktop) or transform (mobile) animates between
/// open and closed states so CSS transitions work. See DESIGN.md "Right Panel
/// Pattern" section.
#[component]
pub fn RightPanel(
    /// Whether the panel is open.
    #[prop(into)]
    open: Signal<bool>,
    /// Called on close button click, backdrop click (mobile), or Escape.
    on_close: Callback<()>,
    /// Current panel width in pixels (desktop only). Caller owns this signal so
    /// they can persist to localStorage if desired. Clamped to `[min_width, max_width]`.
    #[prop(into)]
    width: RwSignal<f64>,
    /// Minimum width in pixels (default 280).
    #[prop(default = DEFAULT_MIN_WIDTH)]
    min_width: f64,
    /// Maximum width in pixels (default 600).
    #[prop(default = DEFAULT_MAX_WIDTH)]
    max_width: f64,
    /// Optional simple title — renders `§ Title` in Instrument Serif with the
    /// amber mark. If `header` is also provided, `header` wins.
    #[prop(optional, into)]
    title: Option<String>,
    /// Optional custom header content — fills the space left of the close button.
    /// Use this for tabbed headers (catalog) or any layout that doesn't fit the
    /// simple `§ Title` form.
    #[prop(optional)]
    header: Option<ChildrenFn>,
    /// Body content — scrollable main region.
    children: ChildrenFn,
    /// Optional footer — rendered below the body with a `border-t`. Used for
    /// chat input rows, footer actions, or metadata.
    #[prop(optional)]
    footer: Option<ChildrenFn>,
    /// Accessible label for the close button. Default: "Close panel".
    #[prop(default = "Close panel".to_string())]
    close_label: String,
) -> impl IntoView {
    let is_mobile = use_is_mobile();
    let (is_resizing, set_is_resizing) = signal(false);

    // Resize range is only consumed inside the hydrate-only drag handler;
    // bind them to `_` on SSR so the prop stays in the public API without
    // triggering unused-variable warnings on server builds.
    #[cfg(not(feature = "hydrate"))]
    let _ = (min_width, max_width, &width, &set_is_resizing);

    // ── Close handler ─────────────────────────────────────────────────────
    let close_label_for_btn = close_label.clone();
    let handle_close = move || on_close.run(());

    // ── Escape-to-close ───────────────────────────────────────────────────
    #[cfg(feature = "hydrate")]
    {
        use send_wrapper::SendWrapper;
        use wasm_bindgen::closure::Closure;
        let escape_cleanup: CleanupSlot = StoredValue::new(None);

        Effect::new(move |_| {
            // Re-bind the listener whenever `open` flips so we don't leak.
            let is_open = open.get();
            // Clear any previous handler before (re-)registering.
            if let Some(teardown) = escape_cleanup.try_update_value(|v| v.take()).flatten() {
                teardown.take()();
            }
            if !is_open {
                return;
            }
            let Some(window) = web_sys::window() else {
                return;
            };
            let Some(document) = window.document() else {
                return;
            };

            let handler = Closure::<dyn FnMut(web_sys::KeyboardEvent)>::new(
                move |ev: web_sys::KeyboardEvent| {
                    if ev.key() == "Escape" {
                        on_close.run(());
                    }
                },
            );
            let _ = document.add_event_listener_with_callback(
                "keydown",
                handler.as_ref().unchecked_ref(),
            );
            let document_for_teardown = document.clone();
            let handler_ref = handler
                .as_ref()
                .unchecked_ref::<js_sys::Function>()
                .clone();
            // Keep handler alive via teardown box.
            let handler_holder = std::cell::RefCell::new(Some(handler));
            let teardown: Box<dyn FnOnce()> = Box::new(move || {
                let _ = document_for_teardown
                    .remove_event_listener_with_callback("keydown", &handler_ref);
                handler_holder.borrow_mut().take();
            });
            escape_cleanup.set_value(Some(SendWrapper::new(teardown)));
        });

        on_cleanup(move || {
            if let Some(teardown) = escape_cleanup.try_update_value(|v| v.take()).flatten() {
                teardown.take()();
            }
        });
    }

    // ── Resize drag (desktop) ─────────────────────────────────────────────
    #[cfg(feature = "hydrate")]
    let drag_cleanup: CleanupSlot = StoredValue::new(None);

    let handle_resize_start = move |ev: web_sys::MouseEvent| {
        ev.prevent_default();
        set_is_resizing.set(true);

        #[cfg(feature = "hydrate")]
        {
            use std::cell::RefCell;
            use std::rc::Rc;
            use wasm_bindgen::closure::Closure;

            let start_x = ev.client_x() as f64;
            let start_w = width.get_untracked();
            let min_w = min_width;
            let max_w = max_width;

            let Some(window) = web_sys::window() else {
                return;
            };
            let Some(document) = window.document() else {
                return;
            };

            let move_handler = Closure::<dyn FnMut(web_sys::MouseEvent)>::new(
                move |ev: web_sys::MouseEvent| {
                    let diff = start_x - ev.client_x() as f64;
                    let new_width = (start_w + diff).clamp(min_w, max_w);
                    width.set(new_width);
                },
            );

            let move_ref = move_handler
                .as_ref()
                .unchecked_ref::<js_sys::Function>()
                .clone();
            let document_for_up = document.clone();
            let move_fn_for_up = move_ref.clone();

            let closures: DragClosures = Rc::new(RefCell::new(None));
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
            let _ = document.add_event_listener_with_callback(
                "mouseup",
                up_handler.as_ref().unchecked_ref(),
            );

            *closures.borrow_mut() = Some((move_handler, up_handler));

            let closures_for_teardown = closures;
            let document_for_teardown = document.clone();
            let move_ref_for_teardown = move_ref.clone();
            let teardown: Box<dyn FnOnce()> = Box::new(move || {
                if let Some((_, ref up_cb)) = *closures_for_teardown.borrow() {
                    let _ = document_for_teardown.remove_event_listener_with_callback(
                        "mousemove",
                        &move_ref_for_teardown,
                    );
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

    // ── Header content ────────────────────────────────────────────────────
    // Either a custom ChildrenFn or the default simple header (§ + title).
    let header_fn = header.clone();
    let title_clone = title.clone();
    let simple_title_view = move || {
        title_clone.clone().map(|t| view! {
            <div class="flex items-center gap-2 min-w-0">
                <span class="font-display italic text-primary text-[22px] leading-none shrink-0 translate-y-[1px]">
                    "§"
                </span>
                <span class="font-display text-[20px] leading-none text-foreground truncate">
                    {t}
                </span>
            </div>
        })
    };

    let render_header = move || {
        if let Some(ref h) = header_fn {
            view! { <div class="flex-1 min-w-0">{h()}</div> }.into_any()
        } else {
            view! { <div class="flex-1 min-w-0">{simple_title_view()}</div> }.into_any()
        }
    };

    // ── Footer content ────────────────────────────────────────────────────
    let footer_fn = footer.clone();
    let render_footer = move || {
        footer_fn.as_ref().map(|f| {
            view! {
                <div class="border-t border-border flex-shrink-0">
                    {f()}
                </div>
            }
        })
    };

    // ── Panel inner (shared across desktop + mobile) ──────────────────────
    let close_label_inner = close_label_for_btn.clone();
    let children_fn = children;
    let panel_inner = move || {
        let close_label = close_label_inner.clone();
        view! {
            <div class="flex flex-col h-full min-w-0 flex-1">
                // Header — h-16 to match page-header
                <div class="h-16 px-4 md:px-5 flex items-center gap-2 flex-shrink-0">
                    {render_header()}
                    <Button
                        variant=ButtonVariant::GhostMuted
                        size=ButtonSize::IconSm
                        aria_label=close_label.clone()
                        on:click=move |_| handle_close()
                    >
                        <Icon icon=phosphor_leptos::X size="16px" weight=IconWeight::Regular />
                    </Button>
                </div>
                // Body — flex-1 scroll region
                <div class="flex-1 overflow-y-auto min-h-0">
                    {children_fn()}
                </div>
                {render_footer()}
            </div>
        }
    };

    // ── Render ─────────────────────────────────────────────────────────────
    view! {
        {move || {
            let is_open = open.get();
            if is_mobile.get() {
                // ── Mobile: fixed overlay with backdrop ───────────────────
                // Always mounted; transform + opacity animate in/out so CSS
                // transitions work. bg-muted + shadow-lg restore the sheet
                // signal on mobile (see DESIGN.md Right Panel Pattern, Mobile).
                let backdrop_class = if is_open {
                    "fixed inset-0 z-40 bg-[var(--color-overlay)] transition-opacity duration-300 ease-in-out opacity-100"
                } else {
                    "fixed inset-0 z-40 bg-[var(--color-overlay)] transition-opacity duration-300 ease-in-out opacity-0 pointer-events-none"
                };
                let panel_class = if is_open {
                    "fixed top-0 right-0 bottom-0 w-[min(400px,85vw)] z-50 bg-muted shadow-lg flex flex-col transition-transform duration-300 ease-in-out translate-x-0"
                } else {
                    "fixed top-0 right-0 bottom-0 w-[min(400px,85vw)] z-50 bg-muted shadow-lg flex flex-col transition-transform duration-300 ease-in-out translate-x-full"
                };
                view! {
                    <>
                        <div
                            class=backdrop_class
                            on:click=move |_| handle_close()
                        />
                        <div class=panel_class>
                            {panel_inner()}
                        </div>
                    </>
                }.into_any()
            } else {
                // ── Desktop: inline resizable slab ────────────────────────
                // Always mounted; width animates 0 ↔ target so transitions work.
                // No border-t — Editorial Margin: panel is part of the same
                // continuous warm surface, separated only by border-l.
                let target_w = width.get();
                let style = if is_open {
                    format!("width: {}px", target_w)
                } else {
                    "width: 0px".to_string()
                };
                let outer_class = if is_resizing.get() {
                    "relative border-l border-border bg-background flex h-full overflow-hidden select-none flex-shrink-0"
                } else {
                    "relative border-l border-border bg-background flex h-full overflow-hidden transition-[width] duration-300 ease-in-out flex-shrink-0"
                };
                view! {
                    <aside class=outer_class style=style>
                        // Resize handle — 4px invisible hit-zone on the left edge.
                        // Only enabled when the panel is open.
                        {move || open.get().then(|| view! {
                            <div
                                class="absolute top-0 bottom-0 left-0 w-1 cursor-ew-resize z-10 hover:bg-border-strong transition-colors"
                                on:mousedown=handle_resize_start
                                aria-hidden="true"
                            />
                        })}
                        {panel_inner()}
                    </aside>
                }.into_any()
            }
        }}
    }
}
