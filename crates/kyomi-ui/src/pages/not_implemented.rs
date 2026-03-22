// SPDX-License-Identifier: AGPL-3.0-or-later

//! Placeholder page for routes not yet implemented in the Leptos frontend.

use leptos::prelude::*;

/// Shown for any route that exists in the old React frontend but hasn't been
/// migrated to Leptos yet. Provides a clear signal during migration that
/// the route is known but intentionally incomplete, not a 404.
#[component]
pub fn NotImplementedPage(
    /// Human-readable name of the page, e.g. "Chat" or "SQL Editor".
    name: &'static str,
) -> impl IntoView {
    view! {
        <div class="flex flex-col items-center justify-center min-h-[60vh] gap-6 text-center px-4">
            <div class="w-16 h-16 rounded-full bg-muted flex items-center justify-center">
                <svg
                    class="w-8 h-8 text-muted-foreground"
                    fill="none"
                    stroke="currentColor"
                    viewBox="0 0 24 24"
                >
                    <path
                        stroke-linecap="round"
                        stroke-linejoin="round"
                        stroke-width="2"
                        d="M12 6v6m0 0v6m0-6h6m-6 0H6"
                    />
                </svg>
            </div>
            <div class="space-y-2">
                <h1 class="text-2xl font-semibold text-foreground">{name}</h1>
                <p class="text-muted-foreground max-w-md">
                    "This page hasn't been migrated to the new frontend yet."
                </p>
            </div>
        </div>
    }
}
