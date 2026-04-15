// SPDX-License-Identifier: AGPL-3.0-or-later

//! Pure-Rust popover positioning helper.
//!
//! Implements the subset of Floating UI / Radix Popper we need to escape
//! `overflow: hidden` ancestors and viewport edges when rendering dropdown
//! menus. Radix/shadcn's `Select`, `DropdownMenu`, `Popover` etc. all use
//! this pattern under the hood via `@floating-ui/react-dom`:
//!
//! 1. **Portal** the content to `document.body` — escapes any
//!    `overflow: hidden` ancestor and avoids z-index stacking context
//!    issues entirely.
//! 2. **Measure** the trigger's bounding client rect + the viewport.
//! 3. **Compute** a position: start with preferred placement, flip to the
//!    opposite side if the primary side overflows the viewport, shift
//!    along the cross-axis if the content overflows the viewport edge,
//!    cap `max-height` if still short.
//! 4. **Reposition** on scroll/resize via `autoUpdate`.
//!
//! This module owns the pure-math part (`compute_position`). The
//! `<Popover>` component that glues it to Leptos lives alongside it.

use leptos::prelude::*;

// ─────────────────────────────────────────────────────────────────────────────
// Geometry — pure types, no DOM dependency, fully unit-testable
// ─────────────────────────────────────────────────────────────────────────────

/// A rectangle in viewport coordinates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub top: f64,
    pub left: f64,
    pub width: f64,
    pub height: f64,
}

impl Rect {
    pub fn bottom(&self) -> f64 { self.top + self.height }
    pub fn right(&self) -> f64 { self.left + self.width }
}

/// Preferred side to anchor the popover on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Bottom,
    Top,
}

/// Cross-axis alignment relative to the trigger.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Align {
    Start,
    End,
}

/// Combined placement. `BottomStart` = open below, align left edge to
/// trigger's left edge (Radix default for `DropdownMenu`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Placement {
    pub side: Side,
    pub align: Align,
}

impl Placement {
    pub const BOTTOM_START: Self = Self { side: Side::Bottom, align: Align::Start };
    pub const BOTTOM_END: Self = Self { side: Side::Bottom, align: Align::End };
    pub const TOP_START: Self = Self { side: Side::Top, align: Align::Start };
    pub const TOP_END: Self = Self { side: Side::Top, align: Align::End };
}

/// Computed position — absolute viewport coordinates plus an optional
/// max-height cap the caller should apply to the content element.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PopoverPosition {
    pub top: f64,
    pub left: f64,
    pub max_height: f64,
    pub placement: Placement,
}

/// 4px gap between the trigger and the popover, matching Radix's default
/// `sideOffset`.
const SIDE_OFFSET: f64 = 4.0;

/// Padding from the viewport edges so the popover never touches them.
const VIEWPORT_PADDING: f64 = 8.0;

/// Compute the popover position given trigger + content + viewport.
///
/// - `trigger` — the triggering element's `getBoundingClientRect()`.
/// - `content_size` — the content's intrinsic `(width, height)` measured
///   while it is temporarily invisible or offscreen.
/// - `viewport_width` / `viewport_height` — window dimensions.
/// - `preferred` — desired placement; the function may flip the side if
///   there isn't enough room.
///
/// Returns viewport-relative `top` / `left` (for `position: fixed`) plus
/// the `max_height` to apply so the popover never extends past the
/// viewport edge.
pub fn compute_position(
    trigger: Rect,
    content_size: (f64, f64),
    viewport_width: f64,
    viewport_height: f64,
    preferred: Placement,
) -> PopoverPosition {
    let (content_w, content_h) = content_size;

    // ── Side decision: flip if the preferred side doesn't have room ──
    let space_below = viewport_height - trigger.bottom() - SIDE_OFFSET - VIEWPORT_PADDING;
    let space_above = trigger.top - SIDE_OFFSET - VIEWPORT_PADDING;

    let side = match preferred.side {
        Side::Bottom => {
            if space_below >= content_h || space_below >= space_above {
                Side::Bottom
            } else {
                Side::Top
            }
        }
        Side::Top => {
            if space_above >= content_h || space_above >= space_below {
                Side::Top
            } else {
                Side::Bottom
            }
        }
    };

    // ── Max height: cap to available space on the chosen side ──
    let available_height = match side {
        Side::Bottom => space_below,
        Side::Top => space_above,
    };
    let max_height = content_h.min(available_height.max(0.0));

    // ── Top coordinate based on side ──
    let top = match side {
        Side::Bottom => trigger.bottom() + SIDE_OFFSET,
        Side::Top => trigger.top - SIDE_OFFSET - max_height,
    };

    // ── Cross-axis: start from preferred align, shift to stay in viewport ──
    let desired_left = match preferred.align {
        Align::Start => trigger.left,
        Align::End => trigger.right() - content_w,
    };

    // Shift to keep the popover inside the viewport horizontally.
    let min_left = VIEWPORT_PADDING;
    let max_left = viewport_width - content_w - VIEWPORT_PADDING;
    let left = if max_left < min_left {
        // Content wider than viewport — just pin to left edge with padding
        min_left
    } else {
        desired_left.clamp(min_left, max_left)
    };

    let final_placement = Placement { side, align: preferred.align };
    PopoverPosition { top, left, max_height, placement: final_placement }
}

