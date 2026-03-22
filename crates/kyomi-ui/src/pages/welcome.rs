// SPDX-License-Identifier: AGPL-3.0-or-later

//! Welcome page — terms acceptance before first use.
//!
//! Full-screen centered card (no sidebar). Users land here after OAuth signup
//! or when an existing user needs to re-accept updated terms.
//!
//! Matches `apps/frontend/src/pages/Welcome.jsx` exactly.

use leptos::prelude::*;

use crate::components::{Alert, AlertVariant, Card, Checkbox, Spinner};
use crate::server_fns::onboarding::{accept_terms, AcceptTermsResult};

/// Welcome page — terms acceptance.
///
/// Query parameters:
/// - `temp_token` (required) — temporary token from signup flow
/// - `existing_user` (optional) — when `"true"`, shows "Welcome Back!" heading
///
/// If `temp_token` is missing, redirects to `/login`.
#[component]
pub fn WelcomePage() -> impl IntoView {
    // ── Query parameter extraction ───────────────────────────────────────
    let params = leptos_router::hooks::use_query_map();
    let temp_token = Signal::derive(move || {
        params.read().get("temp_token").unwrap_or_default()
    });
    let is_existing_user = Signal::derive(move || {
        params.read().get("existing_user").as_deref() == Some("true")
    });

    // ── Form state ───────────────────────────────────────────────────────
    let (agreed, set_agreed) = signal(false);
    let (marketing_consent, set_marketing_consent) = signal(false);
    let (loading, set_loading) = signal(false);
    let (error, set_error) = signal(Option::<String>::None);

    // ── Redirect if no temp_token ────────────────────────────────────────
    // Runs when temp_token signal changes (effectively once on mount since query params are static).
    Effect::new(move || {
        if temp_token.get().is_empty() {
            if let Some(window) = web_sys::window() {
                let _ = window.location().set_href("/login");
            }
        }
    });

    // ── Submit handler ───────────────────────────────────────────────────
    let on_submit = move |_| {
        if !agreed.get_untracked() || loading.get_untracked() {
            return;
        }

        let token = temp_token.get_untracked();
        let consent = marketing_consent.get_untracked();

        set_loading.set(true);
        set_error.set(None);

        leptos::task::spawn_local(async move {
            match accept_terms(token, consent).await {
                Ok(AcceptTermsResult::Success) => {
                    // Terms accepted — hard redirect to pick up new auth cookies
                    if let Some(window) = web_sys::window() {
                        let _ = window.location().set_href("/onboarding");
                    }
                }
                Ok(AcceptTermsResult::Error { message }) => {
                    set_error.set(Some(message));
                    set_loading.set(false);
                }
                Err(e) => {
                    set_error.set(Some(format!(
                        "Failed to accept terms. Please try again. ({})",
                        e
                    )));
                    set_loading.set(false);
                }
            }
        });
    };

    // ── Derived: button disabled ─────────────────────────────────────────
    let button_disabled = move || !agreed.get() || loading.get();

    view! {
        <div class="min-h-screen flex items-center justify-center bg-background p-4">
            <Card class="max-w-2xl w-full p-8">
                // ── Heading ──────────────────────────────────────────
                <div class="text-center mb-6">
                    <h1 class="text-3xl font-bold mb-2">
                        {move || {
                            if is_existing_user.get() {
                                "Welcome Back!"
                            } else {
                                "Welcome to Kyomi!"
                            }
                        }}
                    </h1>
                    <p class="text-muted-foreground">
                        {move || {
                            if is_existing_user.get() {
                                "Please review and accept our updated terms to continue."
                            } else {
                                "Before you continue, please review and accept our terms."
                            }
                        }}
                    </p>
                </div>

                // ── Error alert ──────────────────────────────────────
                {move || {
                    error
                        .get()
                        .map(|msg| {
                            view! {
                                <Alert variant=AlertVariant::Error class="mb-6">
                                    {msg}
                                </Alert>
                            }
                        })
                }}

                // ── Checkboxes ───────────────────────────────────────
                <div class="mb-6 space-y-4">
                    // Terms checkbox
                    <label class="flex items-start space-x-3 cursor-pointer">
                        <Checkbox
                            checked=Signal::derive(move || agreed.get())
                            on_change=Callback::new(move |val: bool| set_agreed.set(val))
                            class="mt-1"
                        />
                        <span class="text-sm">
                            "I have read and agree to the "
                            <a
                                href="https://kyomi.ai/terms"
                                target="_blank"
                                rel="noopener noreferrer"
                                class="text-primary hover:underline"
                            >
                                "Terms of Service"
                            </a>
                            " and "
                            <a
                                href="https://kyomi.ai/privacy"
                                target="_blank"
                                rel="noopener noreferrer"
                                class="text-primary hover:underline"
                            >
                                "Privacy Policy"
                            </a>
                        </span>
                    </label>

                    // Marketing consent checkbox
                    <label class="flex items-start space-x-3 cursor-pointer">
                        <Checkbox
                            checked=Signal::derive(move || marketing_consent.get())
                            on_change=Callback::new(move |val: bool| {
                                set_marketing_consent.set(val)
                            })
                            class="mt-1"
                        />
                        <span class="text-sm">
                            "I agree to receive product updates and announcements from Kyomi. You can unsubscribe anytime."
                        </span>
                    </label>
                </div>

                // ── Submit button ────────────────────────────────────
                // Uses a raw <button> (like login.rs) so `disabled` is reactive.
                <button
                    type="button"
                    disabled=button_disabled
                    on:click=on_submit
                    class="inline-flex items-center justify-center gap-2 whitespace-nowrap rounded-md text-sm font-medium transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:pointer-events-none disabled:opacity-50 [&_svg]:pointer-events-none [&_svg]:size-4 [&_svg]:shrink-0 bg-primary text-primary-foreground shadow hover:bg-primary/90 h-10 rounded-md px-8 w-full"
                >
                    {move || {
                        if loading.get() {
                            view! {
                                <span class="flex items-center justify-center gap-2">
                                    <Spinner />
                                    "Please wait..."
                                </span>
                            }
                                .into_any()
                        } else {
                            view! { "Continue to Kyomi" }.into_any()
                        }
                    }}
                </button>
            </Card>
        </div>
    }
}
