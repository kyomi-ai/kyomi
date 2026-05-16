// SPDX-License-Identifier: AGPL-3.0-or-later

//! Google account link callback page — processes the OAuth redirect after BigQuery consent.
//!
//! Route: `/auth/google/link-callback?code=xxx&state=xxx`
//!
//! Google redirects here after the user completes account linking consent.
//! Auto-processes the callback on mount. On success, redirects to Settings
//! (profile tab) so the user sees the newly-linked Google account.

use leptos::prelude::*;
#[cfg(target_arch = "wasm32")]
use leptos_router::hooks::use_navigate;
use phosphor_leptos::Icon;
use crate::components::{ButtonLink, ButtonSize, ButtonVariant};
use crate::pages::auth::auth_layout::AuthLayout;

// ─────────────────────────────────────────────────────────────────────────────
// Page state
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq)]
enum LinkStatus {
    Processing,
    Success,
    Error,
}

/// Outcome of processing the Google account link callback.
struct LinkOutcome {
    status: LinkStatus,
    message: String,
    /// Where to redirect after a short delay, if applicable.
    redirect_url: Option<String>,
}

/// Process the link callback result and produce the next page state.
/// Not cfg-gated so the compiler sees all `LinkStatus` variants constructed.
async fn process_google_link_callback(
    code: Option<String>,
    state_param: Option<String>,
    error: Option<String>,
) -> LinkOutcome {
    use crate::server_fns::auth::{google_link_callback, GoogleLinkCallbackResult};

    // Check for error param (Google returns this on denial, e.g. access_denied)
    if let Some(err) = error {
        return LinkOutcome {
            status: LinkStatus::Error,
            message: format!("Google OAuth error: {err}"),
            redirect_url: None,
        };
    }

    // Validate required params — both code and state are mandatory
    let (Some(code), Some(state_val)) = (code, state_param) else {
        return LinkOutcome {
            status: LinkStatus::Error,
            message: "Missing authorization code or state parameter".to_string(),
            redirect_url: None,
        };
    };

    // Call the server function
    match google_link_callback(code, state_val).await {
        Ok(GoogleLinkCallbackResult::Success { .. }) => LinkOutcome {
            status: LinkStatus::Success,
            message: "Your Google account has been linked for BigQuery access.".to_string(),
            redirect_url: Some("/settings?tab=profile&google=connected".to_string()),
        },
        Ok(GoogleLinkCallbackResult::Error { message: msg }) => LinkOutcome {
            status: LinkStatus::Error,
            message: msg,
            redirect_url: None,
        },
        Err(e) => LinkOutcome {
            status: LinkStatus::Error,
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
pub fn GoogleLinkCallbackPage() -> impl IntoView {
    #[cfg(target_arch = "wasm32")]
    let navigate = use_navigate();
    let (status, set_status) = signal(LinkStatus::Processing);
    let (message, set_message) = signal(String::from("Linking Google Account"));

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

    // Process the link callback on mount (browser-only: read URL params).
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
    // delay before navigation) and use_navigate() — both !Send browser APIs on
    // wasm32. Signal writes inside the async block use try_set for
    // deferred-write safety.
    leptos::task::spawn_local(async move {
        let outcome = process_google_link_callback(code, state_param, error).await;
        set_status.try_set(outcome.status);
        if !outcome.message.is_empty() {
            set_message.try_set(outcome.message);
        }

        // Redirect handling — gloo_timers and use_navigate are browser-only,
        // but reading redirect_url must happen on both targets to consume the field.
        if let Some(_redirect_url) = outcome.redirect_url {
            #[cfg(target_arch = "wasm32")]
            {
                let navigate_clone = navigate.clone();
                // SPA navigation — keeps WASM in memory, short delay for success feedback
                gloo_timers::future::TimeoutFuture::new(1500).await;
                navigate_clone(&_redirect_url, Default::default());
            }
        }
    });

    // ── Reactive title & subtitle ────────────────────────────────────────
    let title = Signal::derive(move || match status.get() {
        LinkStatus::Processing => "Linking Google Account".to_string(),
        LinkStatus::Success => "Google Account Linked".to_string(),
        LinkStatus::Error => "Link Failed".to_string(),
    });
    let subtitle = Signal::derive(move || match status.get() {
        LinkStatus::Processing => "Completing your Google account link...".to_string(),
        LinkStatus::Success => message.get(),
        LinkStatus::Error => message.get(),
    });

    view! {
        <AuthLayout title=title subtitle=subtitle>
            <div class="text-center space-y-4">
                // Status icon
                <div class="flex justify-center">
                    {move || {
                        if status.get() == LinkStatus::Error {
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
                    if status.get() == LinkStatus::Error {
                        Some(
                            view! {
                                <div class="space-y-3">
                                    <ButtonLink
                                        href="/settings?tab=profile"
                                        variant=ButtonVariant::Outline
                                        size=ButtonSize::Lg
                                        class="w-full"
                                    >
                                        "Return to Settings"
                                    </ButtonLink>
                                </div>
                            },
                        )
                    } else {
                        None
                    }
                }}

                // Help text — shown after 10 seconds if not redirected
                {move || {
                    if show_help_text.get() && status.get() == LinkStatus::Processing {
                        Some(
                            view! {
                                <div class="mt-6 text-center text-sm text-muted-foreground">
                                    <p>
                                        "If this page doesn't automatically redirect, you can return to settings manually."
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
