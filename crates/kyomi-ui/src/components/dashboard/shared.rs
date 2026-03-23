// SPDX-License-Identifier: AGPL-3.0-or-later

//! Shared utilities for dashboard components.
//!
//! Consolidates button class constants, date formatting, SVG icon helpers,
//! the `DashboardListEntry` component, and the `use_is_mobile` reactive hook
//! used by sidebar/panel components.

use leptos::prelude::*;
#[cfg(feature = "hydrate")]
use wasm_bindgen::prelude::*;

use crate::server_fns::dashboards::DashboardListItem;

// ─── Mobile detection ───────────────────────────────────────────────────────

/// Breakpoint (in CSS pixels) below which the viewport is considered mobile.
#[cfg(target_arch = "wasm32")]
pub(crate) const MOBILE_BREAKPOINT: f64 = 768.0;

/// Returns a reactive signal tracking whether the viewport is mobile-sized.
///
/// Checks the initial viewport width and listens for `resize` events.
/// Used by `history_panel`, `copilot_sidebar`, and any future panel that
/// needs responsive desktop/mobile layout switching.
pub(crate) fn use_is_mobile() -> Signal<bool> {
    let (is_mobile, set_is_mobile) = signal(false);
    #[cfg(not(feature = "hydrate"))]
    let _ = set_is_mobile;

    Effect::new(move || {
        #[cfg(feature = "hydrate")]
        if let Some(window) = web_sys::window() {
            let width = window
                .inner_width()
                .ok()
                .and_then(|v| v.as_f64())
                .unwrap_or(1024.0);
            set_is_mobile.set(width < MOBILE_BREAKPOINT);
        }
    });

    #[cfg(feature = "hydrate")]
    {
        use send_wrapper::SendWrapper;
        use wasm_bindgen::closure::Closure;

        let handler = Closure::<dyn Fn()>::new(move || {
            if let Some(window) = web_sys::window() {
                let width = window
                    .inner_width()
                    .ok()
                    .and_then(|v| v.as_f64())
                    .unwrap_or(1024.0);
                set_is_mobile.set(width < MOBILE_BREAKPOINT);
            }
        });

        if let Some(window) = web_sys::window() {
            let _ = window
                .add_event_listener_with_callback("resize", handler.as_ref().unchecked_ref());
            let handler_ref = SendWrapper::new(
                handler.as_ref().unchecked_ref::<js_sys::Function>().clone(),
            );
            let window = SendWrapper::new(window);
            let handler_wrapper = SendWrapper::new(handler);
            on_cleanup(move || {
                let _ = window
                    .take()
                    .remove_event_listener_with_callback("resize", &handler_ref.take());
                drop(handler_wrapper);
            });
        }
    }

    is_mobile.into()
}

// ─── Button class constants ─────────────────────────────────────────────────

/// Button base classes — copied from `button.rs` BASE constant.
pub(crate) const BTN_BASE: &str = "inline-flex items-center justify-center gap-2 whitespace-nowrap rounded-md text-sm font-medium transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:pointer-events-none disabled:opacity-50 [&_svg]:pointer-events-none [&_svg]:size-4 [&_svg]:shrink-0";

/// Default (primary) button variant classes.
pub(crate) const BTN_DEFAULT: &str =
    "bg-primary text-primary-foreground shadow hover:bg-primary/90";

/// Outline button variant classes.
pub(crate) const BTN_OUTLINE: &str = "border border-input bg-background text-foreground shadow-sm hover:bg-accent hover:text-accent-foreground";

/// Default button size classes.
pub(crate) const BTN_SIZE: &str = "h-9 px-4 py-2";

// ─── Date formatting ────────────────────────────────────────────────────────

/// Format an ISO 8601 date string into a human-readable relative/short date.
///
/// Matches the React `formatDate` function used in both
/// `SaveDashboardModal.jsx` and `InsertDashboardLinkModal.jsx`:
/// "Today", "Yesterday", "N days ago", then short date format.
pub(crate) fn format_date(iso: &str) -> String {
    let Ok(dt) = chrono::DateTime::parse_from_rfc3339(iso) else {
        return iso.to_string();
    };
    let now = chrono::Utc::now();
    let diff = now.signed_duration_since(dt);
    let days = diff.num_days();

    if days < 1 {
        return "Today".to_string();
    }
    if days == 1 {
        return "Yesterday".to_string();
    }
    if days < 7 {
        return format!("{days} days ago");
    }

    // Short date: "Mar 22" or "Mar 22, 2025" if different year
    let dt_utc = dt.with_timezone(&chrono::Utc);
    let now_year = now.format("%Y").to_string();
    let dt_year = dt_utc.format("%Y").to_string();

    if dt_year == now_year {
        dt_utc.format("%b %-d").to_string()
    } else {
        dt_utc.format("%b %-d, %Y").to_string()
    }
}

