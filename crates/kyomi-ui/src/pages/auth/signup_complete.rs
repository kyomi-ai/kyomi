// SPDX-License-Identifier: AGPL-3.0-or-later

//! Signup completion page — matches `apps/frontend/src/pages/SignupComplete.jsx`.
//!
//! Route: `/signup/complete?token=xxx`
//!
//! Flow:
//! 1. User clicks email link with signup token
//! 2. User enters name, password, confirm password, accepts terms
//! 3. Click "Create Account" -> verifies token, creates account with password, logs in
//! 4. Redirect to /onboarding for datasource setup
//!
//! State machine: Form | Creating | Success | Error

use leptos::prelude::*;
use leptos_icons::Icon;

use crate::components::{
    Alert, AlertDescription, AlertVariant, Button, ButtonSize, ButtonVariant, Card, CardContent,
    CardDescription, CardHeader, CardTitle, Checkbox, Label, Spinner, INPUT_CLASS,
};
use crate::server_fns::auth::{signup_complete, SignupCompleteResult};

// ─────────────────────────────────────────────────────────────────────────────
// View state machine
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq)]
enum PageState {
    Form,
    Creating,
    Success,
    Error { message: String },
}

// ─────────────────────────────────────────────────────────────────────────────
// Main component
// ─────────────────────────────────────────────────────────────────────────────

