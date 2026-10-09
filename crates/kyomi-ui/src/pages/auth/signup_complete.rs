// SPDX-License-Identifier: AGPL-3.0-or-later

//! Signup completion page.
//!
//! Route: `/signup/complete?token=xxx`
//!
//! Two-step flow (KYO-728 phase 2):
//!
//! 1. **Confirm.** The page renders a Terms/Privacy checkbox, an optional
//!    marketing-consent checkbox, and a "Confirm Email" button. Nothing is
//!    sent to the server until that button is clicked — loading this page
//!    (including an automated prefetch by a corporate mail scanner such as
//!    Outlook/Defender or Proofpoint) never calls `signup_verify` and never
//!    consumes the token. Clicking the button calls
//!    `crate::server_fns::auth::signup_verify`, which creates the account,
//!    records terms/marketing consent, and sets session cookies — the user
//!    is authenticated from this point on.
//! 2. **Credentials.** Now authenticated, the page collects a display name
//!    (`update_profile_name`) and offers a password
//!    (`security::set_password`, skipped if `security::has_password`
//!    already reports one — e.g. the token belonged to an account that
//!    already signed in with Google) and/or a passkey
//!    (`security::start_passkey_registration` /
//!    `complete_passkey_registration`, driven through the browser via
//!    `crate::utils::webauthn::start_registration`). At least one of
//!    password/passkey must be established before "Finish Setup" proceeds —
//!    this is a client-side UX gate, not a server-enforced one, so it reads
//!    as a required step rather than an optional aside without blocking a
//!    user who already has a credential from the adopt path above.
//!
//! On success the user is redirected to `/onboarding`.
//!
//! State machine: `Confirm | Verifying | Credentials | Success | Error`.

use leptos::prelude::*;
#[cfg(target_arch = "wasm32")]
use leptos_router::hooks::use_navigate;
use phosphor_leptos::Icon;
use crate::components::{
    Alert, AlertDescription, AlertVariant, Button, ButtonLink, ButtonSize, ButtonVariant, Checkbox,
    Label, INPUT_CLASS,
};
use crate::pages::auth::auth_layout::AuthLayout;
use crate::server_fns::auth::{signup_verify, SignupVerifyResult};
use crate::server_fns::profile::update_profile_name;
use crate::server_fns::security::{has_password, set_password};
#[cfg(target_arch = "wasm32")]
use crate::server_fns::security::{complete_passkey_registration, start_passkey_registration};

// ─────────────────────────────────────────────────────────────────────────────
// View state machine
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq)]
enum PageState {
    /// Token present, nothing sent to the server yet — waiting for the user
    /// to click "Confirm Email".
    Confirm,
    /// `signup_verify` is in flight.
    Verifying,
    /// Verified and authenticated; collecting name + a credential.
    Credentials,
    Success,
    Error { message: String },
}

/// UI state for the in-place "Add a Passkey" action on the Credentials step.
#[derive(Clone, Debug, PartialEq)]
enum PasskeyUiState {
    Idle,
    Registering,
    Added,
    Failed(String),
}

// ─────────────────────────────────────────────────────────────────────────────
// Main component
// ─────────────────────────────────────────────────────────────────────────────

