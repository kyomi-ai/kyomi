// SPDX-License-Identifier: AGPL-3.0-or-later

use leptos::prelude::*;

use crate::components::Spinner;

/// Passkey sign-in button matching the React Login.jsx passkey button.
///
/// Phase 1: Visual placeholder that calls the on_click handler when clicked.
/// Actual WebAuthn integration comes in Phase 4.
///
/// CSS classes are copied verbatim from the React source.
#[component]
pub fn PasskeySignInButton(
    /// Whether the button is in loading state
    #[prop(into)]
    loading: Signal<bool>,
    /// Whether the button should be disabled (e.g., other auth in progress)
    #[prop(into)]
    disabled: Signal<bool>,
    /// Click handler
    on_click: Callback<()>,
) -> impl IntoView {
    let is_disabled = move || loading.get() || disabled.get();

    let handle_click = move |_| {
        if !is_disabled() {
            on_click.run(());
        }
    };

    view! {
        <button
            type="button"
            on:click=handle_click
            disabled=is_disabled
            class="w-full py-3.5 px-4 bg-primary text-white font-semibold rounded-lg shadow hover:shadow focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring transition-all duration-200 hover:opacity-90 disabled:opacity-50 disabled:cursor-not-allowed"
        >
            {move || {
                if loading.get() {
                    view! {
                        <div class="flex items-center justify-center space-x-2">
                            <Spinner class="text-white" />
                            <span>"Authenticating..."</span>
                        </div>
                    }.into_any()
                } else {
                    view! {
                        <div class="flex items-center justify-center space-x-2">
                            <span class="text-lg">"🔑"</span>
                            <span>"Sign in with Passkey"</span>
                        </div>
                    }.into_any()
                }
            }}
        </button>
    }
}