#[component]
pub fn SignupCompletePage() -> impl IntoView {
    // ── Extract token from URL query params ──────────────────────────────
    let (token, _set_token) = signal(Option::<String>::None);
    let (page_state, set_page_state) = signal(PageState::Form);

    // ── Form signals ─────────────────────────────────────────────────────
    let (name, set_name) = signal(String::new());
    let (password, set_password) = signal(String::new());
    let (confirm_password, set_confirm_password) = signal(String::new());
    let (terms_accepted, set_terms_accepted) = signal(false);
    let (marketing_consent, set_marketing_consent) = signal(false);
    let (error, set_error) = signal(Option::<String>::None);

    // ── Extract token on mount ───────────────────────────────────────────
    // Token extraction is browser-only; SSR provides None.
    #[cfg(target_arch = "wasm32")]
    let initial_token: Option<String> = {
        web_sys::window().and_then(|w| {
            w.location()
                .search()
                .ok()
                .and_then(|search| web_sys::UrlSearchParams::new_with_str(&search).ok())
                .and_then(|params| params.get("token"))
        })
    };
    #[cfg(not(target_arch = "wasm32"))]
    let initial_token: Option<String> = None;

    // Set the token or transition to Error — runs on both targets so the
    // compiler sees all PageState variants constructed.
    if let Some(t) = initial_token {
        _set_token.set(Some(t));
    } else {
        set_page_state.set(PageState::Error {
            message: "Missing signup token. Please use the link from your email.".to_string(),
        });
    }

    // ── Checkbox signals for the Checkbox component ──────────────────────
    let terms_signal = Signal::derive(move || terms_accepted.get());
    let marketing_signal = Signal::derive(move || marketing_consent.get());

    let on_terms_change = Callback::new(move |val: bool| {
        set_terms_accepted.set(val);
    });
    let on_marketing_change = Callback::new(move |val: bool| {
        set_marketing_consent.set(val);
    });

    // ── Form submit handler ──────────────────────────────────────────────
    let on_submit = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();

        let current_name = name.get_untracked();
        let current_password = password.get_untracked();
        let current_confirm = confirm_password.get_untracked();
        let current_terms = terms_accepted.get_untracked();
        let current_marketing = marketing_consent.get_untracked();
        let current_token = token.get_untracked();

        // Client-side validation
        if current_name.trim().is_empty() {
            set_error.set(Some("Please enter your name.".to_string()));
            return;
        }
        if current_password.is_empty() {
            set_error.set(Some("Please enter a password.".to_string()));
            return;
        }
        if current_password.len() < 8 {
            set_error.set(Some(
                "Password must be at least 8 characters.".to_string(),
            ));
            return;
        }
        if current_password != current_confirm {
            set_error.set(Some("Passwords do not match.".to_string()));
            return;
        }
        if !current_terms {
            set_error.set(Some(
                "Please accept the Terms of Service and Privacy Policy.".to_string(),
            ));
            return;
        }

        let Some(tok) = current_token else {
            set_error.set(Some(
                "Missing signup token. Please use the link from your email.".to_string(),
            ));
            return;
        };

        set_error.set(None);
        set_page_state.set(PageState::Creating);

        leptos::task::spawn_local(async move {
            let result = signup_complete(
                tok,
                current_name.trim().to_string(),
                current_password,
                current_terms,
                current_marketing,
            )
            .await;

            match result {
                Ok(SignupCompleteResult::Success { .. }) => {
                    set_page_state.set(PageState::Success);

                    // Redirect to onboarding after 1.5 seconds
                    #[cfg(target_arch = "wasm32")]
                    {
                        use wasm_bindgen::prelude::*;
                        let window = web_sys::window().unwrap();
                        let closure = Closure::once(move || {
                            if let Some(window) = web_sys::window() {
                                let _ = window.location().set_href("/onboarding");
                            }
                        });
                        let _ = window.set_timeout_with_callback_and_timeout_and_arguments_0(
                            closure.as_ref().unchecked_ref(),
                            1500,
                        );
                        closure.forget();
                    }
                }
                Ok(SignupCompleteResult::Error { message }) => {
                    set_error.set(Some(message));
                    set_page_state.set(PageState::Form); // Allow retry
                }
                Err(e) => {
                    set_error.set(Some(format!("Server error: {}", e)));
                    set_page_state.set(PageState::Form); // Allow retry
                }
            }
        });
    };

    // ── Render ────────────────────────────────────────────────────────────
    view! {
        <div class="min-h-screen bg-background flex items-center justify-center p-4">
            {move || {
                let state = page_state.get();
                match state {
                    PageState::Error { message } => error_view(message).into_any(),
                    PageState::Success => success_view().into_any(),
                    PageState::Creating => creating_view().into_any(),
                    PageState::Form => view! {
                        <Card class="w-full max-w-md">
                            <CardHeader>
                                <div class="text-center">
                                    <div class="inline-flex items-center justify-center w-16 h-16 rounded-full bg-primary/10 mx-auto mb-4">
                                        <Icon icon=icondata_lu::LuCheck attr:class="w-8 h-8 text-primary"/>
                                    </div>
                                    <CardTitle class="text-xl">"Email Verified!"</CardTitle>
                                    <CardDescription>
                                        "Complete your account setup below."
                                    </CardDescription>
                                </div>
                            </CardHeader>
                            <CardContent>
                                <form on:submit=on_submit class="space-y-6">
                                    // Name input
                                    <div class="space-y-2">
                                        <Label html_for="name">"Full Name"</Label>
                                        <input
                                            id="name"
                                            type="text"
                                            autocomplete="name"
                                            autofocus
                                            class=INPUT_CLASS
                                            placeholder="John Doe"
                                            required
                                            prop:value=move || name.get()
                                            on:input=move |ev| set_name.set(event_target_value(&ev))
                                        />
                                    </div>

                                    // Password input
                                    <div class="space-y-2">
                                        <Label html_for="password">"Password"</Label>
                                        <input
                                            id="password"
                                            type="password"
                                            autocomplete="new-password"
                                            class=INPUT_CLASS
                                            placeholder="At least 8 characters"
                                            minlength="8"
                                            required
                                            prop:value=move || password.get()
                                            on:input=move |ev| set_password.set(event_target_value(&ev))
                                        />
                                    </div>

                                    // Confirm password input
                                    <div class="space-y-2">
                                        <Label html_for="confirm-password">"Confirm Password"</Label>
                                        <input
                                            id="confirm-password"
                                            type="password"
                                            autocomplete="new-password"
                                            class=INPUT_CLASS
                                            placeholder="Re-enter your password"
                                            minlength="8"
                                            required
                                            prop:value=move || confirm_password.get()
                                            on:input=move |ev| set_confirm_password.set(event_target_value(&ev))
                                        />
                                    </div>

                                    // Terms and consent
                                    <div class="space-y-3">
                                        <div
                                            class="flex items-start space-x-3 cursor-pointer"
                                            on:click=move |_| set_terms_accepted.set(!terms_accepted.get_untracked())
                                        >
                                            <Checkbox
                                                checked=terms_signal
                                                on_change=on_terms_change
                                                class="mt-0.5"
                                            />
                                            <span class="text-sm text-foreground">
                                                "I have read and agree to the "
                                                <a
                                                    href="https://kyomi.ai/terms"
                                                    target="_blank"
                                                    rel="noopener noreferrer"
                                                    class="text-primary hover:underline"
                                                    on:click=move |ev: leptos::ev::MouseEvent| ev.stop_propagation()
                                                >
                                                    "Terms of Service"
                                                </a>
                                                " and "
                                                <a
                                                    href="https://kyomi.ai/privacy"
                                                    target="_blank"
                                                    rel="noopener noreferrer"
                                                    class="text-primary hover:underline"
                                                    on:click=move |ev: leptos::ev::MouseEvent| ev.stop_propagation()
                                                >
                                                    "Privacy Policy"
                                                </a>
                                            </span>
                                        </div>

                                        <div
                                            class="flex items-start space-x-3 cursor-pointer"
                                            on:click=move |_| set_marketing_consent.set(!marketing_consent.get_untracked())
                                        >
                                            <Checkbox
                                                checked=marketing_signal
                                                on_change=on_marketing_change
                                                class="mt-0.5"
                                            />
                                            <span class="text-sm text-muted-foreground">
                                                "I agree to receive product updates and announcements from Kyomi. You can unsubscribe anytime."
                                            </span>
                                        </div>
                                    </div>

                                    // Error alert
                                    {move || error.get().map(|msg| view! {
                                        <Alert variant=AlertVariant::Error>
                                            <AlertDescription>{msg}</AlertDescription>
                                        </Alert>
                                    })}

                                    <Button size=ButtonSize::Lg class="w-full">
                                        "Create Account"
                                    </Button>
                                </form>
                            </CardContent>
                        </Card>
                    }.into_any(),
                }
            }}
        </div>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Error view
// ─────────────────────────────────────────────────────────────────────────────

fn error_view(message: String) -> impl IntoView {
    view! {
        <Card class="w-full max-w-md">
            <CardHeader>
                <div class="text-center">
                    <div class="inline-flex items-center justify-center w-16 h-16 rounded-full bg-error/10 mx-auto mb-4">
                        <Icon icon=icondata_lu::LuTriangleAlert attr:class="w-8 h-8 text-error-foreground"/>
                    </div>
                    <CardTitle class="text-xl">"Signup Link Invalid"</CardTitle>
                    <CardDescription class="text-error-foreground">
                        {message}
                    </CardDescription>
                </div>
            </CardHeader>
            <CardContent class="space-y-4">
                <a href="/login">
                    <Button variant=ButtonVariant::Outline class="w-full">
                        "Back to Login"
                    </Button>
                </a>
            </CardContent>
        </Card>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Success view
// ─────────────────────────────────────────────────────────────────────────────

fn success_view() -> impl IntoView {
    view! {
        <Card class="w-full max-w-md">
            <CardHeader>
                <div class="text-center">
                    <div class="inline-flex items-center justify-center w-16 h-16 rounded-full bg-success/10 mx-auto mb-4">
                        <Icon icon=icondata_lu::LuCheck attr:class="w-8 h-8 text-success-foreground"/>
                    </div>
                    <CardTitle class="text-xl">"Account Created!"</CardTitle>
                    <CardDescription>
                        "Welcome to Kyomi! Setting up your workspace..."
                    </CardDescription>
                </div>
            </CardHeader>
            <CardContent>
                // Spinner lg: h-8 w-8 (matching React Spinner size="lg")
                <Spinner size="h-8 w-8" class="text-primary mx-auto"/>
            </CardContent>
        </Card>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Creating view
// ─────────────────────────────────────────────────────────────────────────────

fn creating_view() -> impl IntoView {
    view! {
        <Card class="w-full max-w-md">
            <CardContent class="pt-6">
                <div class="text-center space-y-4">
                    // Spinner xl: h-12 w-12 (matching React Spinner size="xl")
                    <Spinner size="h-12 w-12" class="text-primary mx-auto"/>
                    <p class="text-muted-foreground">"Creating your account..."</p>
                </div>
            </CardContent>
        </Card>
    }
}
