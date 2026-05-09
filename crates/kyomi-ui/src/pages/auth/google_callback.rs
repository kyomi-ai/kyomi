// SPDX-License-Identifier: AGPL-3.0-or-later

//! Google OAuth callback page — processes the OAuth redirect and handles the result.
//!
//! Route: `/auth/google/callback?code=xxx&state=xxx`
//!
//! Matches `apps/frontend/src/pages/GoogleLoginCallback.jsx`.
//! Auto-processes the callback on mount. No user interaction needed for the happy path.

use leptos::prelude::*;
#[cfg(target_arch = "wasm32")]
use leptos_router::hooks::use_navigate;
use phosphor_leptos::Icon;
use crate::components::{Button, ButtonLink, ButtonSize, ButtonVariant};
use crate::pages::auth::auth_layout::AuthLayout;

// ─────────────────────────────────────────────────────────────────────────────
// Page state
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq)]
enum CallbackStatus {
    Processing,
    Success,
    Error,
}

/// Outcome of processing the Google OAuth callback. The redirect URL (if any)
/// is returned separately so the caller can handle browser navigation.
struct CallbackOutcome {
    status: CallbackStatus,
    message: String,
    /// Where to redirect after a short delay, if applicable.
    redirect_url: Option<String>,
}

/// Process the OAuth callback result and produce the next page state.
/// Not cfg-gated so the compiler sees all `CallbackStatus` variants constructed.
async fn process_google_callback(
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
) -> CallbackOutcome {
    use crate::server_fns::auth::{google_oauth_callback, GoogleCallbackResult};

    // Check for error param (Google returns this on denial)
    if let Some(err) = error {
        return CallbackOutcome {
            status: CallbackStatus::Error,
            message: format!("Google OAuth error: {err}"),
            redirect_url: None,
        };
    }

    // Validate required params
    let (Some(code), Some(state)) = (code, state) else {
        return CallbackOutcome {
            status: CallbackStatus::Error,
            message: "Missing authorization code or state parameter".to_string(),
            redirect_url: None,
        };
    };

    // Call the server function
    match google_oauth_callback(code, Some(state)).await {
        Ok(GoogleCallbackResult::Success { oauth_continue }) => {
            if let Some(oauth_state) = oauth_continue {
                CallbackOutcome {
                    status: CallbackStatus::Success,
                    message: String::new(),
                    redirect_url: Some(format!(
                        "/api/v1/oauth/authorize/continue?state={oauth_state}"
                    )),
                }
            } else {
                CallbackOutcome {
                    status: CallbackStatus::Success,
                    message: String::new(),
                    redirect_url: Some("/".to_string()),
                }
            }
        }
        Ok(GoogleCallbackResult::PendingTerms { redirect_url }) => {
            let url = if redirect_url.is_empty() {
                "/welcome".to_string()
            } else {
                redirect_url
            };
            CallbackOutcome {
                status: CallbackStatus::Success,
                message: "Please accept our Terms of Service to continue".to_string(),
                redirect_url: Some(url),
            }
        }
        Ok(GoogleCallbackResult::Error { message: msg }) => CallbackOutcome {
            status: CallbackStatus::Error,
            message: msg,
            redirect_url: None,
        },
        Ok(GoogleCallbackResult::RateLimited { retry_after_secs }) => CallbackOutcome {
            status: CallbackStatus::Error,
            message: format!(
                "Too many attempts. Please try again in {retry_after_secs} seconds."
            ),
            redirect_url: None,
        },
        Err(e) => CallbackOutcome {
            status: CallbackStatus::Error,
            message: e
                .to_string()
                .strip_prefix("error running server function: ")
                .unwrap_or(&e.to_string())
                .to_string(),
            redirect_url: None,
        },
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Main component
// ─────────────────────────────────────────────────────────────────────────────

#[component]
pub fn GoogleCallbackPage() -> impl IntoView {
    #[cfg(target_arch = "wasm32")]
    let navigate = use_navigate();
    let (status, set_status) = signal(CallbackStatus::Processing);
    let (message, set_message) = signal(String::from("Signing in with Google"));

    // Help text appears after 10s if the redirect hasn't fired. The timer
    // and signal only exist on the wasm target — on SSR there's nothing to
    // wait for, so we expose a constant `false` Signal of the same shape.
    #[cfg(target_arch = "wasm32")]
    let show_help_text = {
        let (show, set) = signal(false);
        use gloo_timers::callback::Timeout;
        Timeout::new(10_000, move || { set.try_set(true); }).forget();
        Signal::derive(move || show.get())
    };
    #[cfg(not(target_arch = "wasm32"))]
    let show_help_text: Signal<bool> = Signal::derive(|| false);

    // Process the OAuth callback on mount (browser-only: read URL params).
    // Uses the browser's native URLSearchParams — same pattern as the other
    // auth completion pages. The backend-side SSR path gets None defaults.
    #[cfg(target_arch = "wasm32")]
    let (code, state_param, error) = {
        let params = web_sys::window()
            .and_then(|w| w.location().search().ok())
            .and_then(|s| web_sys::UrlSearchParams::new_with_str(&s).ok());
        match params {
            Some(p) => (p.get("code"), p.get("state"), p.get("error")),
            None => (None, None, None),
        }
    };
    #[cfg(not(target_arch = "wasm32"))]
    let (code, state_param, error): (Option<String>, Option<String>, Option<String>) =
        (None, None, None);

    // Cannot use Action: uses gloo_timers::future::TimeoutFuture (timer-based
    // delay before navigation) and window.location().set_href() — both !Send
    // browser APIs on wasm32. Signal writes inside the async block use try_set
    // for deferred-write safety.
    leptos::task::spawn_local(async move {
        let outcome = process_google_callback(code, state_param, error).await;
        set_status.try_set(outcome.status);
        if !outcome.message.is_empty() {
            set_message.try_set(outcome.message);
        }

        // Redirect handling — gloo_timers and web_sys are browser-only,
        // but reading redirect_url must happen on both targets to consume the field.
        if let Some(_redirect_url) = outcome.redirect_url {
            #[cfg(target_arch = "wasm32")]
            {
                let navigate_clone = navigate.clone();
                if _redirect_url.contains("/oauth/authorize/continue") {
                    // API endpoint — must hard-redirect (not within the SPA router)
                    gloo_timers::future::TimeoutFuture::new(500).await;
                    if let Some(window) = web_sys::window() {
                        let _ = window.location().set_href(&_redirect_url);
                    }
                } else {
                    // SPA navigation — keeps WASM in memory
                    gloo_timers::future::TimeoutFuture::new(1500).await;
                    navigate_clone(&_redirect_url, Default::default());
                }
            }
        }
    });

    // ── Reactive title & subtitle ────────────────────────────────────────
    let title = Signal::derive(move || match status.get() {
        CallbackStatus::Error => "Google Sign-In".to_string(),
        _ => "Signing in with Google".to_string(),
    });
    let subtitle = Signal::derive(move || match status.get() {
        CallbackStatus::Error => message.get(),
        _ => "Completing your Google sign-in...".to_string(),
    });

    view! {
        <AuthLayout title=title subtitle=subtitle>
            <div class="text-center space-y-4">
                // Status icon
                <div class="flex justify-center">
                    {move || {
                        if status.get() == CallbackStatus::Error {
                            view! {
                                <Icon
                                    icon=phosphor_leptos::X_CIRCLE
                                    size="48px"
                                    attr:class="text-error-foreground"
                                />
                            }
                                .into_any()
                        } else {
                            // Branded moment (auth page) — DESIGN.md Loading State Pattern
                            view! {
                                <img
                                    src="/kyomi_animated_logo.svg"
                                    alt="Processing"
                                    class="w-12 h-12"
                                />
                            }
                                .into_any()
                        }
                    }}
                </div>

                // Error state content
                {move || {
                    if status.get() == CallbackStatus::Error {
                        Some(
                            view! {
                                <div class="space-y-3">
                                    <ButtonLink
                                        href="/login"
                                        variant=ButtonVariant::Outline
                                        size=ButtonSize::Lg
                                        class="w-full"
                                    >
                                        "Return to Login"
                                    </ButtonLink>
                                    <Button
                                        variant=ButtonVariant::Default
                                        size=ButtonSize::Lg
                                        class="w-full"
                                        on:click=move |_| {
                                            #[cfg(target_arch = "wasm32")]
                                            {
                                                if let Some(window) = web_sys::window() {
                                                    let _ = window.location().reload();
                                                }
                                            }
                                        }
                                    >
                                        "Try Again"
                                    </Button>
                                </div>
                            },
                        )
                    } else {
                        None
                    }
                }}

                // Help text — shown after 10 seconds if not redirected
                {move || {
                    if show_help_text.get() && status.get() != CallbackStatus::Error {
                        Some(
                            view! {
                                <div class="mt-6 text-center text-sm text-muted-foreground">
                                    <p>
                                        "If this page doesn't automatically redirect, you can close it and return to login."
                                    </p>
                                </div>
                            },
                        )
                    } else {
                        None
                    }
                }}
            </div>
        </AuthLayout>
    }
}