// ─────────────────────────────────────────────────────────────────────────────
// Leptos component
// ─────────────────────────────────────────────────────────────────────────────

/// Portalled popover anchored to a trigger element.
///
/// Renders `children` as a direct child of `document.body` when `open` is
/// `true`. Positions them against the trigger's bounding client rect using
/// `compute_position`. Reposition on window scroll/resize while open.
/// Handles outside-click and Escape internally — consumer just provides
/// `on_close`.
///
/// The caller is responsible for:
/// - Providing a `trigger_ref: NodeRef<leptos::html::Div>` that wraps (or is)
///   the clickable trigger element.
/// - Toggling the `open` signal on trigger click.
/// - Providing an `on_close` callback that sets `open` to false.
#[component]
pub fn Popover(
    /// Ref on the trigger element — its bounding rect drives positioning,
    /// and clicks inside it are excluded from the outside-click handler.
    trigger_ref: NodeRef<leptos::html::Div>,
    /// Open state. When false, the portal renders nothing.
    #[prop(into)]
    open: Signal<bool>,
    /// Called when the user clicks outside the popover or presses Escape.
    /// Consumer should set their open signal to false.
    on_close: Callback<()>,
    /// Preferred placement. Defaults to `BottomStart`.
    #[prop(default = Placement::BOTTOM_START)]
    placement: Placement,
    /// When `true`, the popover's `min-width` is set to match the trigger's
    /// rendered width. Used by Select-style dropdowns where the options
    /// list should visually extend the trigger.
    #[prop(default = false)]
    match_width: bool,
    /// Additional classes merged into the popover content wrapper.
    #[prop(into, default = String::new())]
    class: String,
    children: ChildrenFn,
) -> impl IntoView {
    let content_ref = NodeRef::<leptos::html::Div>::new();
    let (position, set_position) = signal(None::<PopoverPosition>);
    let (trigger_width, set_trigger_width) = signal(0.0_f64);

    // Recompute position whenever `open` flips true, the window scrolls,
    // or the window resizes. Native builds never reach this block — SSR
    // renders the portal with an off-screen fallback position, then the
    // wasm effect repositions after hydration. Bind the dependent inputs
    // here so rustc sees them as used on both targets.
    #[cfg(not(target_arch = "wasm32"))]
    let _ = (trigger_ref, placement, set_position, set_trigger_width, on_close, match_width);

    #[cfg(target_arch = "wasm32")]
    {
        use wasm_bindgen::prelude::*;
        use wasm_bindgen::JsCast;

        let reposition = move || {
            if !open.get_untracked() {
                set_position.set(None);
                return;
            }
            let Some(trigger_el) = trigger_ref.get_untracked() else { return };
            let Some(content_el) = content_ref.get_untracked() else { return };
            let Some(window) = web_sys::window() else { return };

            let trigger_rect = trigger_el.get_bounding_client_rect();
            set_trigger_width.set(trigger_rect.width());
            // Measure content natural size by reading scrollWidth/scrollHeight —
            // these are the intrinsic dimensions including overflow, which
            // matches Floating UI's measurement strategy.
            let content_w = content_el.scroll_width() as f64;
            let content_h = content_el.scroll_height() as f64;

            let vw = window.inner_width().ok().and_then(|v| v.as_f64()).unwrap_or(0.0);
            let vh = window.inner_height().ok().and_then(|v| v.as_f64()).unwrap_or(0.0);

            let pos = compute_position(
                Rect {
                    top: trigger_rect.top(),
                    left: trigger_rect.left(),
                    width: trigger_rect.width(),
                    height: trigger_rect.height(),
                },
                (content_w, content_h),
                vw,
                vh,
                placement,
            );
            set_position.set(Some(pos));
        };

        // Recompute when open changes or on next frame after render.
        Effect::new({
            let reposition = reposition.clone();
            move |_| {
                if open.get() {
                    // Use request_animation_frame so the content element
                    // has been mounted and measured before we read its
                    // dimensions.
                    let reposition = reposition.clone();
                    let window = web_sys::window().unwrap();
                    let cb = Closure::once_into_js(move || reposition());
                    let _ = window.request_animation_frame(cb.unchecked_ref());
                } else {
                    set_position.set(None);
                }
            }
        });

        // Scroll/resize listeners — active only while open.
        Effect::new({
            let reposition = reposition.clone();
            move |_| {
                if !open.get() {
                    return;
                }
                let Some(window) = web_sys::window() else { return };
                let reposition = reposition.clone();
                let cb = Closure::<dyn FnMut()>::new(move || reposition());
                let _ = window.add_event_listener_with_callback(
                    "scroll",
                    cb.as_ref().unchecked_ref(),
                );
                let _ = window.add_event_listener_with_callback(
                    "resize",
                    cb.as_ref().unchecked_ref(),
                );
                let cb_for_cleanup = send_wrapper::SendWrapper::new(cb);
                on_cleanup(move || {
                    let Some(window) = web_sys::window() else { return };
                    let cb = cb_for_cleanup.take();
                    let _ = window.remove_event_listener_with_callback(
                        "scroll",
                        cb.as_ref().unchecked_ref(),
                    );
                    let _ = window.remove_event_listener_with_callback(
                        "resize",
                        cb.as_ref().unchecked_ref(),
                    );
                });
            }
        });

        // Outside-click + Escape — active only while open. Handlers stored
        // in RefCells so `on_cleanup` can remove the listeners and drop the
        // closures cleanly (no `.forget()`). Matches the existing pattern
        // in `chart_header_bar.rs` and `copilot_sidebar.rs`.
        type KeyHandlerCell = send_wrapper::SendWrapper<
            std::rc::Rc<std::cell::RefCell<Option<(Closure<dyn Fn(web_sys::KeyboardEvent)>, web_sys::Window)>>>,
        >;
        type ClickHandlerCell = send_wrapper::SendWrapper<
            std::rc::Rc<std::cell::RefCell<Option<(Closure<dyn Fn(web_sys::MouseEvent)>, web_sys::Window)>>>,
        >;
        type TimeoutCell = send_wrapper::SendWrapper<
            std::rc::Rc<std::cell::RefCell<Option<i32>>>,
        >;

        let esc_cell: KeyHandlerCell =
            send_wrapper::SendWrapper::new(std::rc::Rc::new(std::cell::RefCell::new(None)));
        let click_cell: ClickHandlerCell =
            send_wrapper::SendWrapper::new(std::rc::Rc::new(std::cell::RefCell::new(None)));
        let timeout_cell: TimeoutCell =
            send_wrapper::SendWrapper::new(std::rc::Rc::new(std::cell::RefCell::new(None)));

        let esc_cell_effect = esc_cell.clone();
        let click_cell_effect = click_cell.clone();
        let timeout_cell_effect = timeout_cell.clone();
        Effect::new(move |_| {
            // When closing, tear down any active listeners.
            if !open.get() {
                if let Some(tid) = timeout_cell_effect.borrow_mut().take() {
                    if let Some(w) = web_sys::window() {
                        w.clear_timeout_with_handle(tid);
                    }
                }
                if let Some((cb, win)) = esc_cell_effect.borrow_mut().take() {
                    let _ = win.remove_event_listener_with_callback(
                        "keydown",
                        cb.as_ref().unchecked_ref(),
                    );
                }
                if let Some((cb, win)) = click_cell_effect.borrow_mut().take() {
                    let _ = win.remove_event_listener_with_callback(
                        "click",
                        cb.as_ref().unchecked_ref(),
                    );
                }
                return;
            }

            let Some(window) = web_sys::window() else { return };

            // Escape key closes immediately.
            let esc_cb = Closure::<dyn Fn(web_sys::KeyboardEvent)>::new(
                move |e: web_sys::KeyboardEvent| {
                    if e.key() == "Escape" {
                        on_close.run(());
                    }
                },
            );
            let _ = window.add_event_listener_with_callback(
                "keydown",
                esc_cb.as_ref().unchecked_ref(),
            );
            *esc_cell_effect.borrow_mut() = Some((esc_cb, window.clone()));

            // Click-outside — attached on a zero-delay timeout so the click
            // that opened the popover doesn't immediately close it.
            let click_cell_cb = click_cell_effect.clone();
            let win_cb = window.clone();
            let cb_setup = Closure::once_into_js(move || {
                let click_cb = Closure::<dyn Fn(web_sys::MouseEvent)>::new(
                    move |e: web_sys::MouseEvent| {
                        let Some(target) = e
                            .target()
                            .and_then(|t| t.dyn_into::<web_sys::Node>().ok())
                        else {
                            return;
                        };
                        let in_trigger = trigger_ref
                            .get_untracked()
                            .is_some_and(|el| el.contains(Some(&target)));
                        let in_content = content_ref
                            .get_untracked()
                            .is_some_and(|el| el.contains(Some(&target)));
                        if !in_trigger && !in_content {
                            on_close.run(());
                        }
                    },
                );
                let _ = win_cb.add_event_listener_with_callback(
                    "click",
                    click_cb.as_ref().unchecked_ref(),
                );
                *click_cell_cb.borrow_mut() = Some((click_cb, win_cb));
            });
            let tid = window
                .set_timeout_with_callback_and_timeout_and_arguments_0(
                    cb_setup.as_ref().unchecked_ref(),
                    0,
                )
                .unwrap_or(0);
            *timeout_cell_effect.borrow_mut() = Some(tid);
        });

        // Final cleanup on component dispose.
        let esc_cell_drop = esc_cell;
        let click_cell_drop = click_cell;
        let timeout_cell_drop = timeout_cell;
        on_cleanup(move || {
            if let Some(tid) = timeout_cell_drop.borrow_mut().take() {
                if let Some(w) = web_sys::window() {
                    w.clear_timeout_with_handle(tid);
                }
            }
            if let Some((cb, win)) = esc_cell_drop.borrow_mut().take() {
                let _ = win.remove_event_listener_with_callback(
                    "keydown",
                    cb.as_ref().unchecked_ref(),
                );
            }
            if let Some((cb, win)) = click_cell_drop.borrow_mut().take() {
                let _ = win.remove_event_listener_with_callback(
                    "click",
                    cb.as_ref().unchecked_ref(),
                );
            }
        });
    }

    // Style string for the portalled content wrapper. Starts hidden with
    // visibility:hidden so the browser can measure it before we position —
    // that's how Floating UI handles "measure without flash".
    let style = move || {
        match position.get() {
            Some(pos) => {
                let width_rule = if match_width {
                    format!("min-width: {}px;", trigger_width.get())
                } else {
                    String::new()
                };
                format!(
                    "position: fixed; top: {}px; left: {}px; max-height: {}px; z-index: 2147483646; {}",
                    pos.top, pos.left, pos.max_height, width_rule
                )
            }
            None => {
                // Not yet positioned — render invisibly off-screen so we
                // can measure without showing a flash at (0, 0).
                "position: fixed; top: -9999px; left: -9999px; visibility: hidden; z-index: 2147483646;".to_string()
            }
        }
    };

    let class = format!("kyomi-popover {}", class);
    let children_stored = StoredValue::new(children);

    view! {
        <Show when=move || open.get()>
            {
                let class = class.clone();
                let style = style;
                view! {
                    <leptos::portal::Portal>
                        <div node_ref=content_ref class=class.clone() style=style>
                            {children_stored.with_value(|c| c())}
                        </div>
                    </leptos::portal::Portal>
                }
            }
        </Show>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn trigger_at(top: f64, left: f64, width: f64, height: f64) -> Rect {
        Rect { top, left, width, height }
    }

    #[test]
    fn bottom_start_fits_below_no_flip() {
        // Trigger at top of screen, plenty of room below
        let trigger = trigger_at(100.0, 200.0, 120.0, 32.0);
        let pos = compute_position(
            trigger,
            (160.0, 200.0),
            1280.0,
            720.0,
            Placement::BOTTOM_START,
        );
        assert_eq!(pos.placement.side, Side::Bottom);
        assert_eq!(pos.top, 136.0); // trigger.bottom() + 4 offset
        assert_eq!(pos.left, 200.0); // aligned to trigger.left
        assert_eq!(pos.max_height, 200.0); // fits fully
    }

    #[test]
    fn bottom_start_flips_when_no_room_below() {
        // Trigger near bottom of screen — not enough room below for 300px
        // content. Should flip to top.
        let trigger = trigger_at(600.0, 200.0, 120.0, 32.0);
        let pos = compute_position(
            trigger,
            (160.0, 300.0),
            1280.0,
            720.0,
            Placement::BOTTOM_START,
        );
        assert_eq!(pos.placement.side, Side::Top);
        // top = trigger.top - 4 offset - max_height
        // max_height = min(300, 600 - 4 - 8) = 300
        // Actually: space_above = 600 - 4 - 8 = 588, max_height = min(300, 588) = 300
        // top = 600 - 4 - 300 = 296
        assert_eq!(pos.top, 296.0);
        assert_eq!(pos.max_height, 300.0);
    }

    #[test]
    fn caps_max_height_when_neither_side_has_room() {
        // Small viewport, trigger in the middle — neither side fits 400px
        // content, so cap to the larger side.
        let trigger = trigger_at(200.0, 100.0, 120.0, 32.0);
        let pos = compute_position(
            trigger,
            (160.0, 400.0),
            1280.0,
            500.0,
            Placement::BOTTOM_START,
        );
        // space_below = 500 - 232 - 4 - 8 = 256
        // space_above = 200 - 4 - 8 = 188
        // Bottom wins (larger than above), max_height = min(400, 256) = 256
        assert_eq!(pos.placement.side, Side::Bottom);
        assert_eq!(pos.max_height, 256.0);
        assert_eq!(pos.top, 236.0);
    }

    #[test]
    fn shifts_left_when_content_overflows_right_edge() {
        // Trigger near right edge, content wider than remaining space
        let trigger = trigger_at(100.0, 1200.0, 80.0, 32.0);
        let pos = compute_position(
            trigger,
            (200.0, 160.0),
            1280.0,
            720.0,
            Placement::BOTTOM_START,
        );
        // desired_left = 1200, but max_left = 1280 - 200 - 8 = 1072
        // clamped to 1072
        assert_eq!(pos.left, 1072.0);
    }

    #[test]
    fn bottom_end_aligns_right_edges() {
        let trigger = trigger_at(100.0, 500.0, 120.0, 32.0);
        let pos = compute_position(
            trigger,
            (200.0, 150.0),
            1280.0,
            720.0,
            Placement::BOTTOM_END,
        );
        // desired_left = trigger.right() - content_w = 620 - 200 = 420
        assert_eq!(pos.left, 420.0);
    }

    #[test]
    fn pins_to_viewport_padding_when_content_wider_than_viewport() {
        let trigger = trigger_at(100.0, 50.0, 80.0, 32.0);
        let pos = compute_position(
            trigger,
            (2000.0, 160.0),
            1280.0,
            720.0,
            Placement::BOTTOM_START,
        );
        // Content wider than viewport — pinned to min_left (padding)
        assert_eq!(pos.left, VIEWPORT_PADDING);
    }

    #[test]
    fn flips_top_preferred_to_bottom_when_no_room_above() {
        let trigger = trigger_at(50.0, 200.0, 120.0, 32.0);
        let pos = compute_position(
            trigger,
            (160.0, 200.0),
            1280.0,
            720.0,
            Placement::TOP_START,
        );
        // space_above = 50 - 12 = 38, not enough for 200px content
        // space_below = 720 - 82 - 12 = 626
        // Below is larger → flip to bottom
        assert_eq!(pos.placement.side, Side::Bottom);
    }
}