#[component]
pub fn SignupCompletePage() -> impl IntoView {
    // ── SPA navigation handle (wasm32 only — only used in wasm async context) ─
    // Wrapped in StoredValue so it can be copied into FnMut closures (view! reactive closures).
    #[cfg(target_arch = "wasm32")]
    let navigate = StoredValue::new(use_navigate());

    // ── Extract token from URL query params ──────────────────────────────
    let (token, _set_token) = signal(Option::<String>::None);
    let (page_state, set_page_state) = signal(PageState::Confirm);

    // ── Step 1 (Confirm) signals ─────────────────────────────────────────
    let (terms_accepted, set_terms_accepted) = signal(false);
    let (marketing_consent, set_marketing_consent) = signal(false);
    let (error, set_error) = signal(Option::<String>::None);

    // ── Step 2 (Credentials) signals ─────────────────────────────────────
    let (name, set_name) = signal(String::new());
    let new_password = RwSignal::new(String::new());
    let confirm_password = RwSignal::new(String::new());
    // `None` until `has_password()` resolves; treated as "no password yet"
    // in the meantime so the password fields default to visible rather than
    // flashing in once the check completes.
    let has_password_signal = RwSignal::new(Option::<bool>::None);
    let webauthn_available = RwSignal::new(false);
    let passkey_added = RwSignal::new(false);
    let passkey_state = RwSignal::new(PasskeyUiState::Idle);

    // ── Extract token on mount ───────────────────────────────────────────
    // Token extraction is browser-only; SSR provides None. This only reads
    // the URL — it never calls signup_verify, so it is safe to run
    // unconditionally on mount (including under a mail scanner's prefetch).
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

    // Set the token or transition to Error — runs on both targets so the
    // compiler sees all PageState variants constructed.
    if let Some(t) = initial_token {
        _set_token.set(Some(t));
    } else {
        set_page_state.set(PageState::Error {
            message: "Missing signup token. Please use the link from your email.".to_string(),
        });
    }

    // ── Check WebAuthn support once, up front ────────────────────────────
    // Pure browser-capability check — no server call, no token involved —
    // so there's no harm running it before the user reaches step 2.
    Effect::new(move |_| {
        leptos::task::spawn_local(async move {
            let available = crate::utils::webauthn::is_webauthn_available().await;
            webauthn_available.try_set(available);
        });
    });

    // ── Fetch has_password() only once authenticated (Credentials step) ──
    // `has_password` requires a session, which only exists after
    // signup_verify succeeds — fetching it any earlier would just be a
    // guaranteed-to-fail authenticated call on page load.
    Effect::new(move |_| {
        if matches!(page_state.get(), PageState::Credentials) {
            leptos::task::spawn_local(async move {
                let result = has_password().await;
                has_password_signal.try_set(result.ok());
            });
        }
    });

    // ── Checkbox signals for the Checkbox component ──────────────────────
    let terms_signal = Signal::derive(move || terms_accepted.get());
    let marketing_signal = Signal::derive(move || marketing_consent.get());

    let on_terms_change = Callback::new(move |val: bool| {
        set_terms_accepted.set(val);
    });
    let on_marketing_change = Callback::new(move |val: bool| {
        set_marketing_consent.set(val);
    });

    // ── Step 1: Confirm action ────────────────────────────────────────────
    // User-triggered mutation -> Action (never raw spawn_local): signup_verify
    // is a plain server fn with no !Send involvement, same as
    // rename_action/delete_action in passkey_manager.rs.
    let confirm_action: Action<(), Result<SignupVerifyResult, ServerFnError>> =
        Action::new(move |_: &()| {
            let tok = token.get_untracked().unwrap_or_default();
            let terms = terms_accepted.get_untracked();
            let marketing = marketing_consent.get_untracked();
            // `name: None` — collected in the Credentials step instead via
            // `update_profile_name`, per `SignupVerifyParams::name`'s doc.
            async move { signup_verify(tok, None, terms, marketing).await }
        });

    Effect::new(move |_| {
        if let Some(result) = confirm_action.value().get() {
            match result {
                Ok(SignupVerifyResult::Success { .. }) => {
                    set_error.set(None);
                    set_page_state.set(PageState::Credentials);
                }
                Ok(SignupVerifyResult::Error { message }) => {
                    set_error.set(Some(message));
                    set_page_state.set(PageState::Confirm);
                }
                Err(e) => {
                    set_error.set(Some(format!("Server error: {}", e)));
                    set_page_state.set(PageState::Confirm);
                }
            }
        }
    });

    let on_confirm_submit = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();

        if confirm_action.pending().get_untracked() {
            return;
        }
        if !terms_accepted.get_untracked() {
            set_error.set(Some(
                "Please accept the Terms of Service and Privacy Policy.".to_string(),
            ));
            return;
        }
        if token.get_untracked().is_none() {
            set_error.set(Some(
                "Missing signup token. Please use the link from your email.".to_string(),
            ));
            return;
        }

        set_error.set(None);
        set_page_state.set(PageState::Verifying);
        confirm_action.dispatch(());
    };

    // ── Step 2: Finish setup action ───────────────────────────────────────
    let finish_action: Action<(String, Option<String>), Result<(), String>> =
        Action::new(move |input: &(String, Option<String>)| {
            let (name, password) = input.clone();
            async move {
                update_profile_name(name).await.map_err(|e| e.to_string())?;
                if let Some(password) = password {
                    set_password(password).await.map_err(|e| e.to_string())?;
                }
                Ok(())
            }
        });

    Effect::new(move |_| {
        if let Some(result) = finish_action.value().get() {
            match result {
                Ok(()) => {
                    set_error.set(None);
                    set_page_state.set(PageState::Success);

                    // Navigate to onboarding after 1.5 seconds (keeps WASM in memory).
                    #[cfg(target_arch = "wasm32")]
                    {
                        leptos::task::spawn_local(async move {
                            let Some(nav) = navigate.try_get_value() else { return };
                            gloo_timers::future::TimeoutFuture::new(1500).await;
                            nav("/onboarding", Default::default());
                        });
                    }
                }
                Err(message) => set_error.set(Some(message)),
            }
        }
    });

    let on_finish_submit = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();

        if finish_action.pending().get_untracked() {
            return;
        }

        let current_name = name.get_untracked();
        if current_name.trim().is_empty() {
            set_error.set(Some("Please enter your name.".to_string()));
            return;
        }

        let pw = new_password.get_untracked();
        let confirm_pw = confirm_password.get_untracked();
        let password_provided = !pw.trim().is_empty() || !confirm_pw.trim().is_empty();
        if password_provided {
            if pw != confirm_pw {
                set_error.set(Some("Passwords do not match.".to_string()));
                return;
            }
            if pw.len() < 8 {
                set_error.set(Some(
                    "Password must be at least 8 characters.".to_string(),
                ));
                return;
            }
        }

        let already_has_credential = has_password_signal.get_untracked().unwrap_or(false)
            || password_provided
            || passkey_added.get_untracked();
        if !already_has_credential {
            set_error.set(Some(
                "Set a password or add a passkey to secure your account.".to_string(),
            ));
            return;
        }

        set_error.set(None);
        finish_action.dispatch((
            current_name.trim().to_string(),
            password_provided.then_some(pw),
        ));
    };

    // ── Passkey add flow (spawn_local — drives !Send WebAuthn browser APIs) ─
    let on_add_passkey = move |_: leptos::ev::MouseEvent| {
        if matches!(passkey_state.get_untracked(), PasskeyUiState::Registering) {
            return;
        }
        passkey_state.set(PasskeyUiState::Registering);

        leptos::task::spawn_local(async move {
            match register_passkey().await {
                Ok(()) => {
                    passkey_added.try_set(true);
                    passkey_state.try_set(PasskeyUiState::Added);
                }
                Err(message) => {
                    passkey_state.try_set(PasskeyUiState::Failed(message));
                }
            }
        });
    };

    // ── Reactive title & subtitle ────────────────────────────────────────
    let title = Signal::derive(move || match page_state.get() {
        PageState::Confirm => "Confirm Your Email".to_string(),
        PageState::Verifying => "Confirming...".to_string(),
        PageState::Credentials => "Secure Your Account".to_string(),
        PageState::Success => "Account Created".to_string(),
        PageState::Error { .. } => "Signup Link Invalid".to_string(),
    });
    let subtitle = Signal::derive(move || match page_state.get() {
        PageState::Confirm => "Click below to verify your email and create your account.".to_string(),
        PageState::Verifying => "Verifying your email — just a moment.".to_string(),
        PageState::Credentials => "Add your name and a way to sign back in.".to_string(),
        PageState::Success => "Welcome to Kyomi! Setting up your workspace...".to_string(),
        PageState::Error { message } => message,
    });

    // ── Render ────────────────────────────────────────────────────────────
    view! {
        <AuthLayout title=title subtitle=subtitle>
            {move || {
                let state = page_state.get();
                match state {
                    PageState::Error { .. } => error_view().into_any(),
                    PageState::Success => success_view().into_any(),
                    PageState::Verifying => verifying_view().into_any(),
                    PageState::Confirm => view! {
                        <div>
                            <div class="text-center">
                                <div class="inline-flex items-center justify-center w-16 h-16 rounded-full bg-primary/10 mx-auto mb-6">
                                    <Icon icon=phosphor_leptos::ENVELOPE attr:class="w-8 h-8 text-primary"/>
                                </div>
                            </div>
                            <form on:submit=on_confirm_submit class="space-y-6">
                                // Terms and consent
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

                                // Error alert
                                {move || error.get().map(|msg| view! {
                                    <Alert variant=AlertVariant::Error>
                                        <AlertDescription>{msg}</AlertDescription>
                                    </Alert>
                                })}

                                <Button
                                    button_type="submit"
                                    size=ButtonSize::Lg
                                    class="w-full"
                                    disabled=Signal::derive(move || {
                                        !terms_accepted.get() || confirm_action.pending().get()
                                    })
                                >
                                    {move || if confirm_action.pending().get() {
                                        "Confirming..."
                                    } else {
                                        "Confirm Email"
                                    }}
                                </Button>
                            </form>
                        </div>
                    }.into_any(),
                    PageState::Credentials => view! {
                        <div>
                            <div class="text-center">
                                <div class="inline-flex items-center justify-center w-16 h-16 rounded-full bg-primary/10 mx-auto mb-6">
                                    <Icon icon=phosphor_leptos::LOCK_KEY attr:class="w-8 h-8 text-primary"/>
                                </div>
                            </div>
                            <form on:submit=on_finish_submit class="space-y-6">
                                // Name input
                                <div class="space-y-2">
                                    <Label html_for="name">"Full Name"</Label>
                                    <input
                                        id="name"
                                        type="text"
                                        autocomplete="name"
                                        autofocus
                                        class=INPUT_CLASS
                                        placeholder="John Doe"
                                        required
                                        prop:value=move || name.get()
                                        on:input=move |ev| set_name.set(event_target_value(&ev))
                                    />
                                </div>

                                // Password fields — hidden once has_password() confirms
                                // the account (adopted from an OAuth sign-in) already has one.
                                <Show when=move || !has_password_signal.get().unwrap_or(false)>
                                    <div class="space-y-2">
                                        <Label html_for="password">"Password (optional if you add a passkey below)"</Label>
                                        <input
                                            id="password"
                                            type="password"
                                            autocomplete="new-password"
                                            class=INPUT_CLASS
                                            placeholder="At least 8 characters"
                                            minlength="8"
                                            prop:value=move || new_password.get()
                                            on:input=move |ev| new_password.set(event_target_value(&ev))
                                        />
                                    </div>
                                    <div class="space-y-2">
                                        <Label html_for="confirm-password">"Confirm Password"</Label>
                                        <input
                                            id="confirm-password"
                                            type="password"
                                            autocomplete="new-password"
                                            class=INPUT_CLASS
                                            placeholder="Re-enter your password"
                                            minlength="8"
                                            prop:value=move || confirm_password.get()
                                            on:input=move |ev| confirm_password.set(event_target_value(&ev))
                                        />
                                    </div>
                                </Show>

                                // Passkey option
                                <Show when=move || webauthn_available.get()>
                                    <div class="space-y-2 rounded-md border border-border p-4">
                                        <p class="text-sm text-foreground">
                                            "Add a passkey to sign in with your device's biometrics instead of a password."
                                        </p>
                                        <Button
                                            variant=ButtonVariant::Outline
                                            class="w-full"
                                            disabled=Signal::derive(move || {
                                                matches!(
                                                    passkey_state.get(),
                                                    PasskeyUiState::Registering | PasskeyUiState::Added
                                                )
                                            })
                                            on:click=on_add_passkey
                                        >
                                            {move || match passkey_state.get() {
                                                PasskeyUiState::Idle => "Add a Passkey".to_string(),
                                                PasskeyUiState::Registering => "Adding Passkey...".to_string(),
                                                PasskeyUiState::Added => "Passkey Added".to_string(),
                                                PasskeyUiState::Failed(_) => "Try Again".to_string(),
                                            }}
                                        </Button>
                                        {move || match passkey_state.get() {
                                            PasskeyUiState::Failed(message) => Some(view! {
                                                <p class="text-sm text-error-foreground">{message}</p>
                                            }),
                                            _ => None,
                                        }}
                                    </div>
                                </Show>

                                // Error alert
                                {move || error.get().map(|msg| view! {
                                    <Alert variant=AlertVariant::Error>
                                        <AlertDescription>{msg}</AlertDescription>
                                    </Alert>
                                })}

                                <Button
                                    button_type="submit"
                                    size=ButtonSize::Lg
                                    class="w-full"
                                    disabled=Signal::derive(move || finish_action.pending().get())
                                >
                                    {move || if finish_action.pending().get() {
                                        "Finishing..."
                                    } else {
                                        "Finish Setup"
                                    }}
                                </Button>
                            </form>
                        </div>
                    }.into_any(),
                }
            }}
        </AuthLayout>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Passkey registration (Credentials step)
// ─────────────────────────────────────────────────────────────────────────────

/// Register a passkey for the just-authenticated account.
///
/// Same 3-step server round trip as `passkey_manager.rs`'s `add_passkey_flow`
/// (start -> browser create -> complete), but drives the browser side
/// through the shared `crate::utils::webauthn::start_registration` helper
/// instead of a private reimplementation of the base64/`ArrayBuffer` dance,
/// and repacks its response into the `{challenge_id, credential}` envelope
/// `complete_passkey_registration` expects.
async fn register_passkey() -> Result<(), String> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        Err("Passkey registration requires a browser".to_string())
    }

    #[cfg(target_arch = "wasm32")]
    {
        let start_json = start_passkey_registration(String::new())
            .await
            .map_err(|e| e.to_string())?;

        let start_value: serde_json::Value = serde_json::from_str(&start_json)
            .map_err(|e| format!("Parse start response: {e}"))?;
        let challenge_id = start_value["challenge_id"]
            .as_str()
            .ok_or("Missing challenge_id in start response")?
            .to_string();
        let options_json = serde_json::to_string(&start_value["options"])
            .map_err(|e| format!("Serialize options: {e}"))?;

        let credential_json = crate::utils::webauthn::start_registration(&options_json)
            .await
            .map_err(|e| map_webauthn_error(&e))?;
        let credential_value: serde_json::Value = serde_json::from_str(&credential_json)
            .map_err(|e| format!("Parse credential: {e}"))?;

        let combined = serde_json::json!({
            "challenge_id": challenge_id,
            "credential": credential_value,
        });
        let combined_json =
            serde_json::to_string(&combined).map_err(|e| format!("Serialize credential: {e}"))?;

        complete_passkey_registration(combined_json)
            .await
            .map_err(|e| e.to_string())?;

        Ok(())
    }
}

