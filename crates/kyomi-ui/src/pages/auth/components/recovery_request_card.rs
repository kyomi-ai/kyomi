// SPDX-License-Identifier: AGPL-3.0-or-later

//! Shared recovery request card used by both account recovery and passkey recovery.
//!
//! Renders an email input → Send Recovery Link form, then transitions to a
//! "Check Your Email" confirmation on submit.
//!
//! ## Enumeration-safety invariant (KYO-684)
//!
//! The `Ok` branch — and only the `Ok` branch — is constant and
//! account-independent: `recovery_start` / `passkey_recovery_start` both
//! return `Ok` whether or not an account exists for the submitted address
//! (see their doc comments in `server_fns/auth.rs`), so transitioning to
//! "Check Your Email" on `Ok` never leaks account existence. `Err` is safe
//! to surface to the user precisely because no `Err` path from either
//! server function is reachable from an account lookup — every
//! account-dependent outcome (no user, unverified, token-mint failure) is
//! folded into `Ok`. The only `Err`s are infrastructure failures, rate
//! limiting, and the self-hosted-without-SMTP precondition, all of which
//! fire before any account lookup runs. If a future change to either
//! server function makes any `Err` path depend on account state, this
//! invariant breaks and the card must go back to discarding that error.

use leptos::prelude::*;
use phosphor_leptos::Icon;
use crate::components::{
    Alert, AlertDescription, AlertTitle, AlertVariant, Button, ButtonLink, ButtonSize,
    ButtonVariant, Label, Spinner, INPUT_CLASS,
};
use crate::pages::auth::auth_layout::AuthLayout;
use crate::server_fns::auth::{passkey_recovery_start, recovery_start};

/// Which recovery flow this card represents. Controls the icon and title.
#[derive(Clone, Copy, PartialEq)]
pub enum RecoveryKind {
    Account,
    Passkey,
}

impl RecoveryKind {
    fn title(self) -> &'static str {
        match self {
            Self::Account => "Recover Your Account",
            Self::Passkey => "Recover Your Passkey",
        }
    }
}

/// Extract user-facing text from a `recovery_start` / `passkey_recovery_start`
/// error.
///
/// Every `Err` either of those two functions returns is built as
/// `ServerFnError::ServerError(msg)` — the config/infra checks
/// (self-hosted-without-SMTP, missing headers, missing KV store) construct it
/// directly via `ServerFnError::new(msg)`, and the rate-limit path goes
/// through `.into_sfn_core()`, which builds the same variant from
/// `kyomi_core::Error::user_message()`. Either way, `msg` is already the
/// clean, human-readable text meant for a person to read. `ServerFnError`'s
/// own `Display` impl prepends a transport-log prefix ("error running server
/// function: ") to that variant, so calling `.to_string()` here would leak
/// that prefix into the UI (see `docs/standards/error-handling/
/// user-message-not-display-for-user-facing-text.md`) — match the
/// `ServerError` variant directly instead and take `msg` as-is. Any other
/// variant (e.g. a `Request` failure because the call never reached the
/// server at all) has no clean equivalent to extract, so it falls back to
/// `Display` — that text carries its own prefix too, but it's the only
/// description available for a failure the server never got a chance to
/// classify.
fn recovery_error_message(e: ServerFnError) -> String {
    match e {
        ServerFnError::ServerError(msg) => msg,
        other => other.to_string(),
    }
}

#[component]
pub fn RecoveryRequestCard(kind: RecoveryKind) -> impl IntoView {
    let (email, set_email) = signal(String::new());
    let (loading, set_loading) = signal(false);
    let (submitted, set_submitted) = signal(false);
    let (error, set_error) = signal(Option::<String>::None);

    let on_submit = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();

        let current_email = email.get_untracked();
        if current_email.trim().is_empty() {
            set_error.set(Some("Please enter your email address.".to_string()));
            return;
        }

        set_loading.set(true);
        set_error.set(None);

        leptos::task::spawn_local(async move {
            // Surfacing `Err` here is enumeration-safe — see the module doc
            // comment. `Ok` (account-independent) always advances to "Check
            // Your Email"; `Err` (infra/rate-limit/config, never
            // account-dependent) is shown to the user and the form stays up
            // so they can act on it and retry.
            let result = match kind {
                RecoveryKind::Account => recovery_start(current_email).await,
                RecoveryKind::Passkey => passkey_recovery_start(current_email).await,
            };
            match result {
                Ok(_) => {
                    set_submitted.try_set(true);
                }
                Err(e) => {
                    set_error.try_set(Some(recovery_error_message(e)));
                }
            }
            set_loading.try_set(false);
        });
    };

    let submit_disabled = move || loading.get() || email.get().trim().is_empty();

    let title = Signal::derive(move || {
        if submitted.get() {
            "Check Your Email".to_string()
        } else {
            kind.title().to_string()
        }
    });
    let subtitle = Signal::derive(move || {
        if submitted.get() {
            "If a verified account exists with this email, we have sent a recovery link."
                .to_string()
        } else {
            "Enter your email address to receive a recovery link.".to_string()
        }
    });

    view! {
        <AuthLayout title=title subtitle=subtitle>
            {move || {
                if submitted.get() {
                    view! { <SubmittedView set_submitted=set_submitted set_email=set_email/> }
                        .into_any()
                } else {
                    view! {
                        <FormView
                            kind=kind
                            email=email
                            set_email=set_email
                            loading=loading
                            error=error
                            submit_disabled=submit_disabled
                            on_submit=on_submit
                        />
                    }
                        .into_any()
                }
            }}
        </AuthLayout>
    }
}

