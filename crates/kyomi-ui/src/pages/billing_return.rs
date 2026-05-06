// SPDX-License-Identifier: AGPL-3.0-or-later

//! Intermediate landing page for Stripe Billing Portal return.
//!
//! The Billing Portal is hosted by Stripe — when the user clicks "Done",
//! Stripe redirects back to our domain. Because cookies are SameSite=Strict,
//! they aren't sent on this cross-site navigation. This page acts as a
//! same-origin bounce: it serves the HTML shell (no auth check), then
//! immediately navigates to `/settings/billing` via client-side routing.
//! The second navigation is same-origin, so cookies are sent normally.

use leptos::prelude::*;

/// Billing portal return bounce page.
#[component]
pub fn BillingReturnPage() -> impl IntoView {
    #[cfg(target_arch = "wasm32")]
    {
        let navigate = leptos_router::hooks::use_navigate();
        // Replace so this intermediate URL doesn't appear in browser history
        navigate(
            "/settings/billing",
            leptos_router::NavigateOptions {
                replace: true,
                ..Default::default()
            },
        );
    }

    view! {
        <div class="flex items-center justify-center min-h-screen bg-background">
            <p class="text-muted-foreground text-sm">"Returning to billing..."</p>
        </div>
    }
}