/// Map WebAuthn error strings to user-friendly messages.
#[cfg(target_arch = "wasm32")]
fn map_webauthn_error(error: &str) -> String {
    if error.contains("InvalidStateError") {
        "A passkey already exists for this device. You can still finish setup with a password."
            .to_string()
    } else if error.contains("NotAllowedError") {
        "Passkey creation was cancelled or timed out. Please try again.".to_string()
    } else if error.contains("AbortError") {
        "Passkey creation was cancelled. Please try again.".to_string()
    } else if error.contains("NotSupportedError") {
        "Your device does not support passkeys. You can add one later from Settings."
            .to_string()
    } else {
        format!("Failed to create passkey: {}", error)
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
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use crate::test_support::extract_between;

    /// This file's own source, for source-text wiring assertions below.
    const SRC: &str = include_str!("signup_complete.rs");

    /// `SRC` sliced to production code only, cutting off at this test
    /// module's own opening marker — needed for any assertion that scans
    /// the *whole* file for a literal, since this test module repeats
    /// several of those literals verbatim in comments/assertion messages.
    const TEST_MOD_MARKER: &str = "#[cfg(test)]\nmod tests {";
    fn production_src() -> &'static str {
        SRC.split(TEST_MOD_MARKER)
            .next()
            .expect("TEST_MOD_MARKER must be found in SRC")
    }

    // ── The property that matters most: signup_verify never fires on mount ──

    /// KYO-728's core safety property: a mail scanner's prefetch of the
    /// emailed link must never consume the signup token. `signup_verify(`
    /// must appear in exactly one place in this file — inside the
    /// `confirm_action` Action body — never inside an `Effect::new` or any
    /// other construct that could run unconditionally at mount.
    #[test]
    fn signup_verify_has_exactly_one_call_site() {
        let src = production_src();
        let count = src.matches("signup_verify(").count();
        assert_eq!(
            count, 1,
            "expected exactly one signup_verify( call site (inside \
             confirm_action) — found {count}. A second call site is exactly \
             the kind of change that could fire on mount."
        );
    }

    /// The single `signup_verify(` call site must live inside the
    /// `confirm_action` Action definition, not inside an `Effect::new` (which
    /// can run unconditionally when its dependencies are first read) or a
    /// bare `spawn_local` outside a click handler.
    #[test]
    fn signup_verify_call_site_is_inside_confirm_action() {
        let src = production_src();
        let action_block = extract_between(
            src,
            "let confirm_action: Action<(), Result<SignupVerifyResult, ServerFnError>> =",
            "Effect::new(move |_| {\n        if let Some(result) = confirm_action.value().get()",
        );
        assert!(
            action_block.contains("signup_verify("),
            "confirm_action's body must contain the signup_verify( call — \
             found:\n{action_block}"
        );
    }

    /// `confirm_action.dispatch(` — the thing that actually runs the Action
    /// body above — must only be reachable from `on_confirm_submit`, which is
    /// wired to the Confirm form's `on:submit`, not from any effect or
    /// mount-time closure.
    #[test]
    fn confirm_action_dispatch_is_gated_behind_the_confirm_form_submit_handler() {
        let src = production_src();
        let count = src.matches("confirm_action.dispatch(").count();
        assert_eq!(
            count, 1,
            "expected exactly one confirm_action.dispatch( call site — found {count}."
        );

        let handler_block = extract_between(
            src,
            "let on_confirm_submit = move |ev: leptos::ev::SubmitEvent| {",
            "// ── Step 2: Finish setup action",
        );
        assert!(
            handler_block.contains("confirm_action.dispatch(())"),
            "confirm_action.dispatch(()) must be called from on_confirm_submit — \
             found:\n{handler_block}"
        );

        let confirm_form = extract_between(
            src,
            "PageState::Confirm => view! {",
            "PageState::Credentials => view! {",
        );
        assert!(
            confirm_form.contains("on:submit=on_confirm_submit"),
            "the Confirm step's <form> must be wired to on_confirm_submit — \
             found:\n{confirm_form}"
        );
    }

    /// Negative space: no `Effect::new` in this file may reference
    /// `signup_verify` or dispatch `confirm_action` — that would be exactly
    /// the auto-fire-on-mount regression this page exists to prevent.
    #[test]
    fn no_effect_references_signup_verify_or_dispatches_confirm_action() {
        let src = production_src();
        for window in src.split("Effect::new(").skip(1) {
            // Each `window` starts immediately after an `Effect::new(` call;
            // bound it to a plausible single-effect extent so unrelated
            // later code in the file isn't accidentally included.
            let bounded = window.get(..800).unwrap_or(window);
            assert!(
                !bounded.contains("signup_verify("),
                "an Effect::new body references signup_verify( — this can \
                 fire on mount:\n{bounded}"
            );
            assert!(
                !bounded.contains("confirm_action.dispatch("),
                "an Effect::new body dispatches confirm_action — this can \
                 fire on mount:\n{bounded}"
            );
        }
    }

    // ── Initial state is Confirm, not further along ──────────────────────

    /// The page must start in `PageState::Confirm` when a token is present
    /// — never `Verifying` or `Credentials`, which would imply the account
    /// was already touched before any user interaction.
    #[test]
    fn initial_page_state_is_confirm() {
        let src = production_src();
        assert!(
            src.contains("let (page_state, set_page_state) = signal(PageState::Confirm);"),
            "the page must mount into PageState::Confirm"
        );
    }
}
