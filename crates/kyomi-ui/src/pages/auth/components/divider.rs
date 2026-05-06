// SPDX-License-Identifier: AGPL-3.0-or-later

use leptos::prelude::*;

/// Auth divider with centered text, matching the React Login.jsx divider.
///
/// Renders a horizontal line with text centered on top (e.g., "or", "or sign in with email").
///
/// CSS classes are copied verbatim from the React source.
#[component]
pub fn AuthDivider(
    /// The text to show in the divider (e.g., "or", "or sign in with email")
    #[prop(into)]
    text: String,
) -> impl IntoView {
    view! {
        <div class="relative my-6">
            <div class="absolute inset-0 flex items-center">
                <div class="w-full border-t border-border"></div>
            </div>
            <div class="relative flex justify-center text-sm">
                <span class="px-4 bg-background text-muted-foreground">{text}</span>
            </div>
        </div>
    }
}
