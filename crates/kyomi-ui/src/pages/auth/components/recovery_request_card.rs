// SPDX-License-Identifier: AGPL-3.0-or-later

//! Shared recovery request card used by both account recovery and passkey recovery.
//!
//! Renders an email input → Send Recovery Link form, then transitions to a
//! "Check Your Email" confirmation on submit. Always transitions to the
//! submitted state regardless of backend response to prevent email enumeration.

use leptos::prelude::*;
use leptos_icons::Icon;

use crate::components::{
    Alert, AlertDescription, AlertTitle, AlertVariant, Button, ButtonLink, ButtonSize,
    ButtonVariant, Label, Spinner, INPUT_CLASS,
};
use crate::pages::auth::auth_layout::AuthLayout;
use crate::server_fns::auth::recovery_start;

/// Which recovery flow this card represents. Controls the icon and title.
#[derive(Clone, Copy, PartialEq)]
pub enum RecoveryKind {
    Account,
    Passkey,
}

impl RecoveryKind {
    fn title(self) -> &'static str {
        match self {
            Self::Account => "Recover Your Account",
            Self::Passkey => "Recover Your Passkey",
        }
    }
}

#[component]
pub fn RecoveryRequestCard(kind: RecoveryKind) -> impl IntoView {
    let (email, set_email) = signal(String::new());
    let (loading, set_loading) = signal(false);
    let (submitted, set_submitted) = signal(false);
    let (error, set_error) = signal(Option::<String>::None);

    let on_submit = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();

        let current_email = email.get_untracked();
        if current_email.trim().is_empty() {
            set_error.set(Some("Please enter your email address.".to_string()));
            return;
        }

        set_loading.set(true);
        set_error.set(None);

        leptos::task::spawn_local(async move {
            let _ = recovery_start(current_email).await;
            set_submitted.set(true);
            set_loading.set(false);
        });
    };

    let submit_disabled = move || loading.get() || email.get().trim().is_empty();

    let title = Signal::derive(move || {
        if submitted.get() {
            "Check Your Email".to_string()
        } else {
            kind.title().to_string()
        }
    });
    let subtitle = Signal::derive(move || {
        if submitted.get() {
            "If a verified account exists with this email, we have sent a recovery link."
                .to_string()
        } else {
            "Enter your email address to receive a recovery link.".to_string()
        }
    });

    view! {
        <AuthLayout title=title subtitle=subtitle>
            {move || {
                if submitted.get() {
                    view! { <SubmittedView set_submitted=set_submitted set_email=set_email/> }
                        .into_any()
                } else {
                    view! {
                        <FormView
                            kind=kind
                            email=email
                            set_email=set_email
                            loading=loading
                            error=error
                            submit_disabled=submit_disabled
                            on_submit=on_submit
                        />
                    }
                        .into_any()
                }
            }}
        </AuthLayout>
    }
}

#[component]
fn FormView(
    kind: RecoveryKind,
    email: ReadSignal<String>,
    set_email: WriteSignal<String>,
    loading: ReadSignal<bool>,
    error: ReadSignal<Option<String>>,
    submit_disabled: impl Fn() -> bool + Copy + Send + Sync + 'static,
    on_submit: impl Fn(leptos::ev::SubmitEvent) + Copy + Send + Sync + 'static,
) -> impl IntoView {
    view! {
        <div>
            <div class="text-center">
                <div class="inline-flex items-center justify-center w-16 h-16 rounded-full bg-primary/10 mx-auto mb-6">
                    {match kind {
                        RecoveryKind::Account => view! {
                            <Icon icon=icondata_lu::LuLockKeyhole attr:class="w-8 h-8 text-primary"/>
                        }.into_any(),
                        RecoveryKind::Passkey => view! {
                            <Icon icon=icondata_lu::LuKeyRound attr:class="w-8 h-8 text-primary"/>
                        }.into_any(),
                    }}
                </div>
            </div>
            <form on:submit=on_submit class="space-y-4">
                    <Show when=move || error.get().is_some()>
                        <Alert variant=AlertVariant::Error>
                            <AlertTitle>"Error"</AlertTitle>
                            <AlertDescription>
                                {move || error.get().unwrap_or_default()}
                            </AlertDescription>
                        </Alert>
                    </Show>

                    <div class="space-y-2">
                        <Label html_for="recovery-email">"Email address"</Label>
                        <input
                            id="recovery-email"
                            type="email"
                            placeholder="you@example.com"
                            autocomplete="email"
                            autofocus=true
                            required=true
                            class=INPUT_CLASS
                            prop:value=move || email.get()
                            on:input=move |ev| set_email.set(event_target_value(&ev))
                        />
                    </div>

                    <Button
                        button_type="submit"
                        variant=ButtonVariant::Default
                        size=ButtonSize::Lg
                        disabled=Signal::derive(submit_disabled)
                        class="w-full"
                    >
                        {move || {
                            if loading.get() {
                                view! {
                                    <div class="flex items-center justify-center space-x-2">
                                        <Spinner class="text-primary-foreground"/>
                                        <span>"Sending..."</span>
                                    </div>
                                }.into_any()
                            } else {
                                view! { <span>"Send Recovery Link"</span> }.into_any()
                            }
                        }}
                    </Button>

                <div class="text-center pt-2">
                    <a
                        href="/login"
                        class="text-sm text-muted-foreground hover:text-foreground transition-colors"
                    >
                        "Back to login"
                    </a>
                </div>
            </form>
        </div>
    }
}

#[component]
fn SubmittedView(
    set_submitted: WriteSignal<bool>,
    set_email: WriteSignal<String>,
) -> impl IntoView {
    view! {
        <div class="space-y-4">
            <div class="text-center">
                <div class="inline-flex items-center justify-center w-16 h-16 rounded-full bg-primary/10 mx-auto mb-6">
                    <Icon icon=icondata_lu::LuMail attr:class="w-8 h-8 text-primary"/>
                </div>
            </div>
            <p class="text-sm text-center text-muted-foreground">
                "The recovery link expires in 15 minutes and can only be used once."
            </p>

            <div class="pt-4">
                <ButtonLink
                    href="/login"
                    variant=ButtonVariant::Outline
                    size=ButtonSize::Lg
                    class="w-full mb-4"
                >
                    "Back to Login"
                </ButtonLink>

                <Button
                    variant=ButtonVariant::Link
                    class="w-full"
                    on:click=move |_| {
                        set_submitted.set(false);
                        set_email.set(String::new());
                    }
                >
                    "Try a different email"
                </Button>
            </div>
        </div>
    }
}
