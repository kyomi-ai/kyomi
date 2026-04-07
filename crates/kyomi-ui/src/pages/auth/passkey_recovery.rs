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

use crate::components::{
    Alert, AlertDescription, AlertTitle, AlertVariant, Card, CardContent, CardDescription,
    CardHeader, CardTitle, Label, Spinner, INPUT_CLASS,
};
use crate::server_fns::auth::recovery_start;

// ─────────────────────────────────────────────────────────────────────────────
// Button class constants (composed inline like account_recovery.rs)
// ─────────────────────────────────────────────────────────────────────────────

/// Base + Default variant + Lg size + w-full
const BTN_PRIMARY_LG_FULL: &str = "inline-flex items-center justify-center gap-2 whitespace-nowrap rounded-md text-sm font-medium transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:pointer-events-none disabled:opacity-50 [&_svg]:pointer-events-none [&_svg]:size-4 [&_svg]:shrink-0 bg-primary text-primary-foreground shadow hover:bg-primary/90 h-10 rounded-md px-8 w-full";

/// Base + Outline variant + Default size + w-full mb-4
const BTN_OUTLINE_FULL: &str = "inline-flex items-center justify-center gap-2 whitespace-nowrap rounded-md text-sm font-medium transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:pointer-events-none disabled:opacity-50 [&_svg]:pointer-events-none [&_svg]:size-4 [&_svg]:shrink-0 border border-input bg-background text-foreground shadow-sm hover:bg-secondary hover:text-accent-foreground h-9 px-4 py-2 w-full mb-4";

/// Base + Link variant + Default size + w-full text-muted-foreground
const BTN_LINK_FULL_MUTED: &str = "inline-flex items-center justify-center gap-2 whitespace-nowrap rounded-md text-sm font-medium transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:pointer-events-none disabled:opacity-50 [&_svg]:pointer-events-none [&_svg]:size-4 [&_svg]:shrink-0 text-primary underline-offset-4 hover:underline h-9 px-4 py-2 w-full text-muted-foreground";

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
                        <svg
                            class="w-8 h-8 text-primary"
                            fill="none"
                            stroke="currentColor"
                            viewBox="0 0 24 24"
                        >
                            <path
                                stroke-linecap="round"
                                stroke-linejoin="round"
                                stroke-width="2"
                                d="M15 7a2 2 0 012 2m4 0a6 6 0 01-7.743 5.743L11 17H9v2H7v2H4a1 1 0 01-1-1v-2.586a1 1 0 01.293-.707l5.964-5.964A6 6 0 1121 9z"
                            />
                        </svg>
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
                            class=format!("{} h-11", INPUT_CLASS)
                            prop:value=move || email.get()
                            on:input=move |ev| set_email.set(event_target_value(&ev))
                        />
                    </div>

                    <button
                        type="submit"
                        disabled=submit_disabled
                        class=BTN_PRIMARY_LG_FULL
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
                    </button>

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
                        <svg
                            class="w-8 h-8 text-primary"
                            fill="none"
                            stroke="currentColor"
                            viewBox="0 0 24 24"
                        >
                            <path
                                stroke-linecap="round"
                                stroke-linejoin="round"
                                stroke-width="2"
                                d="M3 8l7.89 5.26a2 2 0 002.22 0L21 8M5 19h14a2 2 0 002-2V7a2 2 0 00-2-2H5a2 2 0 00-2 2v10a2 2 0 002 2z"
                            />
                        </svg>
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
                    <a href="/login">
                        <button type="button" class=BTN_OUTLINE_FULL>
                            "Back to Login"
                        </button>
                    </a>

                    <button
                        type="button"
                        on:click=move |_| {
                            set_submitted.set(false);
                            set_email.set(String::new());
                        }
                        class=BTN_LINK_FULL_MUTED
                    >
                        "Try a different email"
                    </button>
                </div>
            </CardContent>
        </Card>
    }
}
