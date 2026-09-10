// SPDX-License-Identifier: AGPL-3.0-or-later

//! Signup completion page (KYO-683 Phase 2) — two steps after the emailed
//! verification link.
//!
//! Route: `/signup/complete?token=xxx`
//!
//! Flow:
//! 1. **Confirm** (Step A) — the page loads from the emailed link. It does
//!    NOT redeem the token on load: a GET must stay side-effect-free
//!    because corporate mail scanners (Outlook/Defender, Proofpoint)
//!    prefetch URLs found in email and would burn a single-use token before
//!    the human ever clicks. The user ticks the terms/marketing checkboxes
//!    and clicks "Verify Email", which POSTs `signup_verify` — the only
//!    call that consumes the token. On success the account exists
//!    (`verified = true`, no credentials yet) and the caller is signed in
//!    (cookies set).
//! 2. **CredentialSetup** (Step B) — now authenticated, the page collects a
//!    name and offers a passkey and/or password. Presented as a step in
//!    the flow, not an optional aside: there is no "Skip" button and no
//!    "you can do this later" copy, because the ticket wants people to
//!    actually set a credential up. Skipping by closing the tab is still
//!    survivable — `/account/recover` already serves a verified account
//!    with no credentials at all (`recovery_start_service`).
//! 3. **Completing** — a brief branded pause (DESIGN.md's animated-logo
//!    loading pattern) while the name is saved, then `nav("/onboarding")`.
//!
//! State machine: Confirm | CredentialSetup | Completing | Error.
//!
//! Reuses existing server fns rather than a second WebAuthn ceremony or a
//! second password path: `security::set_password` (already generic over
//! any authenticated user), `security::{start_passkey_registration,
//! complete_passkey_registration}` (purpose `PASSKEY_ADD_DEVICE`, already
//! generic), `profile::update_profile_name`, and the canonical
//! `utils::webauthn::start_registration` browser bridge (the same
//! registration bridge `passkey_recovery_complete.rs` uses for its own
//! ceremony).

use leptos::prelude::*;
#[cfg(target_arch = "wasm32")]
use leptos_router::hooks::use_navigate;
use phosphor_leptos::Icon;
use crate::components::{
    Alert, AlertDescription, AlertVariant, Button, ButtonLink, ButtonSize, ButtonVariant, Checkbox,
    Label, Spinner, INPUT_CLASS,
};
use crate::pages::auth::auth_layout::AuthLayout;
use crate::server_fns::auth::{signup_verify, SignupVerifyResult};
use crate::server_fns::profile::update_profile_name;
use crate::server_fns::security::set_password;

// ─────────────────────────────────────────────────────────────────────────────
// View state machine
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq)]
enum PageState {
    /// Step A — token present, not yet redeemed.
    Confirm,
    /// Step B — account exists and is authenticated; offer passkey/password.
    CredentialSetup,
    /// Saving the name and about to navigate to onboarding.
    Completing,
    /// Terminal: no token at all in the URL, or the server rejected it as
    /// invalid/expired. Both mean this link cannot be completed as-is.
    Error { message: String },
}

// ─────────────────────────────────────────────────────────────────────────────
// Main component
// ─────────────────────────────────────────────────────────────────────────────

