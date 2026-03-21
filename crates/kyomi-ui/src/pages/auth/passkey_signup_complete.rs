// SPDX-License-Identifier: AGPL-3.0-or-later

//! Passkey signup completion page — matches `apps/frontend/src/pages/PasskeySignupComplete.jsx`.
//!
//! Route: `/auth/passkey-signup?token=xxx`
//!
//! Unified flow (single page, single button):
//! 1. User clicks email link with signup token
//! 2. User enters name, accepts terms (single form)
//! 3. Click "Create Account" -> verifies token, creates passkey, logs in
//! 4. Redirect to /onboarding for datasource setup
//!
//! State machine: Form | Creating { status_message } | Success | Error

use leptos::prelude::*;

use crate::components::{
    Alert, AlertDescription, AlertVariant, Button, ButtonSize, ButtonVariant, Card, CardContent,
    CardDescription, CardHeader, CardTitle, Checkbox, Label, INPUT_CLASS,
};
use crate::server_fns::auth::{
    passkey_register_complete, passkey_signup_complete, LoginResult, PasskeyRegisterStartResult,
};

// ─────────────────────────────────────────────────────────────────────────────
// View state machine
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq)]
#[allow(dead_code)] // Variants constructed in wasm32-only blocks
enum PageState {
    Form,
    Creating { status_message: String },
    Success,
    Error { message: String },
}

// ─────────────────────────────────────────────────────────────────────────────
// Main component
// ─────────────────────────────────────────────────────────────────────────────