#[component]
fn FormView(
    kind: RecoveryKind,
    email: ReadSignal<String>,
    set_email: WriteSignal<String>,
    loading: ReadSignal<bool>,
    error: ReadSignal<Option<String>>,
    submit_disabled: impl Fn() -> bool + Copy + Send + Sync + 'static,
    on_submit: impl Fn(leptos::ev::SubmitEvent) + Copy + Send + Sync + 'static,
) -> impl IntoView {
    view! {
        <div>
            <div class="text-center">
                <div class="inline-flex items-center justify-center w-16 h-16 rounded-full bg-primary/10 mx-auto mb-6">
                    {match kind {
                        RecoveryKind::Account => view! {
                            <Icon icon=phosphor_leptos::LOCK_KEY attr:class="w-8 h-8 text-primary"/>
                        }.into_any(),
                        RecoveryKind::Passkey => view! {
                            <Icon icon=phosphor_leptos::KEY attr:class="w-8 h-8 text-primary"/>
                        }.into_any(),
                    }}
                </div>
            </div>
            <form on:submit=on_submit class="space-y-4">
                    <Show when=move || error.get().is_some()>
                        <Alert variant=AlertVariant::Error>
                            <AlertTitle>"Error"</AlertTitle>
                            <AlertDescription>
                                {move || error.get().unwrap_or_default()}
                            </AlertDescription>
                        </Alert>
                    </Show>

                    <div class="space-y-2">
                        <Label html_for="recovery-email">"Email address"</Label>
                        <input
                            id="recovery-email"
                            type="email"
                            placeholder="you@example.com"
                            autocomplete="email"
                            autofocus=true
                            required=true
                            class=INPUT_CLASS
                            prop:value=move || email.get()
                            on:input=move |ev| set_email.set(event_target_value(&ev))
                        />
                    </div>

                    <Button
                        button_type="submit"
                        variant=ButtonVariant::Default
                        size=ButtonSize::Lg
                        disabled=Signal::derive(submit_disabled)
                        class="w-full"
                    >
                        {move || {
                            if loading.get() {
                                view! {
                                    <div class="flex items-center justify-center space-x-2">
                                        <Spinner class="text-primary-foreground"/>
                                        <span>"Sending..."</span>
                                    </div>
                                }.into_any()
                            } else {
                                view! { <span>"Send Recovery Link"</span> }.into_any()
                            }
                        }}
                    </Button>

                <div class="text-center pt-2">
                    <a
                        href="/login"
                        class="text-sm text-muted-foreground hover:text-foreground transition-colors"
                    >
                        "Back to login"
                    </a>
                </div>
            </form>
        </div>
    }
}

