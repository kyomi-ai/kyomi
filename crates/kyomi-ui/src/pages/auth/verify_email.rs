// SPDX-License-Identifier: AGPL-3.0-or-later

//! Email verification page — processes the token from a verification email link.
//!
//! Route: `/verify-email?token=xxx`
//!
//! The verification email includes a link like `{frontend_url}/verify-email?token={raw_token}`.
//! This page extracts the token, calls the `verify_email` server fn, marks the
//! user as verified, and redirects to `/login` after a short success delay.
//!
//! State machine: Processing → Success | Error

use leptos::prelude::*;
#[cfg(target_arch = "wasm32")]
use leptos_router::hooks::use_navigate;
use phosphor_leptos::{Icon, IconWeight};
use crate::components::{Button, ButtonLink, ButtonSize, ButtonVariant};
use crate::pages::auth::auth_layout::AuthLayout;

// ─────────────────────────────────────────────────────────────────────────────
// Page state
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq)]
enum VerifyState {
    Processing,
    Success,
    Error { message: String },
}

// ─────────────────────────────────────────────────────────────────────────────
// Main component
// ─────────────────────────────────────────────────────────────────────────────

#[component]
pub fn VerifyEmailPage() -> impl IntoView {
    #[cfg(target_arch = "wasm32")]
    let navigate = use_navigate();
    let (state, set_state) = signal(VerifyState::Processing);

    // Help text timer — appears after 10 s if the page is stuck in Processing.
    // Browser-only: only meaningful when waiting on a real async call.
    #[cfg(target_arch = "wasm32")]
    let show_help_text = {
        let (show, set) = signal(false);
        use gloo_timers::callback::Timeout;
        Timeout::new(10_000, move || {
            set.try_set(true);
        })
        .forget();
        Signal::derive(move || show.get())
    };
    #[cfg(not(target_arch = "wasm32"))]
    let show_help_text: Signal<bool> = Signal::derive(|| false);

    // Extract token from URL query params.
    // Browser-only: the server-side render provides None so the compiler sees
    // all VerifyState variants constructed on both targets.
    #[cfg(target_arch = "wasm32")]
    let token: Option<String> = {
        let params = web_sys::window()
            .and_then(|w| w.location().search().ok())
            .and_then(|s| web_sys::UrlSearchParams::new_with_str(&s).ok());
        match params {
            Some(p) => p.get("token"),
            None => None,
        }
    };
    #[cfg(not(target_arch = "wasm32"))]
    let token: Option<String> = None;

    // Cannot use Action: gloo_timers::future::TimeoutFuture and use_navigate()
    // are !Send browser APIs. Signal writes inside the async block use try_set
    // for deferred-write safety.
    leptos::task::spawn_local(async move {
        use crate::server_fns::auth::{verify_email, VerifyEmailResult};

        let Some(tok) = token else {
            set_state.try_set(VerifyState::Error {
                message: "No verification token provided.".to_string(),
            });
            return;
        };

        match verify_email(tok).await {
            Ok(VerifyEmailResult::Success { .. }) => {
                set_state.try_set(VerifyState::Success);

                #[cfg(target_arch = "wasm32")]
                {
                    let navigate_clone = navigate.clone();
                    gloo_timers::future::TimeoutFuture::new(3000).await;
                    navigate_clone("/login", Default::default());
                }
            }
            Ok(VerifyEmailResult::InvalidToken) => {
                set_state.try_set(VerifyState::Error {
                    message: "The verification link is invalid or has expired.".to_string(),
                });
            }
            Ok(VerifyEmailResult::Error { message }) => {
                set_state.try_set(VerifyState::Error { message });
            }
            Err(e) => {
                set_state.try_set(VerifyState::Error {
                    message: e
                        .to_string()
                        .strip_prefix("error running server function: ")
                        .unwrap_or(&e.to_string())
                        .to_string(),
                });
            }
        }
    });

    // ── Reactive title & subtitle ────────────────────────────────────────
    let title = Signal::derive(move || match state.get() {
        VerifyState::Processing => "Verifying Email".to_string(),
        VerifyState::Success => "Email Verified".to_string(),
        VerifyState::Error { .. } => "Verification Failed".to_string(),
    });
    let subtitle = Signal::derive(move || match state.get() {
        VerifyState::Processing => "Completing your email verification...".to_string(),
        VerifyState::Success => "Your email has been verified successfully.".to_string(),
        VerifyState::Error { .. } => {
            "The verification link is invalid or has expired.".to_string()
        }
    });

    view! {
        <AuthLayout title=title subtitle=subtitle>
            <div class="text-center space-y-6">
                // Status icon
                <div class="flex justify-center">
                    {move || match state.get() {
                        VerifyState::Processing => view! {
                            // Branded moment (auth page) — DESIGN.md Loading State Pattern
                            <img
                                src="/kyomi_animated_logo.svg"
                                alt="Processing"
                                class="w-12 h-12"
                            />
                        }
                        .into_any(),
                        VerifyState::Success => view! {
                            <div class="inline-flex items-center justify-center w-16 h-16 rounded-full bg-success/10">
                                <Icon
                                    icon=phosphor_leptos::CHECK_CIRCLE
                                    weight=IconWeight::Fill
                                    attr:class="w-8 h-8 text-success-foreground"
                                />
                            </div>
                        }
                        .into_any(),
                        VerifyState::Error { .. } => view! {
                            <div class="inline-flex items-center justify-center w-16 h-16 rounded-full bg-error/10">
                                <Icon
                                    icon=phosphor_leptos::X_CIRCLE
                                    weight=IconWeight::Fill
                                    attr:class="w-8 h-8 text-error-foreground"
                                />
                            </div>
                        }
                        .into_any(),
                    }}
                </div>

                // Success state content
                {move || {
                    if state.get() == VerifyState::Success {
                        Some(view! {
                            <div class="space-y-4">
                                <p class="text-sm text-muted-foreground">
                                    "You can now sign in to your account."
                                </p>
                                <ButtonLink
                                    href="/login"
                                    size=ButtonSize::Lg
                                    class="w-full"
                                >
                                    "Continue to Login"
                                </ButtonLink>
                            </div>
                        })
                    } else {
                        None
                    }
                }}

                // Error state content
                {move || match state.get() {
                    VerifyState::Error { .. } => Some(view! {
                        <div class="space-y-3">
                            <ButtonLink
                                href="/signup"
                                variant=ButtonVariant::Outline
                                size=ButtonSize::Lg
                                class="w-full"
                            >
                                "Return to Sign Up"
                            </ButtonLink>
                            <Button
                                variant=ButtonVariant::Ghost
                                size=ButtonSize::Lg
                                class="w-full"
                                on:click=move |_| {
                                    #[cfg(target_arch = "wasm32")]
                                    {
                                        if let Some(w) = web_sys::window() {
                                            let _ = w.location().reload();
                                        }
                                    }
                                }
                            >
                                "Try Again"
                            </Button>
                        </div>
                    }),
                    _ => None,
                }}

                // Help text — shown after 10 s if stuck in Processing state
                {move || {
                    if show_help_text.get() && state.get() == VerifyState::Processing {
                        Some(view! {
                            <div class="mt-6 text-center text-sm text-muted-foreground">
                                <p>
                                    "If this page doesn't automatically redirect, you can return to sign up manually."
                                </p>
                            </div>
                        })
                    } else {
                        None
                    }
                }}
            </div>
        </AuthLayout>
    }
}
