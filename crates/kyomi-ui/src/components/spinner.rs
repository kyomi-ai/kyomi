// SPDX-License-Identifier: AGPL-3.0-or-later

use leptos::prelude::*;

/// Spinner component matching the React `<Spinner />` (Loader2 icon with animate-spin).
///
/// Uses an inline SVG replicating Lucide's Loader2 icon since we cannot use
/// the lucide-react npm package in Leptos. The SVG paths are copied verbatim
/// from the Lucide icon set.
///
/// Sizes match the React Spinner component:
/// - sm: h-4 w-4 (default, used inside buttons)
/// - md: h-6 w-6
/// - lg: h-8 w-8
#[component]
pub fn Spinner(
    /// Additional CSS classes to apply (e.g., "text-white", "text-muted-foreground")
    #[prop(into, optional)]
    class: String,
) -> impl IntoView {
    // Default size is sm (h-4 w-4) matching the React Spinner's default
    let classes = format!("animate-spin h-4 w-4 {class}");

    view! {
        <svg
            class=classes
            xmlns="http://www.w3.org/2000/svg"
            width="24"
            height="24"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            stroke-width="2"
            stroke-linecap="round"
            stroke-linejoin="round"
        >
            // Lucide Loader2 icon paths
            <path d="M21 12a9 9 0 1 1-6.219-8.56" />
        </svg>
    }
}
