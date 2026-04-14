// SPDX-License-Identifier: AGPL-3.0-or-later

use leptos::prelude::*;
use phosphor_leptos::Icon;
use crate::components::{Button, ButtonSize, ButtonVariant, Spinner};

/// Passkey sign-in button — wraps the design-system `<Button>` with loading/icon.
#[component]
pub fn PasskeySignInButton(
    #[prop(into)] loading: Signal<bool>,
    #[prop(into)] disabled: Signal<bool>,
    on_click: Callback<()>,
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
                        <span>"Authenticating..."</span>
                    }.into_any()
                } else {
                    view! {
                        <Icon icon=phosphor_leptos::KEY size="20px"/>
                        <span>"Sign in with Passkey"</span>
                    }.into_any()
                }
            }}
        </Button>
    }
}
