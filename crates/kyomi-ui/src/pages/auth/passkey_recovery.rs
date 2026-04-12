// SPDX-License-Identifier: AGPL-3.0-or-later

//! Passkey recovery page — matches `apps/frontend/src/pages/PasskeyRecovery.jsx`.
//!
//! Route: `/auth/recover-passkey`
//!
//! Flow:
//! 1. User enters their email address
//! 2. Backend sends recovery email (if verified account exists)
//! 3. Same success message shown regardless (prevents email enumeration)
//!
//! Uses Card layout centered on screen, NOT AuthLayout.

use leptos::prelude::*;
use leptos_icons::Icon;

use crate::components::{
    Alert, AlertDescription, AlertTitle, AlertVariant, Button, ButtonLink, ButtonSize,
    ButtonVariant, Card, CardContent, CardDescription, CardHeader, CardTitle, Label, Spinner,
    INPUT_CLASS,
};
use crate::server_fns::auth::recovery_start;

// ─────────────────────────────────────────────────────────────────────────────
// Main component
// ─────────────────────────────────────────────────────────────────────────────

#[component]
pub fn PasskeyRecoveryPage() -> impl IntoView {
    let (email, set_email) = signal(String::new());
    let (loading, set_loading) = signal(false);
    let (submitted, set_submitted) = signal(false);
    let (error, set_error) = signal(Option::<String>::None);

    // ── Form submit handler ──────────────────────────────────────────────
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
            // Always transition to submitted — prevents email enumeration
            let _ = recovery_start(current_email).await;
            set_submitted.set(true);
            set_loading.set(false);
        });
    };

    // ── Derived: disable submit when email empty or loading ──────────────
    let submit_disabled = move || loading.get() || email.get().trim().is_empty();

    // ── Render ───────────────────────────────────────────────────────────
    view! {
        <div class="min-h-screen bg-background flex items-center justify-center p-4">
            {move || {
                if submitted.get() {
                    view! { <SubmittedView set_submitted=set_submitted set_email=set_email /> }
                        .into_any()
                } else {
                    view! {
                        <FormView
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
        </div>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Form View — email input
// ─────────────────────────────────────────────────────────────────────────────

#[component]
fn FormView(
    email: ReadSignal<String>,
    set_email: WriteSignal<String>,
    loading: ReadSignal<bool>,
    error: ReadSignal<Option<String>>,
    submit_disabled: impl Fn() -> bool + Copy + Send + Sync + 'static,
    on_submit: impl Fn(leptos::ev::SubmitEvent) + Copy + Send + Sync + 'static,
) -> impl IntoView {
    view! {
        <Card class="w-full max-w-md">
            <CardHeader>
                <div class="text-center">
                    <div class="inline-flex items-center justify-center w-16 h-16 rounded-full bg-primary/10 mx-auto mb-4">
                        <Icon icon=icondata_lu::LuKeyRound attr:class="w-8 h-8 text-primary"/>
                    </div>
                    <CardTitle class="text-xl">"Recover Your Account"</CardTitle>
                    <CardDescription>
                        "Enter your email address to receive a recovery link."
                    </CardDescription>
                </div>
            </CardHeader>
            <CardContent>
                <form on:submit=on_submit class="space-y-4">
                    // Error alert
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
            </CardContent>
        </Card>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Submitted View — confirmation
// ─────────────────────────────────────────────────────────────────────────────

#[component]
fn SubmittedView(
    set_submitted: WriteSignal<bool>,
    set_email: WriteSignal<String>,
) -> impl IntoView {
    view! {
        <Card class="w-full max-w-md">
            <CardHeader>
                <div class="text-center">
                    <div class="inline-flex items-center justify-center w-16 h-16 rounded-full bg-primary/10 mx-auto mb-4">
                        <Icon icon=icondata_lu::LuMail attr:class="w-8 h-8 text-primary"/>
                    </div>
                    <CardTitle class="text-xl">"Check Your Email"</CardTitle>
                    <CardDescription>
                        "If a verified account exists with this email, we have sent a recovery link."
                    </CardDescription>
                </div>
            </CardHeader>
            <CardContent class="space-y-4">
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
            </CardContent>
        </Card>
    }
}
