// SPDX-License-Identifier: AGPL-3.0-or-later

use leptos::prelude::*;
use phosphor_leptos::Icon;
use crate::components::{Button, ButtonSize, ButtonVariant, Spinner};

/// Passkey button — wraps the design-system `<Button>` with loading/icon.
///
/// Shared by the login page's "Sign in with Passkey" action and the signup
/// page's "Sign up with Passkey" action — the two flows differ only in
/// label text, so the label is a prop rather than each caller hand-rolling
/// its own near-duplicate button.
#[component]
pub fn PasskeySignInButton(
    #[prop(into)] loading: Signal<bool>,
    #[prop(into)] disabled: Signal<bool>,
    on_click: Callback<()>,
    /// Resting-state label, e.g. "Sign in with Passkey" or "Sign up with Passkey".
    #[prop(into, default = "Sign in with Passkey".to_string())]
    label: String,
    /// Loading-state label, e.g. "Authenticating..." or "Creating passkey...".
    #[prop(into, default = "Authenticating...".to_string())]
    loading_label: String,
) -> impl IntoView {
    let is_disabled = Signal::derive(move || loading.get() || disabled.get());

    let handle_click = move |_| {
        if !is_disabled.get() {
            on_click.run(());
        }
    };

    view! {
        <Button
            variant=ButtonVariant::Default
            size=ButtonSize::Lg
            disabled=is_disabled
            class="w-full"
            on:click=handle_click
        >
            {move || {
                if loading.get() {
                    view! {
                        <Spinner class="text-primary-foreground"/>
                        <span>{loading_label.clone()}</span>
                    }.into_any()
                } else {
                    view! {
                        <Icon icon=phosphor_leptos::KEY size="20px"/>
                        <span>{label.clone()}</span>
                    }.into_any()
                }
            }}
        </Button>
    }
}
