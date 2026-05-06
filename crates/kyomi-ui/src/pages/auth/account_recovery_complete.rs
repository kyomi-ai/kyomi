// SPDX-License-Identifier: AGPL-3.0-or-later

//! Account recovery completion page — matches `apps/frontend/src/pages/AccountRecoveryComplete.jsx`.
//!
//! Route: `/account/recover/complete?token=xxx`
//!
//! Flow:
//! 1. User clicks recovery link from email
//! 2. This page verifies the recovery token with backend
//! 3. On success, shows "Set new password" form
//! 4. User sets a new password
//! 5. On success, user is logged in and redirected to home
//!
//! State machine: Verifying | Ready | Submitting | Success | Error

use leptos::prelude::*;
#[cfg(target_arch = "wasm32")]
use leptos_router::hooks::use_navigate;
use phosphor_leptos::Icon;
use crate::components::{
    Alert, AlertDescription, AlertTitle, AlertVariant, Button, ButtonLink, ButtonSize,
    ButtonVariant, Label, Spinner, INPUT_CLASS,
};
use crate::pages::auth::auth_layout::AuthLayout;
use crate::server_fns::auth::{
    recovery_set_password, recovery_verify, RecoverySetPasswordResult, RecoveryVerifyResult,
};

// ─────────────────────────────────────────────────────────────────────────────
// View state machine
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq)]
enum PageState {
    Verifying,
    Ready {
        recovery_session_id: String,
        has_passkeys: bool,
    },
    Submitting {
        recovery_session_id: String,
        has_passkeys: bool,
    },
    Success,
    Error {
        message: String,
    },
}