// ─── SVG Icons ──────────────────────────────────────────────────────────────

/// Dashboard icon — grid/layout SVG path.
pub(crate) fn dashboard_icon(class: &str) -> impl IntoView {
    let class = class.to_string();
    view! {
        <svg class=class fill="none" stroke="currentColor" viewBox="0 0 24 24">
            <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M9 17V7m0 10a2 2 0 01-2 2H5a2 2 0 01-2-2V7a2 2 0 012-2h2a2 2 0 012 2m0 10a2 2 0 002 2h2a2 2 0 002-2M9 7a2 2 0 012-2h2a2 2 0 012 2m0 10V7m0 10a2 2 0 002 2h2a2 2 0 002-2V7a2 2 0 00-2-2h-2a2 2 0 00-2 2" />
        </svg>
    }
}

/// Checkmark circle icon — filled circle with checkmark, viewBox="0 0 20 20".
pub(crate) fn check_circle_icon(class: &str) -> impl IntoView {
    let class = class.to_string();
    view! {
        <svg class=class fill="currentColor" viewBox="0 0 20 20">
            <path fill-rule="evenodd" d="M10 18a8 8 0 100-16 8 8 0 000 16zm3.707-9.293a1 1 0 00-1.414-1.414L9 10.586 7.707 9.293a1 1 0 00-1.414 1.414l2 2a1 1 0 001.414 0l4-4z" clip-rule="evenodd" />
        </svg>
    }
}

// ─── DashboardListEntry component ───────────────────────────────────────────

/// A single dashboard entry in a scrollable list.
///
/// Used by both `SaveDashboardModal` (existing dashboards list) and
/// `InsertDashboardLinkModal` (dashboard selection list).
#[component]
pub(crate) fn DashboardListEntry(
    dashboard: DashboardListItem,
    #[prop(into)]
    selected_dashboard_id: Signal<Option<String>>,
    /// Called with the dashboard_id when clicked
    on_select: impl Fn(String) + 'static + Clone,
) -> impl IntoView {
    let id = dashboard.dashboard_id.clone();
    let id_for_click = id.clone();
    let id_for_class = id.clone();
    let id_for_icon_class = id.clone();
    let id_for_icon_text = id.clone();
    let id_for_check = id.clone();
    let title = dashboard.title.clone();
    let created_at = format_date(&dashboard.created_at);
    let on_select_clone = on_select.clone();

    view! {
        <div
            on:click=move |_| on_select_clone(id_for_click.clone())
            class=move || {
                let selected = selected_dashboard_id.get().as_deref() == Some(&*id_for_class);
                if selected {
                    "border-2 rounded-lg p-4 cursor-pointer transition-all border-primary bg-primary/10"
                } else {
                    "border-2 rounded-lg p-4 cursor-pointer transition-all border-border hover:border-input hover:bg-accent"
                }
            }
        >
            <div class="flex items-center gap-3">
                // Icon container
                <div class=move || {
                    let selected = selected_dashboard_id.get().as_deref() == Some(&*id_for_icon_class);
                    if selected {
                        "flex-shrink-0 w-10 h-10 rounded-lg flex items-center justify-center bg-primary"
                    } else {
                        "flex-shrink-0 w-10 h-10 rounded-lg flex items-center justify-center bg-accent"
                    }
                }>
                    {move || {
                        let selected = selected_dashboard_id.get().as_deref() == Some(&*id_for_icon_text);
                        let class = if selected {
                            "w-5 h-5 text-white"
                        } else {
                            "w-5 h-5 text-muted-foreground"
                        };
                        dashboard_icon(class)
                    }}
                </div>

                // Title + date
                <div class="flex-1 min-w-0">
                    <h3 class="text-base font-medium text-foreground truncate">
                        {if title.is_empty() { "Untitled Dashboard".to_string() } else { title.clone() }}
                    </h3>
                    <p class="text-sm text-muted-foreground mt-0.5">
                        {created_at.clone()}
                    </p>
                </div>

                // Checkmark when selected
                {move || {
                    let selected = selected_dashboard_id.get().as_deref() == Some(&*id_for_check);
                    if selected {
                        Some(check_circle_icon("w-5 h-5 text-primary flex-shrink-0"))
                    } else {
                        None
                    }
                }}
            </div>
        </div>
    }
}
