// SPDX-License-Identifier: AGPL-3.0-or-later

//! Top-of-page navigation progress bar (NProgress-style).
//!
//! Shows a thin animated amber bar at the top of the viewport while a route
//! transition is in progress. Controlled by the `is_routing` signal from
//! `<Router set_is_routing>`.
//!
//! Behavior:
//! - `is_routing=true`: bar appears and animates from 0% → 90% width (ease-out)
//! - `is_routing=false`: bar snaps to 100% then fades out
//! - First load (SSR): not visible (no route transition happening)

use leptos::prelude::*;

/// A thin animated progress bar shown during route transitions.
#[component]
pub fn NavigationProgress(
    /// Whether a route transition is currently in progress.
    is_routing: ReadSignal<bool>,
) -> impl IntoView {
    view! {
        <div
            class="fixed top-0 left-0 right-0 z-50 pointer-events-none"
            style:height="2px"
        >
            <div
                class=move || {
                    let routing = is_routing.get();
                    let mut classes = String::from("h-full bg-primary ");
                    if routing {
                        // Animating: grow from 0% to 90% over 2s
                        classes.push_str("transition-all duration-[2000ms] ease-out w-[90%] opacity-100");
                    } else {
                        // Done: snap to 100% then fade out
                        classes.push_str("transition-all duration-300 ease-in w-full opacity-0");
                    }
                    classes
                }
            />
        </div>
    }
}
