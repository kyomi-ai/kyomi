// SPDX-License-Identifier: AGPL-3.0-or-later

//! Passkey recovery completion page — matches `apps/frontend/src/pages/PasskeyRecoveryComplete.jsx`.
//!
//! Route: `/auth/recover-passkey/complete?token=xxx`
//!
//! Flow:
//! 1. User clicks recovery link from email
//! 2. This page verifies the recovery token with backend
//! 3. On success, shows "Create New Passkey" button
//! 4. User creates new passkey via WebAuthn
//! 5. Auto-login and redirect to home
//!
//! State machine: Verifying | Ready | Creating | Success | Error

use leptos::prelude::*;
#[cfg(target_arch = "wasm32")]
use leptos_router::hooks::use_navigate;
use phosphor_leptos::Icon;
use crate::components::{
    Alert, AlertDescription, AlertTitle, AlertVariant, Button, ButtonLink, ButtonSize,
    ButtonVariant,
};
use crate::pages::auth::auth_layout::AuthLayout;
use crate::server_fns::auth::{
    passkey_recovery_complete, passkey_recovery_verify, LoginResult, PasskeyRecoveryVerifyResult,
};

// ─────────────────────────────────────────────────────────────────────────────
// View state machine
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq)]
enum PageState {
    Verifying,
    Ready {
        challenge_id: String,
        creation_challenge: String,
        email: String,
    },
    Creating,
    Success,
    Error {
        message: String,
    },
}

