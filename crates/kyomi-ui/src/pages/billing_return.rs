// SPDX-License-Identifier: AGPL-3.0-or-later

//! Intermediate landing page for Stripe Billing Portal return.
//!
//! The Billing Portal is hosted by Stripe — when the user clicks "Done",
//! Stripe redirects back to our domain. Because cookies are SameSite=Strict,
//! they aren't sent on this cross-site navigation. This page acts as a
//! same-origin bounce: it serves the HTML shell (no auth check), then
//! immediately navigates to `/settings/billing` via client-side routing.
//! The second navigation is same-origin, so cookies are sent normally.
//!
//! That bounce to `/settings/billing` mounts a fresh `Layout` (KYO-806
//! item 5), which fetches `get_sidebar_user`/`get_user_context` from
//! scratch rather than reusing any state from before the Portal round trip.
//! So the outcome of the Portal visit is whatever those refetch to: if the
//! workspace is still lapsed, `Layout`'s `show_paywall` gate renders the
//! full-screen paywall at this URL, exactly as it would anywhere else; if
//! the Portal visit fixed billing, the user lands in the normal settings
//! page. No code here makes that decision — it's a consequence of `Layout`
//! always mounting fresh on navigation, not special-cased behaviour.

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
