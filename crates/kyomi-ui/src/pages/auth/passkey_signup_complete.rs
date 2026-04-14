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
use leptos_icons::Icon;

use crate::components::{
    Alert, AlertDescription, AlertVariant, Button, ButtonLink, ButtonSize, ButtonVariant, Checkbox,
    Label, INPUT_CLASS,
};
use crate::pages::auth::auth_layout::AuthLayout;
use crate::server_fns::auth::{
    passkey_register_complete, passkey_signup_complete, LoginResult, PasskeyRegisterStartResult,
};

// ─────────────────────────────────────────────────────────────────────────────
// View state machine
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq)]
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

    // ── Reactive title & subtitle ────────────────────────────────────────
    let title = Signal::derive(move || match page_state.get() {
        PageState::Form => "Email Verified".to_string(),
        PageState::Creating { .. } => "Creating Account".to_string(),
        PageState::Success => "Account Created".to_string(),
        PageState::Error { .. } => "Signup Link Invalid".to_string(),
    });
    let subtitle = Signal::derive(move || match page_state.get() {
        PageState::Form => "Complete your account setup below.".to_string(),
        PageState::Creating { status_message } => status_message,
        PageState::Success => "Welcome to Kyomi! Setting up your workspace...".to_string(),
        PageState::Error { message } => message,
    });

    // ── Render ────────────────────────────────────────────────────────────
    view! {
        <AuthLayout title=title subtitle=subtitle>
            {move || {
                let state = page_state.get();
                match state {
                    PageState::Error { .. } => error_view().into_any(),
                    PageState::Success => success_view().into_any(),
                    PageState::Creating { .. } => creating_view().into_any(),
                    PageState::Form => view! {
                        <div>
                            <div class="text-center">
                                <div class="inline-flex items-center justify-center w-16 h-16 rounded-full bg-primary/10 mx-auto mb-6">
                                    <Icon icon=icondata_lu::LuCheck attr:class="w-8 h-8 text-primary"/>
                                </div>
                            </div>
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
                        </div>
                    }.into_any(),
                }
            }}
        </AuthLayout>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Error view
// ─────────────────────────────────────────────────────────────────────────────

fn error_view() -> impl IntoView {
    view! {
        <div class="space-y-4">
            <div class="text-center">
                <div class="inline-flex items-center justify-center w-16 h-16 rounded-full bg-error/10 mx-auto mb-6">
                    <Icon icon=icondata_lu::LuTriangleAlert attr:class="w-8 h-8 text-error-foreground"/>
                </div>
            </div>
            <ButtonLink href="/login" variant=ButtonVariant::Outline class="w-full">
                "Back to Login"
            </ButtonLink>
        </div>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Success view
// ─────────────────────────────────────────────────────────────────────────────

fn success_view() -> impl IntoView {
    view! {
        <div class="space-y-4">
            <div class="text-center">
                <div class="inline-flex items-center justify-center w-16 h-16 rounded-full bg-success/10 mx-auto mb-6">
                    <Icon icon=icondata_lu::LuCheck attr:class="w-8 h-8 text-success-foreground"/>
                </div>
            </div>
            // Branded moment (auth page) — DESIGN.md Loading State Pattern
            <img src="/kyomi_animated_logo.svg" alt="Processing" class="w-8 h-8 mx-auto"/>
        </div>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Creating view
// ─────────────────────────────────────────────────────────────────────────────

fn creating_view() -> impl IntoView {
    view! {
        <div class="text-center space-y-4">
            // Branded moment (auth page) — DESIGN.md Loading State Pattern
            <img src="/kyomi_animated_logo.svg" alt="Processing" class="w-12 h-12 mx-auto"/>
        </div>
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
