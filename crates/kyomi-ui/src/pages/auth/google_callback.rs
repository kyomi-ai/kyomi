// SPDX-License-Identifier: AGPL-3.0-or-later

//! Google OAuth callback page — processes the OAuth redirect and handles the result.
//!
//! Route: `/auth/google/callback?code=xxx&state=xxx`
//!
//! Matches `apps/frontend/src/pages/GoogleLoginCallback.jsx`.
//! Auto-processes the callback on mount. No user interaction needed for the happy path.

use leptos::prelude::*;

#[cfg(target_arch = "wasm32")]
use crate::server_fns::auth::{google_oauth_callback, GoogleCallbackResult};

// ─────────────────────────────────────────────────────────────────────────────
// Page state
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq)]
enum CallbackStatus {
    Processing,
    Success,
    Error,
}

// ─────────────────────────────────────────────────────────────────────────────
// Query param helper
// ─────────────────────────────────────────────────────────────────────────────

/// Extract a query parameter value from a `?key=val&key2=val2` search string.
fn get_query_param(search: &str, key: &str) -> Option<String> {
    let search = search.strip_prefix('?').unwrap_or(search);
    for pair in search.split('&') {
        if let Some((k, v)) = pair.split_once('=') {
            if k == key {
                // URL-decode the value (percent-encoded)
                return Some(
                    percent_decode(v),
                );
            }
        }
    }
    None
}

