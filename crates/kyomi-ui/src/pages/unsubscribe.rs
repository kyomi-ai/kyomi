// SPDX-License-Identifier: AGPL-3.0-or-later

//! Unsubscribe page — public, no auth required.
//!
//! Matches `apps/frontend/src/pages/Unsubscribe.jsx` exactly.
//! Full-screen centered layout with email input, success state, and error handling.

use leptos::prelude::*;
use leptos_router::hooks::use_query_map;

use crate::components::{Alert, AlertDescription, AlertTitle, AlertVariant, Spinner};
use crate::server_fns::unsubscribe::unsubscribe_email;

/// Unsubscribe page — full-screen centered, no sidebar, no auth.
///
/// Pre-fills email from `?email=` query parameter. Shows a success message
/// after unsubscribe, or an error alert on failure with retry capability.
#[component]
pub fn UnsubscribePage() -> impl IntoView {
    let query = use_query_map();

    // Pre-fill email from query parameter.
    let initial_email = query
        .get_untracked()
        .get("email")
        .unwrap_or_default();

    let (email, set_email) = signal(initial_email);
    let (loading, set_loading) = signal(false);
    let (success, set_success) = signal(false);
    let (error, set_error) = signal(Option::<String>::None);

    // Derived: disable submit when email is empty or loading.
    let submit_disabled = Memo::new(move |_| {
        loading.get() || email.get().trim().is_empty()
    });

    // Handle unsubscribe form submission.
    let on_submit = move |ev: web_sys::SubmitEvent| {
        ev.prevent_default();
        set_loading.set(true);
        set_error.set(None);

        let email_val = email.get();
        leptos::task::spawn_local(async move {
            match unsubscribe_email(email_val).await {
                Ok(()) => {
                    set_success.set(true);
                }
                Err(e) => {
                    set_error.set(Some(
                        e.to_string()
                            .strip_prefix("error running server function: ")
                            .unwrap_or(&e.to_string())
                            .to_string(),
                    ));
                }
            }
            set_loading.set(false);
        });
    };

    view! {
        <div class="min-h-screen bg-background flex items-center justify-center p-8">
            <div class="w-full max-w-md">
                // Header with logo and title
                <div class="text-center mb-8">
                    <img
                        src="/kyomi_full_logo.svg"
                        alt="Kyomi"
                        class="h-12 mx-auto mb-6 dark:hidden"
                    />
                    <img
                        src="/kyomi_full_logo_white.svg"
                        alt="Kyomi"
                        class="h-12 mx-auto mb-6 hidden dark:block"
                    />
                    <h1 class="text-3xl font-bold text-foreground mb-2">
                        "Unsubscribe from Updates"
                    </h1>
                    <p class="text-muted-foreground">
                        "We\u{2019}re sorry to see you go"
                    </p>
                </div>

                // Content area: success state or form
                <div class="space-y-6">
                    <Show
                        when=move || success.get()
                        fallback=move || {
                            view! {
                                <form on:submit=on_submit class="space-y-5">
                                    <div>
                                        <label
                                            for="email"
                                            class="block text-sm font-semibold text-foreground mb-3"
                                        >
                                            "Email Address"
                                        </label>
                                        <input
                                            id="email"
                                            name="email"
                                            type="email"
                                            autocomplete="email"
                                            class="w-full px-4 py-3.5 bg-muted border border-border rounded-xl text-foreground placeholder-muted-foreground focus:outline-none focus:ring-2 focus:ring-primary focus:border-transparent transition-all duration-200 hover:bg-background"
                                            prop:value=move || email.get()
                                            on:input=move |ev| {
                                                set_email
                                                    .set(event_target_value(&ev))
                                            }
                                            placeholder="you@company.com"
                                            required=true
                                        />
                                    </div>

                                    // Error alert
                                    <Show when=move || error.get().is_some()>
                                        <Alert variant=AlertVariant::Error>
                                            <AlertDescription>
                                                {move || {
                                                    error.get().unwrap_or_default()
                                                }}
                                            </AlertDescription>
                                        </Alert>
                                    </Show>

                                    // Submit button — raw <button> for reactive disabled prop
                                    // Classes from Button component: BASE + Default variant + Lg size
                                    <button
                                        type="submit"
                                        disabled=submit_disabled
                                        class="inline-flex items-center justify-center gap-2 whitespace-nowrap rounded-md text-sm font-medium transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:pointer-events-none disabled:opacity-50 [&_svg]:pointer-events-none [&_svg]:size-4 [&_svg]:shrink-0 bg-primary text-primary-foreground shadow hover:bg-primary/90 h-9 px-4 py-2 w-full"
                                    >
                                        <Show
                                            when=move || loading.get()
                                            fallback=|| {
                                                view! { "Unsubscribe" }
                                            }
                                        >
                                            <Spinner />
                                            "Unsubscribing..."
                                        </Show>
                                    </button>

                                    // "Never mind" link
                                    <div class="text-center">
                                        <a
                                            href="/"
                                            class="text-sm text-muted-foreground hover:text-foreground transition-colors"
                                        >
                                            "Never mind, take me back"
                                        </a>
                                    </div>
                                </form>
                            }
                        }
                    >
                        // Success state
                        <Alert variant=AlertVariant::Success>
                            <AlertTitle>"You\u{2019}ve been unsubscribed"</AlertTitle>
                            <AlertDescription>
                                "You won\u{2019}t receive any more emails from us about the Kyomi beta launch."
                                <div class="mt-4">
                                    <a
                                        href="/"
                                        class="text-primary hover:underline font-medium"
                                    >
                                        "Return to homepage"
                                    </a>
                                </div>
                            </AlertDescription>
                        </Alert>
                    </Show>
                </div>
            </div>
        </div>
    }
}