#[component]
pub fn PasskeySignupCompletePage() -> impl IntoView {
    // ── Extract token from URL query params ──────────────────────────────
    let (token, _set_token) = signal(Option::<String>::None);
    let (page_state, set_page_state) = signal(PageState::Form);

    // ── Form signals ─────────────────────────────────────────────────────
    let (name, set_name) = signal(String::new());
    let (terms_accepted, set_terms_accepted) = signal(false);
    let (marketing_consent, set_marketing_consent) = signal(false);
    let (error, set_error) = signal(Option::<String>::None);

    // ── Extract token on mount ───────────────────────────────────────────
    #[cfg(target_arch = "wasm32")]
    {
        Effect::new(move || {
            if let Some(window) = web_sys::window() {
                if let Ok(search) = window.location().search() {
                    let params = web_sys::UrlSearchParams::new_with_str(&search).ok();
                    if let Some(params) = params {
                        if let Some(t) = params.get("token") {
                            _set_token.set(Some(t));
                            return;
                        }
                    }
                }
            }
            set_page_state.set(PageState::Error {
                message: "Missing signup token. Please use the link from your email.".to_string(),
            });
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
        let current_terms = terms_accepted.get_untracked();
        let current_marketing = marketing_consent.get_untracked();
        let current_token = token.get_untracked();

        // Client-side validation
        if current_name.trim().is_empty() {
            set_error.set(Some("Please enter your name.".to_string()));
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
        set_page_state.set(PageState::Creating {
            status_message: "Verifying your email...".to_string(),
        });

        leptos::task::spawn_local(async move {
            // Step 1: Verify token, update name/terms, get WebAuthn challenge
            let start_result = passkey_signup_complete(
                tok,
                current_name.trim().to_string(),
                current_terms,
                current_marketing,
            )
            .await;

            let PasskeyRegisterStartResult {
                challenge_id,
                creation_challenge,
            } = match start_result {
                Ok(r) => r,
                Err(e) => {
                    set_error.set(Some(format!("{}", e)));
                    set_page_state.set(PageState::Form);
                    return;
                }
            };

            // Step 2: Create passkey via WebAuthn
            set_page_state.set(PageState::Creating {
                status_message: "Creating your passkey...".to_string(),
            });

            let credential_json =
                match crate::utils::webauthn::start_registration(&creation_challenge).await {
                    Ok(json) => json,
                    Err(e) => {
                        let msg = map_webauthn_error(&e);
                        set_error.set(Some(msg));
                        set_page_state.set(PageState::Form);
                        return;
                    }
                };

            // Step 3: Complete registration on server
            set_page_state.set(PageState::Creating {
                status_message: "Finalizing your account...".to_string(),
            });

            match passkey_register_complete(challenge_id, credential_json).await {
                Ok(LoginResult::Success { .. }) => {
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
                        let _ = window
                            .set_timeout_with_callback_and_timeout_and_arguments_0(
                                closure.as_ref().unchecked_ref(),
                                1500,
                            );
                        closure.forget();
                    }
                }
                Ok(other) => {
                    let msg = match other {
                        LoginResult::Error { message } => message,
                        _ => "Unexpected response from server. Please try again.".to_string(),
                    };
                    set_error.set(Some(msg));
                    set_page_state.set(PageState::Form);
                }
                Err(e) => {
                    set_error.set(Some(format!("Failed to create account: {}", e)));
                    set_page_state.set(PageState::Form);
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
                    PageState::Creating { status_message } => creating_view(status_message).into_any(),
                    PageState::Form => view! {
                        <Card class="w-full max-w-md">
                            <CardHeader>
                                <div class="text-center">
                                    <div class="inline-flex items-center justify-center w-16 h-16 rounded-full bg-primary/10 mx-auto mb-4">
                                        <svg class="w-8 h-8 text-primary" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                                            <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M5 13l4 4L19 7" />
                                        </svg>
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

                                    <p class="text-xs text-center text-muted-foreground">
                                        "You will be prompted to create a passkey using your fingerprint, face, or security key."
                                    </p>
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
                        <svg class="w-8 h-8 text-error-foreground" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                            <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M12 9v2m0 4h.01m-6.938 4h13.856c1.54 0 2.502-1.667 1.732-3L13.732 4c-.77-1.333-2.694-1.333-3.464 0L3.34 16c-.77 1.333.192 3 1.732 3z" />
                        </svg>
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
                        <svg class="w-8 h-8 text-success-foreground" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                            <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M5 13l4 4L19 7" />
                        </svg>
                    </div>
                    <CardTitle class="text-xl">"Account Created!"</CardTitle>
                    <CardDescription>
                        "Welcome to Kyomi! Setting up your workspace..."
                    </CardDescription>
                </div>
            </CardHeader>
            <CardContent>
                // Spinner lg: h-8 w-8 (matching React Spinner size="lg")
                <svg
                    class="animate-spin h-8 w-8 text-primary mx-auto"
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
                    <path d="M21 12a9 9 0 1 1-6.219-8.56" />
                </svg>
            </CardContent>
        </Card>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Creating view
// ─────────────────────────────────────────────────────────────────────────────

fn creating_view(status_message: String) -> impl IntoView {
    view! {
        <Card class="w-full max-w-md">
            <CardContent class="pt-6">
                <div class="text-center space-y-4">
                    // Spinner xl: h-12 w-12 (matching React Spinner size="xl")
                    <svg
                        class="animate-spin h-12 w-12 text-primary mx-auto"
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
                        <path d="M21 12a9 9 0 1 1-6.219-8.56" />
                    </svg>
                    <p class="text-muted-foreground">{status_message}</p>
                </div>
            </CardContent>
        </Card>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// WebAuthn error mapping
// ─────────────────────────────────────────────────────────────────────────────

/// Map WebAuthn error strings to user-friendly messages.
///
/// Mirrors the React error handling in PasskeySignupComplete.jsx.
fn map_webauthn_error(error: &str) -> String {
    if error.contains("InvalidStateError") {
        "A passkey already exists for this device. Please try with a different device.".to_string()
    } else if error.contains("NotAllowedError") {
        "Passkey creation was cancelled or timed out. Please try again.".to_string()
    } else if error.contains("AbortError") {
        "Passkey creation was cancelled. Please try again.".to_string()
    } else if error.contains("NotSupportedError") {
        "Your device does not support passkeys. Please try a different authentication method."
            .to_string()
    } else {
        format!("Failed to create passkey: {}", error)
    }
}