/// Verify the recovery token and return the resulting page state.
/// Not cfg-gated so the compiler sees all `PageState` variants constructed.
async fn verify_recovery_token(token: Option<String>) -> PageState {
    let Some(token) = token else {
        return PageState::Error {
            message: "Missing recovery token. Please use the link from your email.".to_string(),
        };
    };
    match recovery_verify(token).await {
        Ok(RecoveryVerifyResult::Success {
            recovery_session_id,
            has_passkeys,
        }) => PageState::Ready {
            recovery_session_id,
            has_passkeys,
        },
        Ok(RecoveryVerifyResult::Error { message }) => PageState::Error { message },
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
pub fn AccountRecoveryCompletePage() -> impl IntoView {
    // ── SPA navigation handle (wasm32 only — only used in wasm async context) ─
    // Wrapped in StoredValue so it can be copied into FnMut closures (view! reactive closures).
    #[cfg(target_arch = "wasm32")]
    let navigate = StoredValue::new(use_navigate());

    let (page_state, set_page_state) = signal(PageState::Verifying);

    // ── Form signals ─────────────────────────────────────────────────────
    let (new_password, set_new_password) = signal(String::new());
    let (confirm_password, set_confirm_password) = signal(String::new());
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

    // spawn_local compiles on both targets; the extracted function ensures
    // the compiler sees all PageState variants constructed.
    leptos::task::spawn_local(async move {
        set_page_state.set(verify_recovery_token(initial_token).await);
    });

    // ── Form submit handler ──────────────────────────────────────────────
    let on_submit = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();

        let current_password = new_password.get_untracked();
        let current_confirm = confirm_password.get_untracked();

        // Client-side validation
        if current_password.len() < 8 {
            set_error.set(Some(
                "Password must be at least 8 characters long.".to_string(),
            ));
            return;
        }
        if current_password != current_confirm {
            set_error.set(Some("Passwords do not match.".to_string()));
            return;
        }

        // Extract recovery_session_id from current state
        let current_state = page_state.get_untracked();
        let (recovery_session_id, has_passkeys) = match current_state {
            PageState::Ready {
                recovery_session_id,
                has_passkeys,
            } => (recovery_session_id, has_passkeys),
            _ => return,
        };

        set_error.set(None);
        set_page_state.set(PageState::Submitting {
            recovery_session_id: recovery_session_id.clone(),
            has_passkeys,
        });

        leptos::task::spawn_local(async move {
            match recovery_set_password(recovery_session_id.clone(), current_password).await {
                Ok(RecoverySetPasswordResult::Success) => {
                    set_page_state.set(PageState::Success);

                    // Navigate to home after 2 seconds (keeps WASM in memory)
                    #[cfg(target_arch = "wasm32")]
                    {
                        let nav = navigate.get_value();
                        gloo_timers::future::TimeoutFuture::new(2000).await;
                        nav("/", Default::default());
                    }
                }
                Ok(RecoverySetPasswordResult::Error { message }) => {
                    set_error.set(Some(message));
                    set_page_state.set(PageState::Ready {
                        recovery_session_id,
                        has_passkeys,
                    });
                }
                Err(e) => {
                    set_error.set(Some(format!("Failed to set password. Please try again. ({})", e)));
                    set_page_state.set(PageState::Ready {
                        recovery_session_id,
                        has_passkeys,
                    });
                }
            }
        });
    };

    // ── Reactive title & subtitle ────────────────────────────────────────
    let title = Signal::derive(move || match page_state.get() {
        PageState::Verifying => "Verifying Recovery Link".to_string(),
        PageState::Ready { .. } | PageState::Submitting { .. } => {
            "Set a New Password".to_string()
        }
        PageState::Success => "Password Updated".to_string(),
        PageState::Error { .. } => "Recovery Failed".to_string(),
    });
    let subtitle = Signal::derive(move || match page_state.get() {
        PageState::Verifying => "Checking that your link is still valid.".to_string(),
        PageState::Ready { .. } | PageState::Submitting { .. } => {
            "Choose a new password for your account.".to_string()
        }
        PageState::Success => "You're signed in. Redirecting to the app...".to_string(),
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
                    PageState::Ready { has_passkeys, .. }
                    | PageState::Submitting { has_passkeys, .. } => {
                        let is_submitting = matches!(page_state.get(), PageState::Submitting { .. });
                        let current_new_password = new_password.get();
                        let current_confirm_password = confirm_password.get();
                        let btn_disabled = is_submitting
                            || current_new_password.is_empty()
                            || current_confirm_password.is_empty();
                        let current_error = error.get();

                        ready_view(ReadyViewParams {
                            has_passkeys,
                            is_submitting,
                            btn_disabled,
                            current_error,
                            new_password,
                            set_new_password,
                            confirm_password,
                            set_confirm_password,
                            on_submit,
                        })
                            .into_any()
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
// Ready view (password form)
// ─────────────────────────────────────────────────────────────────────────────

struct ReadyViewParams<F>
where
    F: Fn(leptos::ev::SubmitEvent) + Send + 'static,
{
    has_passkeys: bool,
    is_submitting: bool,
    btn_disabled: bool,
    current_error: Option<String>,
    new_password: ReadSignal<String>,
    set_new_password: WriteSignal<String>,
    confirm_password: ReadSignal<String>,
    set_confirm_password: WriteSignal<String>,
    on_submit: F,
}

fn ready_view<F>(params: ReadyViewParams<F>) -> impl IntoView
where
    F: Fn(leptos::ev::SubmitEvent) + Send + 'static,
{
    let ReadyViewParams {
        has_passkeys,
        is_submitting,
        btn_disabled,
        current_error,
        new_password,
        set_new_password,
        confirm_password,
        set_confirm_password,
        on_submit,
    } = params;
    view! {
        <div>
            <div class="text-center">
                <div class="inline-flex items-center justify-center w-16 h-16 rounded-full bg-primary/10 mx-auto mb-6">
                    <Icon icon=phosphor_leptos::LOCK_KEY attr:class="w-8 h-8 text-primary"/>
                </div>
            </div>
            <form on:submit=on_submit class="space-y-4">
                    // Passkeys info alert
                    {has_passkeys.then(|| view! {
                        <Alert>
                            <AlertTitle>"Passkeys available"</AlertTitle>
                            <AlertDescription>
                                "Your account also has passkeys registered. You can continue to use them after setting a new password."
                            </AlertDescription>
                        </Alert>
                    })}

                    // Error alert
                    {current_error.map(|msg| view! {
                        <Alert variant=AlertVariant::Error>
                            <AlertDescription>{msg}</AlertDescription>
                        </Alert>
                    })}

                    // New password input
                    <div class="space-y-2">
                        <Label html_for="new-password">"New password"</Label>
                        <input
                            id="new-password"
                            type="password"
                            autocomplete="new-password"
                            autofocus
                            class=INPUT_CLASS
                            placeholder="At least 8 characters"
                            minlength="8"
                            required
                            prop:value=move || new_password.get()
                            on:input=move |ev| set_new_password.set(event_target_value(&ev))
                        />
                    </div>

                    // Confirm password input
                    <div class="space-y-2">
                        <Label html_for="confirm-password">"Confirm password"</Label>
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

                    // Submit button
                    <Button size=ButtonSize::Lg class="w-full" disabled=btn_disabled>
                        {if is_submitting {
                            view! {
                                <div class="flex items-center justify-center space-x-2">
                                    <Spinner class="text-primary-foreground"/>
                                    <span>"Setting password..."</span>
                                </div>
                            }.into_any()
                        } else {
                            view! { <span>"Set New Password"</span> }.into_any()
                        }}
                    </Button>

                // Session validity note
                <div class="text-center pt-2 border-t border-border">
                    <p class="text-xs text-muted-foreground mt-4">
                        "This recovery session is valid for 15 minutes."
                    </p>
                </div>
            </form>
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
            <ButtonLink href="/account/recover" variant=ButtonVariant::Default class="w-full">
                "Request New Recovery Link"
            </ButtonLink>
            <ButtonLink href="/login" variant=ButtonVariant::Outline class="w-full">
                "Back to Login"
            </ButtonLink>
        </div>
    }
}