#[component]
fn SubmittedView(
    set_submitted: WriteSignal<bool>,
    set_email: WriteSignal<String>,
) -> impl IntoView {
    view! {
        <div class="space-y-4">
            <div class="text-center">
                <div class="inline-flex items-center justify-center w-16 h-16 rounded-full bg-primary/10 mx-auto mb-6">
                    <Icon icon=phosphor_leptos::ENVELOPE attr:class="w-8 h-8 text-primary"/>
                </div>
            </div>
            <p class="text-sm text-center text-muted-foreground">
                "The recovery link expires in 15 minutes and can only be used once."
            </p>

            <div class="pt-4">
                <ButtonLink
                    href="/login"
                    variant=ButtonVariant::Outline
                    size=ButtonSize::Lg
                    class="w-full mb-4"
                >
                    "Back to Login"
                </ButtonLink>

                <Button
                    variant=ButtonVariant::Link
                    class="w-full"
                    on:click=move |_| {
                        set_submitted.set(false);
                        set_email.set(String::new());
                    }
                >
                    "Try a different email"
                </Button>
            </div>
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::extract_between;

    /// This file's own source, for source-text wiring assertions below —
    /// see `docs/standards/testing/anchor-source-text-markers-on-code-not-copy.md`.
    /// Leptos view trees (and the `spawn_local` closures embedded in them)
    /// can't be exercised as plain unit tests without a DOM/integration
    /// harness this crate doesn't have — see `crate::test_support`'s module
    /// doc comment — so the `on_submit` branch invariants below are pinned
    /// against the source text instead of executed.
    const SRC: &str = include_str!("recovery_request_card.rs");

    const TEST_MOD_MARKER: &str = "#[cfg(test)]\nmod tests {";
    fn production_src() -> &'static str {
        SRC.split(TEST_MOD_MARKER)
            .next()
            .expect("TEST_MOD_MARKER must be found in SRC")
    }

    /// The `on_submit` closure body, from its own `let` binding to the next
    /// `let` binding after it (`submit_disabled`) — both structural markers
    /// a change to the match arms inside can't remove.
    fn on_submit_block() -> &'static str {
        extract_between(
            production_src(),
            "let on_submit = move |ev: leptos::ev::SubmitEvent| {",
            "let submit_disabled = move || loading.get()",
        )
    }

    // ── KYO-684: Ok always advances, Err never does ──────────────────────

    /// The `Ok` arm must advance to "Check Your Email" and must not also
    /// touch `error` — `Ok` is the account-independent, enumeration-safe
    /// outcome and its handling must stay unconditional (see the module doc
    /// comment's invariant).
    #[test]
    fn ok_arm_sets_submitted_and_does_not_touch_error() {
        let block = on_submit_block();
        let ok_arm = extract_between(block, "Ok(_) => {", "Err(e) => {");
        assert!(
            ok_arm.contains("set_submitted.try_set(true)"),
            "Ok arm must advance to the submitted state; got:\n{ok_arm}"
        );
        assert!(
            !ok_arm.contains("set_error"),
            "Ok arm must not touch the error signal — Ok is the constant, \
             account-independent outcome; got:\n{ok_arm}"
        );
    }

    /// The regression this ticket exists to fix: the `Err` arm must surface
    /// the error and must NOT advance to the submitted state — the whole
    /// point is that a rate-limited or SMTP-misconfigured caller stays on
    /// the form instead of being told to go check an email nobody sent.
    #[test]
    fn err_arm_sets_error_and_does_not_advance_to_submitted() {
        let block = on_submit_block();
        let err_arm = extract_between(block, "Err(e) => {", "set_loading.try_set(false);");
        assert!(
            err_arm.contains("set_error.try_set(Some(recovery_error_message(e)))"),
            "Err arm must surface the server's message via set_error; got:\n{err_arm}"
        );
        assert!(
            !err_arm.contains("set_submitted"),
            "Err arm must NOT set submitted — a rate-limited or \
             SMTP-misconfigured caller must stay on the form, not be told \
             to check an email nobody sent (KYO-684); got:\n{err_arm}"
        );
    }

    /// `set_loading.try_set(false)` must run unconditionally after the
    /// match, not once per arm — otherwise a future edit could plausibly
    /// leave `loading` stuck on one of the two paths.
    #[test]
    fn loading_is_cleared_exactly_once_outside_the_match_arms() {
        let block = on_submit_block();
        let count = block.matches("set_loading.try_set(false)").count();
        assert_eq!(
            count, 1,
            "expected set_loading.try_set(false) to appear exactly once, \
             after the match rather than duplicated per-arm; found {count}"
        );
    }

    // ── KYO-684: error text extraction ────────────────────────────────────

    /// The presentation decision for this ticket: show the server's message
    /// as-is, never wrapped in a "Server error: " prefix the way
    /// `login.rs` does — these messages (rate-limit text, the self-hosted
    /// SMTP precondition) are already sanitized and user/operator-actionable,
    /// and that prefix would turn "Ask your administrator to configure SMTP"
    /// into noise.
    #[test]
    fn error_text_is_not_wrapped_in_a_server_error_prefix() {
        let block = on_submit_block();
        assert!(
            !block.contains("Server error:"),
            "recovery errors must be shown as-is, not wrapped the way \
             login.rs wraps its server errors; got:\n{block}"
        );
    }

    /// `recovery_error_message` must extract the `ServerFnError::ServerError`
    /// payload directly rather than calling `.to_string()` on the whole
    /// error — `ServerFnError`'s `Display` impl prepends "error running
    /// server function: " to that variant's message, which would leak a
    /// transport-log prefix into user-actionable text like the rate-limit
    /// message or the self-hosted SMTP precondition.
    #[test]
    fn recovery_error_message_extracts_server_error_payload_without_display_prefix() {
        let e = ServerFnError::ServerError("Rate limited. Try again in 42 seconds".to_string());
        let msg = recovery_error_message(e);
        assert_eq!(
            msg, "Rate limited. Try again in 42 seconds",
            "must extract the ServerError payload verbatim, not \
             e.to_string() (which prepends a transport-log prefix)"
        );
        assert!(
            !msg.contains("error running server function"),
            "extracted message must not carry ServerFnError's Display \
             prefix; got: {msg:?}"
        );
    }

    /// A `ServerFnError` variant with no clean payload to extract (e.g. a
    /// network failure that never reached the server) still needs *some*
    /// text shown — the fallback preserves the failure's own description
    /// via `Display`, even though that description carries its own prefix.
    #[test]
    fn recovery_error_message_falls_back_to_display_for_non_server_error_variants() {
        let e = ServerFnError::Request("network unreachable".to_string());
        let msg = recovery_error_message(e);
        assert!(
            msg.contains("network unreachable"),
            "fallback must preserve the underlying failure description; got: {msg:?}"
        );
    }
}