/// Minimal percent-decoding for URL query values.
fn percent_decode(input: &str) -> String {
    let mut result = String::with_capacity(input.len());
    let mut chars = input.as_bytes().iter();
    while let Some(&b) = chars.next() {
        if b == b'%' {
            let hi = chars.next().copied();
            let lo = chars.next().copied();
            if let (Some(h), Some(l)) = (hi, lo) {
                if let (Some(hv), Some(lv)) = (hex_val(h), hex_val(l)) {
                    result.push((hv << 4 | lv) as char);
                    continue;
                }
            }
            // Malformed — emit literal
            result.push('%');
        } else if b == b'+' {
            result.push(' ');
        } else {
            result.push(b as char);
        }
    }
    result
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Main component
// ─────────────────────────────────────────────────────────────────────────────

#[component]
pub fn GoogleCallbackPage() -> impl IntoView {
    let (status, set_status) = signal(CallbackStatus::Processing);
    let (message, set_message) = signal(String::from("Signing in with Google"));
    let (show_help_text, set_show_help_text) = signal(false);

    // Show help text after 10 seconds if redirect hasn't happened
    #[cfg(target_arch = "wasm32")]
    {
        use gloo_timers::callback::Timeout;
        let timeout = Timeout::new(10_000, move || {
            set_show_help_text.set(true);
        });
        // Prevent the timeout from being cancelled by drop
        timeout.forget();
    }

    // Process the OAuth callback on mount
    #[cfg(target_arch = "wasm32")]
    {
        leptos::task::spawn_local(async move {
            let (code, state, error) = {
                let search = web_sys::window()
                    .and_then(|w| w.location().search().ok())
                    .unwrap_or_default();
                (
                    get_query_param(&search, "code"),
                    get_query_param(&search, "state"),
                    get_query_param(&search, "error"),
                )
            };

            // Check for error param (Google returns this on denial)
            if let Some(err) = error {
                set_status.set(CallbackStatus::Error);
                set_message.set(format!("Google OAuth error: {err}"));
                return;
            }

            // Validate required params
            let (Some(code), Some(state)) = (code, state) else {
                set_status.set(CallbackStatus::Error);
                set_message.set("Missing authorization code or state parameter".to_string());
                return;
            };

            // Call the server function
            match google_oauth_callback(code, Some(state)).await {
                Ok(GoogleCallbackResult::Success { oauth_continue }) => {
                    set_status.set(CallbackStatus::Success);

                    if let Some(oauth_state) = oauth_continue {
                        // Continue MCP OAuth flow
                        let url = format!(
                            "/api/v1/oauth/authorize/continue?state={oauth_state}"
                        );
                        gloo_timers::future::TimeoutFuture::new(500).await;
                        if let Some(window) = web_sys::window() {
                            let _ = window.location().set_href(&url);
                        }
                    } else {
                        // Existing user with terms accepted — go straight to home
                        gloo_timers::future::TimeoutFuture::new(1500).await;
                        if let Some(window) = web_sys::window() {
                            let _ = window.location().set_href("/");
                        }
                    }
                }
                Ok(GoogleCallbackResult::PendingTerms { redirect_url }) => {
                    set_status.set(CallbackStatus::Success);
                    set_message
                        .set("Please accept our Terms of Service to continue".to_string());

                    gloo_timers::future::TimeoutFuture::new(1500).await;
                    if let Some(window) = web_sys::window() {
                        let url = if redirect_url.is_empty() {
                            "/welcome".to_string()
                        } else {
                            redirect_url
                        };
                        let _ = window.location().set_href(&url);
                    }
                }
                Ok(GoogleCallbackResult::Error { message: msg }) => {
                    set_status.set(CallbackStatus::Error);
                    set_message.set(msg);
                }
                Ok(GoogleCallbackResult::RateLimited { retry_after_secs }) => {
                    set_status.set(CallbackStatus::Error);
                    set_message.set(format!(
                        "Too many attempts. Please try again in {retry_after_secs} seconds."
                    ));
                }
                Err(e) => {
                    set_status.set(CallbackStatus::Error);
                    set_message.set(
                        e.to_string()
                            .strip_prefix("error running server function: ")
                            .unwrap_or(&e.to_string())
                            .to_string(),
                    );
                }
            }
        });
    }

    // ── SSR fallback (should never be seen in practice) ──────────────────
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = (&set_status, &set_message, &set_show_help_text);
    }

    view! {
        <div class="min-h-screen bg-background flex items-center justify-center px-4">
            <div class="max-w-md w-full">
                <div class="bg-background p-8 text-center">
                    // Status icon
                    <div class="flex justify-center">
                        {move || {
                            if status.get() == CallbackStatus::Error {
                                // XCircle SVG — Lucide icon
                                view! {
                                    <svg
                                        class="text-error-foreground"
                                        xmlns="http://www.w3.org/2000/svg"
                                        width="48"
                                        height="48"
                                        viewBox="0 0 24 24"
                                        fill="none"
                                        stroke="currentColor"
                                        stroke-width="2"
                                        stroke-linecap="round"
                                        stroke-linejoin="round"
                                    >
                                        <circle cx="12" cy="12" r="10"/>
                                        <path d="m15 9-6 6"/>
                                        <path d="m9 9 6 6"/>
                                    </svg>
                                }
                                    .into_any()
                            } else {
                                // Animated Kyomi logo during processing/success
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
                                    <h2 class="text-xl font-semibold text-foreground mb-2 mt-4">
                                        "Google Sign-In"
                                    </h2>
                                    <p class="text-error-foreground mb-4">{move || message.get()}</p>
                                    <div class="space-y-3">
                                        <a
                                            href="/login"
                                            class="block w-full px-4 py-2 bg-muted-foreground text-primary-foreground rounded-xl hover:bg-foreground transition-colors"
                                        >
                                            "Return to Login"
                                        </a>
                                        <button
                                            on:click=move |_| {
                                                #[cfg(target_arch = "wasm32")]
                                                {
                                                    if let Some(window) = web_sys::window() {
                                                        let _ = window.location().reload();
                                                    }
                                                }
                                            }
                                            class="w-full px-4 py-2 bg-primary text-white rounded-xl hover:bg-primary/90 transition-colors"
                                        >
                                            "Try Again"
                                        </button>
                                    </div>
                                },
                            )
                        } else {
                            None
                        }
                    }}
                </div>

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
        </div>
    }
}