/// Verify the recovery token and return the resulting page state.
/// Not cfg-gated so the compiler sees all `PageState` variants constructed.
async fn verify_passkey_recovery_token(token: Option<String>) -> PageState {
    let Some(token) = token else {
        return PageState::Error {
            message: "Missing recovery token. Please use the link from your email.".to_string(),
        };
    };
    match passkey_recovery_verify(token).await {
        Ok(PasskeyRecoveryVerifyResult::Success {
            challenge_id,
            creation_challenge,
            email,
        }) => PageState::Ready {
            challenge_id,
            creation_challenge,
            email,
        },
        Ok(PasskeyRecoveryVerifyResult::Error { message }) => PageState::Error { message },
        Err(e) => PageState::Error {
            message: format!(
                "Invalid or expired recovery link. Please request a new one. ({})",
                e
            ),
        },
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Main component
// ─────────────────────────────────────────────────────────────────────────────

#[component]
pub fn PasskeyRecoveryCompletePage() -> impl IntoView {
    // ── SPA navigation handle (wasm32 only — only used in wasm async context) ─
    // Wrapped in StoredValue so it can be copied into FnMut closures (view! reactive closures).
    #[cfg(target_arch = "wasm32")]
    let navigate = StoredValue::new(use_navigate());

    let (page_state, set_page_state) = signal(PageState::Verifying);
    let (error, set_error) = signal(Option::<String>::None);

    // ── Extract token on mount and verify ────────────────────────────────
    // Token extraction is browser-only; SSR provides None (page won't be displayed).
    #[cfg(target_arch = "wasm32")]
    let initial_token: Option<String> = {
        let window = web_sys::window();
        window.and_then(|w| {
            w.location()
                .search()
                .ok()
                .and_then(|search| web_sys::UrlSearchParams::new_with_str(&search).ok())
                .and_then(|params| params.get("token"))
        })
    };
    #[cfg(not(target_arch = "wasm32"))]
    let initial_token: Option<String> = None;

    // ── Token verification action ────────────────────────────────────────
    // Converts the spawn_local on mount to an Action dispatched once via
    // Effect. verify_passkey_recovery_token only calls a server fn — no !Send
    // browser APIs — so Action is safe here.
    let verify_action = Action::new(move |token: &Option<String>| {
        let token = token.clone();
        async move { verify_passkey_recovery_token(token).await }
    });

    // Dispatch the verify action exactly once on mount.
    Effect::new(move |already_ran: Option<()>| {
        if already_ran.is_none() {
            verify_action.dispatch(initial_token.clone());
        }
    });

    // React to the verify action result: transition page state.
    Effect::new(move |_| {
        if let Some(new_state) = verify_action.value().get() {
            set_page_state.set(new_state);
        }
    });

    // ── Create passkey handler ────────────────────────────────────────────
    // Cannot use Action: calls start_registration() which uses JsFuture and
    // navigator.credentials.create() — !Send browser APIs. Signal writes
    // inside the async block use try_set for deferred-write safety.
    let on_create_passkey = move |_: leptos::ev::MouseEvent| {
        let current_state = page_state.get_untracked();
        let (challenge_id, creation_challenge) = match current_state {
            PageState::Ready {
                challenge_id,
                creation_challenge,
                ..
            } => (challenge_id, creation_challenge),
            _ => return,
        };

        set_error.set(None);
        set_page_state.set(PageState::Creating);

        leptos::task::spawn_local(async move {
            // Step 1: Create credential via WebAuthn
            let credential_json =
                match crate::utils::webauthn::start_registration(&creation_challenge).await {
                    Ok(json) => json,
                    Err(e) => {
                        let msg = map_webauthn_error(&e);
                        set_error.try_set(Some(msg));
                        // Allow retry — restore ready state.
                        // In practice, the challenge is still valid in KV.
                        set_page_state.try_set(PageState::Ready {
                            challenge_id,
                            creation_challenge,
                            email: String::new(), // email display is secondary
                        });
                        return;
                    }
                };

            // Step 2: Complete recovery registration on server (auto-login).
            // Binds to the recovery_session cookie set by
            // passkey_recovery_verify — see KYO-284.
            match passkey_recovery_complete(challenge_id.clone(), credential_json).await {
                Ok(LoginResult::Success { .. }) => {
                    set_page_state.try_set(PageState::Success);

                    // Navigate to home after 2 seconds (keeps WASM in memory).
                    // gloo_timers::future::TimeoutFuture is !Send — must stay
                    // in spawn_local.
                    #[cfg(target_arch = "wasm32")]
                    {
                        let Some(nav) = navigate.try_get_value() else { return };
                        gloo_timers::future::TimeoutFuture::new(2000).await;
                        nav("/", Default::default());
                    }
                }
                Ok(other) => {
                    let msg = match other {
                        LoginResult::Error { message } => message,
                        _ => "Unexpected response from server. Please try again.".to_string(),
                    };
                    set_error.try_set(Some(msg));
                    set_page_state.try_set(PageState::Ready {
                        challenge_id,
                        creation_challenge,
                        email: String::new(),
                    });
                }
                Err(e) => {
                    set_error.try_set(Some(format!("Failed to create passkey: {}", e)));
                    set_page_state.try_set(PageState::Ready {
                        challenge_id,
                        creation_challenge,
                        email: String::new(),
                    });
                }
            }
        });
    };

    // ── Reactive title & subtitle ────────────────────────────────────────
    let title = Signal::derive(move || match page_state.get() {
        PageState::Verifying => "Verifying Recovery Link".to_string(),
        PageState::Ready { .. } => "Create New Passkey".to_string(),
        PageState::Creating => "Creating Passkey".to_string(),
        PageState::Success => "New Passkey Created".to_string(),
        PageState::Error { .. } => "Recovery Failed".to_string(),
    });
    let subtitle = Signal::derive(move || match page_state.get() {
        PageState::Verifying => "Checking that your link is still valid.".to_string(),
        PageState::Ready { .. } => {
            "Your identity is verified. Create a new passkey to regain access.".to_string()
        }
        PageState::Creating => "Setting things up — just a moment.".to_string(),
        PageState::Success => "Your account is recovered. Redirecting to the app...".to_string(),
        PageState::Error { message } => message,
    });

    // ── Render ────────────────────────────────────────────────────────────
    view! {
        <AuthLayout title=title subtitle=subtitle>
            {move || {
                let state = page_state.get();
                match state {
                    PageState::Verifying => verifying_view().into_any(),
                    PageState::Error { .. } => error_view().into_any(),
                    PageState::Success => success_view().into_any(),
                    PageState::Creating => creating_view().into_any(),
                    PageState::Ready { email, .. } => {
                        let current_error = error.get();
                        ready_view(
                            email,
                            current_error,
                            on_create_passkey,
                        ).into_any()
                    }
                }
            }}
        </AuthLayout>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Verifying view
// ─────────────────────────────────────────────────────────────────────────────

fn verifying_view() -> impl IntoView {
    view! {
        <div class="text-center space-y-4">
            // Branded moment (auth page) — DESIGN.md Loading State Pattern
            <img src="/kyomi_animated_logo.svg" alt="Processing" class="w-12 h-12 mx-auto"/>
        </div>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Ready view — create new passkey
// ─────────────────────────────────────────────────────────────────────────────

fn ready_view(
    email: String,
    current_error: Option<String>,
    on_create_passkey: impl Fn(leptos::ev::MouseEvent) + Send + 'static,
) -> impl IntoView {
    view! {
        <div class="space-y-6">
            <div class="text-center">
                <div class="inline-flex items-center justify-center w-16 h-16 rounded-full bg-primary/10 mx-auto mb-6">
                    <Icon icon=phosphor_leptos::KEY attr:class="w-8 h-8 text-primary"/>
                </div>
            </div>
            // Show email if available
                {(!email.is_empty()).then(|| {
                    let email_clone = email.clone();
                    view! {
                        <div class="text-center text-sm text-muted-foreground">
                            "Recovering account: "
                            <span class="font-medium text-foreground">{email_clone}</span>
                        </div>
                    }
                })}

                // Error alert
                {current_error.map(|msg| view! {
                    <Alert variant=AlertVariant::Error>
                        <AlertTitle>"Error"</AlertTitle>
                        <AlertDescription>{msg}</AlertDescription>
                    </Alert>
                })}

                <Button
                    size=ButtonSize::Lg
                    class="w-full"
                    on:click=on_create_passkey
                >
                    <div class="flex items-center justify-center space-x-2">
                        <span>"Create New Passkey"</span>
                    </div>
                </Button>

                <p class="text-xs text-center text-muted-foreground">
                    "You will be prompted to use your fingerprint, face, or security key."
                </p>

            <div class="text-center pt-2 border-t border-border">
                <p class="text-xs text-muted-foreground mt-4">
                    "This recovery session is valid for 15 minutes. After creating your passkey, your old passkeys will remain active."
                </p>
            </div>
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
// Success view
// ─────────────────────────────────────────────────────────────────────────────

fn success_view() -> impl IntoView {
    view! {
        <div class="space-y-4">
            <div class="text-center">
                <div class="inline-flex items-center justify-center w-16 h-16 rounded-full bg-success/10 mx-auto mb-6">
                    <Icon icon=phosphor_leptos::CHECK attr:class="w-8 h-8 text-success-foreground"/>
                </div>
            </div>
            // Branded moment (auth page) — DESIGN.md Loading State Pattern
            <img src="/kyomi_animated_logo.svg" alt="Processing" class="w-8 h-8 mx-auto"/>
        </div>
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
                    <Icon icon=phosphor_leptos::WARNING attr:class="w-8 h-8 text-error-foreground"/>
                </div>
            </div>
            <ButtonLink href="/auth/recover-passkey" variant=ButtonVariant::Default class="w-full">
                "Request New Recovery Link"
            </ButtonLink>
            <ButtonLink href="/login" variant=ButtonVariant::Outline class="w-full">
                "Back to Login"
            </ButtonLink>
        </div>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// WebAuthn error mapping
// ─────────────────────────────────────────────────────────────────────────────

/// Map WebAuthn error strings to user-friendly messages.
///
/// Mirrors the React error handling in PasskeyRecoveryComplete.jsx.
fn map_webauthn_error(error: &str) -> String {
    if error.contains("InvalidStateError") {
        "A passkey already exists for this device. Please try with a different device.".to_string()
    } else if error.contains("NotAllowedError") {
        "Passkey creation was cancelled or timed out. Please try again.".to_string()
    } else if error.contains("AbortError") {
        "Passkey creation was cancelled. Please try again.".to_string()
    } else if error.contains("NotSupportedError") {
        "Your device does not support passkeys. Please contact support for alternative recovery options.".to_string()
    } else {
        format!("Failed to create passkey: {}", error)
    }
}