#[component]
pub fn SignupCompletePage() -> impl IntoView {
    // ── SPA navigation handle (wasm32 only — only used in wasm async context) ─
    #[cfg(target_arch = "wasm32")]
    let navigate = StoredValue::new(use_navigate());

    // ── Extract token from URL query params ──────────────────────────────
    let (token, set_token) = signal(Option::<String>::None);
    let (page_state, set_page_state) = signal(PageState::Confirm);

    // ── Step A signals ───────────────────────────────────────────────────
    let (terms_accepted, set_terms_accepted) = signal(false);
    let (marketing_consent, set_marketing_consent) = signal(false);
    let (confirm_error, set_confirm_error) = signal(Option::<String>::None);

    // ── Step B signals ───────────────────────────────────────────────────
    let (cred_name, set_cred_name) = signal(String::new());
    let (cred_password, set_cred_password) = signal(String::new());
    let (cred_confirm_password, set_cred_confirm_password) = signal(String::new());
    let (passkey_added, set_passkey_added) = signal(false);
    let (password_set, set_password_set) = signal(false);
    let (passkey_loading, set_passkey_loading) = signal(false);
    let (webauthn_available, set_webauthn_available) = signal(false);
    let (credential_error, set_credential_error) = signal(Option::<String>::None);

    // ── Extract token on mount ───────────────────────────────────────────
    // Token extraction is browser-only; SSR provides None. This page is
    // never SSR-rendered (served as a CSR shell — see
    // `apps/server/src/lib.rs`'s `/signup/complete` route), so the
    // non-wasm32 arm below only matters for `cargo test --features ssr`.
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

    if let Some(t) = initial_token {
        set_token.set(Some(t));
    } else {
        set_page_state.set(PageState::Error {
            message: "Missing signup token. Please use the link from your email.".to_string(),
        });
    }

    // ── Checkbox signals for the Checkbox component ──────────────────────
    // Page-owned derives: created in this page component body, so their own
    // Owner is this page. Safe because their only reads are the `checked`
    // props of the two `<Checkbox>` elements below, inside this same
    // component's own view tree — a descendant scope of the same page
    // Owner — so each derive and its reader are disposed together (KYO-548).
    let terms_signal = Signal::derive(move || terms_accepted.get()); // lint-allow: disposal-safe=page-owned derive, only reader is the <Checkbox checked=...> prop in this page's own view tree (KYO-548)
    let marketing_signal = Signal::derive(move || marketing_consent.get()); // lint-allow: disposal-safe=page-owned derive, only reader is the <Checkbox checked=...> prop in this page's own view tree (KYO-548)
    let on_terms_change = Callback::new(move |val: bool| set_terms_accepted.set(val));
    let on_marketing_change = Callback::new(move |val: bool| set_marketing_consent.set(val));

    // ── Check WebAuthn availability once at mount ────────────────────────
    // `is_webauthn_available()` has a non-wasm stub returning `false`, so
    // this is safe to call unconditionally rather than gating on
    // target_arch — the browser-only work happens inside that function,
    // not here.
    leptos::task::spawn_local(async move {
        let available = crate::utils::webauthn::is_webauthn_available().await;
        set_webauthn_available.try_set(available);
    });

    // ── Step A: verify action (the POST that redeems the token) ─────────
    let verify_action: Action<(String, bool, bool), Result<SignupVerifyResult, ServerFnError>> =
        Action::new(move |(tok, terms, marketing): &(String, bool, bool)| {
            let tok = tok.clone();
            let terms = *terms;
            let marketing = *marketing;
            async move { signup_verify(tok, terms, marketing).await }
        });

    Effect::new(move |_| {
        if let Some(result) = verify_action.value().get() {
            match result {
                Ok(SignupVerifyResult::Success { name, .. }) => {
                    // Pre-fill the name field only when the server had one
                    // to give us — see `SignupVerifyResult::Success`'s doc
                    // comment (the race case where an already-verified row
                    // is signed into rather than created bare).
                    if !name.is_empty() {
                        set_cred_name.set(name);
                    }
                    set_confirm_error.set(None);
                    set_page_state.set(PageState::CredentialSetup);
                }
                Ok(SignupVerifyResult::Error { message }) => {
                    set_confirm_error.set(Some(message));
                }
                Err(e) => {
                    set_confirm_error.set(Some(format!("Server error: {}", e)));
                }
            }
        }
    });

    let on_confirm_click = move |_: leptos::ev::MouseEvent| {
        if verify_action.pending().get_untracked() {
            return;
        }
        if !terms_accepted.get_untracked() {
            set_confirm_error.set(Some(
                "Please accept the Terms of Service and Privacy Policy.".to_string(),
            ));
            return;
        }
        let Some(tok) = token.get_untracked() else {
            set_page_state.set(PageState::Error {
                message: "Missing signup token. Please use the link from your email.".to_string(),
            });
            return;
        };
        set_confirm_error.set(None);
        verify_action.dispatch((tok, true, marketing_consent.get_untracked()));
    };

    let confirm_disabled =
        move || verify_action.pending().get() || !terms_accepted.get();

    // ── Step B: set-password action ──────────────────────────────────────
    let set_password_action: Action<String, Result<String, ServerFnError>> =
        Action::new(move |pw: &String| {
            let pw = pw.clone();
            async move { set_password(pw).await }
        });

    Effect::new(move |_| {
        if let Some(result) = set_password_action.value().get() {
            match result {
                Ok(_) => {
                    set_password_set.set(true);
                    set_credential_error.set(None);
                    set_cred_password.set(String::new());
                    set_cred_confirm_password.set(String::new());
                }
                Err(e) => set_credential_error.set(Some(e.to_string())),
            }
        }
    });

    let on_set_password = move |_: leptos::ev::MouseEvent| {
        if set_password_action.pending().get_untracked() {
            return;
        }
        let pw = cred_password.get_untracked();
        let confirm = cred_confirm_password.get_untracked();
        if pw.len() < 8 {
            set_credential_error.set(Some("Password must be at least 8 characters.".to_string()));
            return;
        }
        if pw != confirm {
            set_credential_error.set(Some("Passwords do not match.".to_string()));
            return;
        }
        set_credential_error.set(None);
        set_password_action.dispatch(pw);
    };

    // ── Step B: add-passkey handler ──────────────────────────────────────
    // Cannot use Action: add_passkey_flow() drives navigator.credentials.create()
    // via JsFuture — a !Send browser API. Signal writes after the await use
    // try_set for deferred-write safety, matching the rest of this crate's
    // WebAuthn call sites.
    let on_add_passkey = move |_: leptos::ev::MouseEvent| {
        if passkey_loading.get_untracked() {
            return;
        }
        set_passkey_loading.set(true);
        set_credential_error.set(None);
        leptos::task::spawn_local(async move {
            let result = add_passkey_flow().await;
            set_passkey_loading.try_set(false);
            match result {
                Ok(()) => {
                    set_passkey_added.try_set(true);
                }
                Err(e) => {
                    set_credential_error.try_set(Some(e));
                }
            }
        });
    };

    // ── Step B: continue action (save name, then navigate) ───────────────
    let continue_action: Action<String, Result<(), ServerFnError>> =
        Action::new(move |name: &String| {
            let name = name.clone();
            async move { update_profile_name(name).await }
        });

    Effect::new(move |_| {
        if let Some(result) = continue_action.value().get() {
            match result {
                Ok(()) => {
                    set_page_state.set(PageState::Completing);
                    // gloo_timers::future::TimeoutFuture is browser-only —
                    // a branded pause before navigating, same pattern used
                    // elsewhere in this auth flow (e.g.
                    // account_recovery_complete.rs's post-success transition).
                    #[cfg(target_arch = "wasm32")]
                    leptos::task::spawn_local(async move {
                        gloo_timers::future::TimeoutFuture::new(1200).await;
                        if let Some(nav) = navigate.try_get_value() {
                            nav("/onboarding", Default::default());
                        }
                    });
                }
                Err(e) => {
                    set_credential_error.set(Some(format!("Failed to save your name: {}", e)));
                }
            }
        }
    });

    let on_continue = move |_: leptos::ev::MouseEvent| {
        if continue_action.pending().get_untracked() {
            return;
        }
        let name = cred_name.get_untracked();
        if name.trim().is_empty() {
            set_credential_error.set(Some("Please enter your name.".to_string()));
            return;
        }
        set_credential_error.set(None);
        continue_action.dispatch(name.trim().to_string());
    };

    let continue_disabled = move || {
        continue_action.pending().get()
            || cred_name.get().trim().is_empty()
            || !(passkey_added.get() || password_set.get())
    };

    // ── Reactive title & subtitle ────────────────────────────────────────
    // Page-owned derives: created in this page component body, so their own
    // Owner is this page. Safe because their only reads are the `title=`/
    // `subtitle=` props passed to `<AuthLayout>` below — `AuthLayout` is a
    // plain child component invoked from this page's own `view!` call, not
    // a persistent Layout-scoped wrapper (that role belongs only to the
    // authenticated app shell's `<Layout>` in app.rs, which this auth page
    // is not nested under), so its child scope is a descendant of this
    // page's own Owner and disposes with it (KYO-548).
    let title = Signal::derive(move || match page_state.get() { // lint-allow: disposal-safe=page-owned derive, only reader is <AuthLayout title=...>, a child scope of this page (KYO-548)
        PageState::Confirm => "Confirm Your Email".to_string(),
        PageState::CredentialSetup => "Secure Your Account".to_string(),
        PageState::Completing => "All Set".to_string(),
        PageState::Error { .. } => "Signup Link Invalid".to_string(),
    });
    let subtitle = Signal::derive(move || match page_state.get() { // lint-allow: disposal-safe=page-owned derive, only reader is <AuthLayout subtitle=...>, a child scope of this page (KYO-548)
        PageState::Confirm => "Accept the terms to finish verifying your email.".to_string(),
        PageState::CredentialSetup => {
            "Add a passkey or password to finish setting up your account.".to_string()
        }
        PageState::Completing => "Setting up your workspace...".to_string(),
        PageState::Error { message } => message,
    });

    // ── Render ────────────────────────────────────────────────────────────
    view! {
        <AuthLayout title=title subtitle=subtitle>
            {move || {
                match page_state.get() {
                    PageState::Error { .. } => error_view().into_any(),
                    PageState::Completing => completing_view().into_any(),
                    PageState::Confirm => view! {
                        <div>
                            <div class="text-center">
                                <div class="inline-flex items-center justify-center w-16 h-16 rounded-full bg-primary/10 mx-auto mb-6">
                                    <Icon icon=phosphor_leptos::ENVELOPE_SIMPLE_OPEN attr:class="w-8 h-8 text-primary"/>
                                </div>
                            </div>
                            <div class="space-y-6">
                                <div class="space-y-3">
                                    <label class="flex items-start space-x-3 cursor-pointer">
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
                                    <label class="flex items-start space-x-3 cursor-pointer">
                                        <Checkbox
                                            checked=marketing_signal
                                            on_change=on_marketing_change
                                            class="mt-0.5"
                                        />
                                        <span class="text-sm text-muted-foreground">
                                            "I agree to receive product updates and announcements from Kyomi. You can unsubscribe anytime."
                                        </span>
                                    </label>
                                </div>

                                <Show when=move || confirm_error.get().is_some()>
                                    <Alert variant=AlertVariant::Error>
                                        <AlertDescription>
                                            {move || confirm_error.get().unwrap_or_default()}
                                        </AlertDescription>
                                    </Alert>
                                </Show>

                                <Button
                                    button_type="button"
                                    size=ButtonSize::Lg
                                    class="w-full"
                                    on:click=on_confirm_click
                                    disabled=Signal::derive(confirm_disabled)
                                >
                                    {move || {
                                        if verify_action.pending().get() {
                                            view! {
                                                <div class="flex items-center justify-center space-x-2">
                                                    <Spinner class="text-primary-foreground"/>
                                                    <span>"Verifying..."</span>
                                                </div>
                                            }.into_any()
                                        } else {
                                            view! { <span>"Verify Email"</span> }.into_any()
                                        }
                                    }}
                                </Button>
                            </div>
                        </div>
                    }.into_any(),
                    PageState::CredentialSetup => view! {
                        <div class="space-y-6">
                            <div class="text-center">
                                <div class="inline-flex items-center justify-center w-16 h-16 rounded-full bg-primary/10 mx-auto mb-2">
                                    <Icon icon=phosphor_leptos::SHIELD_CHECK attr:class="w-8 h-8 text-primary"/>
                                </div>
                            </div>

                            <div class="space-y-2">
                                <Label html_for="cred-name">"Full Name"</Label>
                                <input
                                    id="cred-name"
                                    type="text"
                                    autocomplete="name"
                                    autofocus
                                    class=INPUT_CLASS
                                    placeholder="John Doe"
                                    required
                                    prop:value=move || cred_name.get()
                                    on:input=move |ev| set_cred_name.set(event_target_value(&ev))
                                />
                            </div>

                            <Show when=move || credential_error.get().is_some()>
                                <Alert variant=AlertVariant::Error>
                                    <AlertDescription>
                                        {move || credential_error.get().unwrap_or_default()}
                                    </AlertDescription>
                                </Alert>
                            </Show>

                            // Passkey — recommended.
                            <div class="rounded-md border border-border p-4 space-y-3">
                                <div class="flex items-center justify-between gap-3">
                                    <div>
                                        <p class="text-sm font-medium text-foreground">"Passkey"</p>
                                        <p class="text-xs text-muted-foreground">
                                            "Sign in with your device's biometrics — recommended."
                                        </p>
                                    </div>
                                    <span class="text-xs font-medium text-primary bg-primary/10 px-2 py-0.5 rounded-full flex-shrink-0">
                                        "Recommended"
                                    </span>
                                </div>
                                <Show
                                    when=move || passkey_added.get()
                                    fallback=move || view! {
                                        <Show
                                            when=move || webauthn_available.get()
                                            fallback=|| view! {
                                                <p class="text-xs text-muted-foreground">
                                                    "Passkeys aren't supported on this device or browser."
                                                </p>
                                            }
                                        >
                                            // Inline page-owned derive: created here, inside this
                                            // page's own view!, with its only read being this
                                            // same `disabled=` prop — reader and derive are the
                                            // same expression, so they share this page's Owner
                                            // and dispose together (KYO-548).
                                            <Button
                                                variant=ButtonVariant::Default
                                                size=ButtonSize::Default
                                                on:click=on_add_passkey
                                                disabled=Signal::derive(move || passkey_loading.get()) // lint-allow: disposal-safe=inline page-owned derive, only reader is this same disabled= prop (KYO-548)
                                            >
                                                <Icon icon=phosphor_leptos::KEY size="16px"/>
                                                {move || if passkey_loading.get() { "Adding..." } else { "Add Passkey" }}
                                            </Button>
                                        </Show>
                                    }.into_any()
                                >
                                    <p class="text-sm text-success-foreground flex items-center gap-1.5">
                                        <Icon icon=phosphor_leptos::CHECK_CIRCLE attr:class="w-4 h-4"/>
                                        "Passkey added"
                                    </p>
                                </Show>
                            </div>

                            // Password.
                            <div class="rounded-md border border-border p-4 space-y-3">
                                <p class="text-sm font-medium text-foreground">"Password"</p>
                                <Show
                                    when=move || password_set.get()
                                    fallback=move || view! {
                                        <div class="space-y-3">
                                            <input
                                                type="password"
                                                autocomplete="new-password"
                                                class=INPUT_CLASS
                                                placeholder="At least 8 characters"
                                                minlength="8"
                                                prop:value=move || cred_password.get()
                                                on:input=move |ev| set_cred_password.set(event_target_value(&ev))
                                            />
                                            <input
                                                type="password"
                                                autocomplete="new-password"
                                                class=INPUT_CLASS
                                                placeholder="Confirm password"
                                                minlength="8"
                                                prop:value=move || cred_confirm_password.get()
                                                on:input=move |ev| set_cred_confirm_password.set(event_target_value(&ev))
                                            />
                                            // Inline page-owned derive: same reasoning as the
                                            // Add Passkey button's `disabled=` above — created
                                            // and read in the same expression, both scoped to
                                            // this page's own Owner (KYO-548).
                                            <Button
                                                variant=ButtonVariant::Outline
                                                on:click=on_set_password
                                                disabled=Signal::derive(move || set_password_action.pending().get()) // lint-allow: disposal-safe=inline page-owned derive, only reader is this same disabled= prop (KYO-548)
                                            >
                                                {move || if set_password_action.pending().get() { "Setting..." } else { "Set Password" }}
                                            </Button>
                                        </div>
                                    }.into_any()
                                >
                                    <p class="text-sm text-success-foreground flex items-center gap-1.5">
                                        <Icon icon=phosphor_leptos::CHECK_CIRCLE attr:class="w-4 h-4"/>
                                        "Password set"
                                    </p>
                                </Show>
                            </div>

                            <Button
                                button_type="button"
                                size=ButtonSize::Lg
                                class="w-full"
                                on:click=on_continue
                                disabled=Signal::derive(continue_disabled)
                            >
                                {move || if continue_action.pending().get() { "Saving..." } else { "Continue" }}
                            </Button>
                        </div>
                    }.into_any(),
                }
            }}
        </AuthLayout>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Step B WebAuthn orchestration
// ─────────────────────────────────────────────────────────────────────────────

/// Orchestrate the passkey-registration ceremony for the credential-setup
/// step: mint a challenge via the authenticated `start_passkey_registration`
/// server fn (KYO-683 Phase 1 — already generic over any authenticated
/// user, purpose `PASSKEY_ADD_DEVICE`), drive `navigator.credentials.create()`
/// through the canonical `utils::webauthn::start_registration` bridge — the
/// same registration bridge `passkey_recovery_complete.rs` uses for its own
/// ceremony (`login.rs`'s passkey sign-in is authentication, not
/// registration, so it calls the sibling `start_authentication` bridge
/// instead) — deliberately not a second hand-rolled WebAuthn ceremony —
/// then verify via `complete_passkey_registration`. No device-name field is
/// collected here (an empty string is sent): this step already asks for a
/// name and a password, so a third free-text field for a device label would
/// add friction the "no skip, but no extra burden either" design
/// deliberately avoids. The empty string is not auto-detected into anything
/// — `server_fns::security::start_passkey_registration` replaces a blank
/// (or whitespace-only) name with the literal string `"Unknown Device"`
/// before minting the challenge, same as every other caller of that server
/// fn; this page has no code path that produces a more specific label.
async fn add_passkey_flow() -> Result<(), String> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        Err("Passkey registration requires a browser".to_string())
    }

    #[cfg(target_arch = "wasm32")]
    {
        use crate::server_fns::security::{complete_passkey_registration, start_passkey_registration};

        let options_json = start_passkey_registration(String::new())
            .await
            .map_err(|e| e.to_string())?;

        // start_passkey_registration returns `{"challenge_id": ..., "options": ccr}`
        // — unwrap to the inner `options` before handing it to the canonical
        // start_registration() bridge, which expects the raw creation options
        // (or a `{"publicKey": ...}` wrapper), not this envelope.
        let data: serde_json::Value = serde_json::from_str(&options_json)
            .map_err(|e| format!("Parse registration options: {e}"))?;
        let challenge_id = data["challenge_id"]
            .as_str()
            .ok_or("Missing challenge_id in registration response")?
            .to_string();
        let inner_options = serde_json::to_string(&data["options"])
            .map_err(|e| format!("Serialize registration options: {e}"))?;

        let credential_json = crate::utils::webauthn::start_registration(&inner_options).await?;
        let credential_value: serde_json::Value = serde_json::from_str(&credential_json)
            .map_err(|e| format!("Parse credential: {e}"))?;

        // complete_passkey_registration expects the credential re-wrapped
        // with the challenge_id — the counterpart envelope to the one
        // start_passkey_registration sent.
        let combined = serde_json::json!({
            "challenge_id": challenge_id,
            "credential": credential_value,
        });
        let combined_json = serde_json::to_string(&combined)
            .map_err(|e| format!("Serialize credential envelope: {e}"))?;

        complete_passkey_registration(combined_json)
            .await
            .map(|_| ())
            .map_err(|e| e.to_string())
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
            <ButtonLink href="/login" variant=ButtonVariant::Outline class="w-full">
                "Back to Login"
            </ButtonLink>
        </div>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Completing view
// ─────────────────────────────────────────────────────────────────────────────

fn completing_view() -> impl IntoView {
    view! {
        <div class="text-center space-y-4">
            // Branded moment (auth page) — DESIGN.md Loading State Pattern
            <img src="/kyomi_animated_logo.svg" alt="Processing" class="w-12 h-12 mx-auto"/>
        </div>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests (KYO-683 Phase 2)
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use crate::test_support::extract_between;

    const SRC: &str = include_str!("signup_complete.rs");
    const TEST_MOD_MARKER: &str = "#[cfg(test)]\nmod tests {";
    fn production_src() -> &'static str {
        SRC.split(TEST_MOD_MARKER)
            .next()
            .expect("TEST_MOD_MARKER must be found in SRC")
    }

    /// The GET that loads this page from the emailed link must never
    /// redeem the token — only the Step A "Verify Email" click may. A mail
    /// scanner (Outlook/Defender, Proofpoint) prefetches URLs found in
    /// email; if `signup_verify` fired anywhere outside the click handler's
    /// dispatch body, the scanner's GET would burn the single-use token
    /// before the human ever saw the page. Pinning the call count to
    /// exactly one is a cheap proxy for "only reachable from a click".
    #[test]
    fn signup_verify_is_only_called_from_the_confirm_click_handler() {
        let count = production_src().matches("signup_verify(").count();
        assert_eq!(
            count, 1,
            "signup_verify must be called exactly once — from verify_action's \
             dispatch body — found {count} call site(s)"
        );
    }

    /// Step A (Confirm) must not collect a password or a name — KYO-683
    /// moved both into Step B (CredentialSetup), and Step A only redeems
    /// the token plus the terms/marketing checkboxes.
    #[test]
    fn confirm_step_collects_no_credential_or_name_fields() {
        let confirm_block = extract_between(
            production_src(),
            "PageState::Confirm => view! {",
            "PageState::CredentialSetup => view! {",
        );
        assert!(
            !confirm_block.contains("type=\"password\""),
            "Step A (Confirm) must not collect a password — that belongs to Step B \
             (CredentialSetup)"
        );
        assert!(
            !confirm_block.contains("Full Name"),
            "Step A (Confirm) must not collect a name — KYO-683 moved the name field \
             to Step B (CredentialSetup)"
        );
    }

    /// CredentialSetup must read as a mandatory step in the flow, not a
    /// skippable aside — the ticket is explicit that passkey/password setup
    /// must not read as skippable, because Kyomi wants people to actually
    /// set one up. No "Skip" button, no "later" copy.
    #[test]
    fn credential_setup_has_no_skip_option() {
        let block = extract_between(
            production_src(),
            "PageState::CredentialSetup => view! {",
            "</AuthLayout>",
        );
        let lower = block.to_lowercase();
        assert!(
            !lower.contains("skip"),
            "CredentialSetup must not offer a Skip option — KYO-683 presents \
             passkey/password setup as a mandatory step in the flow, not an \
             optional aside"
        );
        assert!(
            !lower.contains("later"),
            "CredentialSetup must not use \"you can do this later\"-style copy"
        );
    }

    /// `continue_disabled` must require both a non-empty name AND at least
    /// one credential (passkey or password) — without this, "Continue"
    /// would let the step be skipped in practice even without an explicit
    /// Skip button.
    #[test]
    fn continue_requires_name_and_at_least_one_credential() {
        let src = production_src();
        assert!(
            src.contains("cred_name.get().trim().is_empty()"),
            "continue_disabled must require a non-empty name"
        );
        assert!(
            src.contains("!(passkey_added.get() || password_set.get())"),
            "continue_disabled must require at least one of passkey_added / \
             password_set — otherwise Continue would be reachable with zero \
             credentials set, which is exactly the skip path KYO-683 forbids"
        );
    }

    /// Both credentials must stay reachable after setting one — setting a
    /// password must not hide the passkey option, and vice versa, so
    /// "both" is actually achievable per the ticket's acceptance criteria.
    #[test]
    fn both_credentials_remain_reachable_independently() {
        let src = production_src();
        assert!(
            src.contains("when=move || passkey_added.get()"),
            "the passkey section must be gated on its own passkey_added signal, \
             independent of password_set"
        );
        assert!(
            src.contains("when=move || password_set.get()"),
            "the password section must be gated on its own password_set signal, \
             independent of passkey_added"
        );
    }

    /// CredentialSetup must reuse the existing authenticated server fns —
    /// not a second WebAuthn ceremony or a second password-hashing path.
    #[test]
    fn credential_setup_reuses_existing_server_fns() {
        let src = production_src();
        for needle in [
            "start_passkey_registration(",
            "complete_passkey_registration(",
            "set_password(",
            "update_profile_name(",
        ] {
            assert!(
                src.contains(needle),
                "CredentialSetup must call {needle} — reusing the existing \
                 authenticated server fns rather than a second implementation"
            );
        }
    }

    /// The passkey option must be gated on WebAuthn availability, per the
    /// ticket ("gate its availability on webauthn::is_webauthn_available()").
    #[test]
    fn passkey_option_gated_on_webauthn_availability() {
        assert!(
            production_src().contains("is_webauthn_available()"),
            "the passkey section must check webauthn::is_webauthn_available() \
             before offering the Add Passkey button"
        );
    }
}
