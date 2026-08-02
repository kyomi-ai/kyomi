// SPDX-License-Identifier: AGPL-3.0-or-later

//! Login page — full implementation matching `apps/frontend/src/pages/Login.jsx`.
//!
//! State machine with four views: Credentials, TwoFactor, Signup, CheckEmail.
//! Uses `AuthLayout` for the shared two-panel layout, existing sub-components
//! (`PasskeySignInButton`, `GoogleSignInButton`, `AuthDivider`), and server
//! functions (`get_auth_config`, `login_with_password`).

use leptos::prelude::*;
use phosphor_leptos::Icon;
#[cfg(target_arch = "wasm32")]
use leptos_router::hooks::{use_navigate, use_query_map};

use crate::components::{
    Alert, AlertDescription, AlertTitle, AlertVariant, Button, ButtonSize, ButtonVariant, Label,
    Spinner, INPUT_CLASS,
};
use crate::pages::auth::auth_layout::AuthLayout;
use crate::pages::auth::components::{AuthDivider, GoogleSignInButton, PasskeySignInButton};
use crate::server_fns::auth::{
    get_auth_config, login_with_password, passkey_login_complete, passkey_login_start,
    passkey_signup_start, resend_verification, signup_start, LoginResult,
    PasskeySignupStartResult, SignupResult,
};

// ─────────────────────────────────────────────────────────────────────────────
// Login subtitle rotation — editorial voice on the sign-in screen.
// Picked once per mount (see `subtitle_idx` below) so the value is stable
// across the reactive subtitle signal.
// ─────────────────────────────────────────────────────────────────────────────

const LOGIN_SUBTITLES: &[&str] = &[
    "Your data is where you left it.",
    "Back to the numbers.",
    "Let's see what changed.",
    "Numbers missed you.",
    "New data. Same warehouse.",
    "Everything you left running.",
    "The queries are ready.",
];

// ─────────────────────────────────────────────────────────────────────────────
// View state machine
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq)]
enum LoginView {
    Credentials,
    TwoFactor { email: String },
    Signup,
    CheckEmail { email: String },
}

// ─────────────────────────────────────────────────────────────────────────────
// Main component
// ─────────────────────────────────────────────────────────────────────────────

#[component]
pub fn LoginPage(
    /// When true, start in signup mode instead of login mode.
    #[prop(default = false)]
    signup_mode: bool,
) -> impl IntoView {
    // ── View state ──────────────────────────────────────────────────────
    let initial_view = if signup_mode {
        LoginView::Signup
    } else {
        LoginView::Credentials
    };
    let (view_state, set_view_state) = signal(initial_view);

    // ── Credentials view signals ────────────────────────────────────────
    let (email, set_email) = signal(String::new());
    let (password, set_password) = signal(String::new());
    let (error, set_error) = signal(Option::<String>::None);
    let (success_msg, set_success_msg) = signal(Option::<String>::None);

    // ── 2FA signals ─────────────────────────────────────────────────────
    let (totp_code, set_totp_code) = signal(String::new());

    // ── Passkey / Google loading ────────────────────────────────────────
    let (passkey_loading, set_passkey_loading) = signal(false);
    let (google_loading, set_google_loading) = signal(false);

    // ── Signup signals ──────────────────────────────────────────────────
    let (signup_email, set_signup_email) = signal(String::new());
    let (signup_name, set_signup_name) = signal(String::new());
    let (signup_password, set_signup_password) = signal(String::new());
    // ── Verification needed (email not verified) ────────────────────────
    let (verification_needed, set_verification_needed) = signal(false);
    let (verification_email, set_verification_email) = signal(String::new());
    let (resend_success, set_resend_success) = signal(false);

    // ── SPA navigation handle (must be obtained at component level) ─────
    // Wrapped in StoredValue so it can be copied into Fn + Copy closures.
    #[cfg(target_arch = "wasm32")]
    let navigate = StoredValue::new(use_navigate());

    // ── Read post-login destination from query params ─────────────────
    // `oauth_continue` (set by /api/v1/oauth/authorize when an MCP client
    // initiates OAuth) takes precedence over `redirect` (set by the server
    // auth guard). When present, we send the user to the OAuth continue
    // endpoint after sign-in so the MCP client gets its callback.
    //
    // Splicing `oauth_continue` directly into the URL is safe because the
    // server generates it via `redis_ops::generate_token()` (base64url
    // alphabet — `A-Z a-z 0-9 - _`), none of which require percent-encoding.
    #[cfg(target_arch = "wasm32")]
    let redirect_url = {
        let query = use_query_map();
        move || -> String {
            query.with(|q| {
                if let Some(oc) = q.get("oauth_continue").filter(|s| !s.is_empty()) {
                    return format!("/api/v1/oauth/authorize/continue?state={oc}");
                }
                q.get("redirect")
                    .filter(|r| !r.is_empty() && r.starts_with('/'))
                    .unwrap_or_else(|| "/".to_string())
            })
        }
    };

    // ── Login action (password + 2FA submit) ────────────────────────────
    // Replaces the spawn_local pattern for login_with_password. Input tuple:
    // (email, password, totp_opt). The action value is the raw server result;
    // navigation and state transitions happen in the Effect below.
    let login_action = Action::new(
        move |(login_email, login_password, totp_opt): &(String, String, Option<String>)| {
            let login_email = login_email.clone();
            let login_password = login_password.clone();
            let totp_opt = totp_opt.clone();
            async move { login_with_password(login_email, login_password, totp_opt).await }
        },
    );

    // ── Signup action ───────────────────────────────────────────────────
    // Input tuple: (email, name_opt, password_opt). Returns (email, result) so
    // the Effect can use the dispatch-time email for CheckEmail navigation
    // without reading the (potentially mutated) live signal.
    let signup_action = Action::new(
        move |(dispatched_email, name_opt, password_opt): &(
            String,
            Option<String>,
            Option<String>,
        )| {
            let dispatched_email = dispatched_email.clone();
            let name_opt = name_opt.clone();
            let password_opt = password_opt.clone();
            async move {
                let result = signup_start(dispatched_email.clone(), name_opt, password_opt).await;
                (dispatched_email, result)
            }
        },
    );

    // ── Passkey signup action ───────────────────────────────────────────
    // Unlike the login page's passkey handler, this never touches
    // navigator.credentials — it only mints a signup token/email link, so
    // it's a plain server call and can use Action (no !Send browser API
    // involved). Input tuple: (email, name_opt). Returns (email, result) so
    // the Effect can navigate to the CheckEmail view using the dispatch-time
    // email, matching signup_action's pattern above.
    let passkey_signup_action = Action::new(
        move |(dispatched_email, name_opt): &(String, Option<String>)| {
            let dispatched_email = dispatched_email.clone();
            let name_opt = name_opt.clone();
            async move {
                let result = passkey_signup_start(dispatched_email.clone(), name_opt).await;
                (dispatched_email, result)
            }
        },
    );

    // ── Resend verification action ──────────────────────────────────────
    let resend_action = Action::new(move |ver_email: &String| {
        let ver_email = ver_email.clone();
        async move { resend_verification(ver_email).await }
    });

    // ── Effect: react to login action result ────────────────────────────
    // Handles navigation and state transitions after login_with_password
    // completes. Runs in component scope — automatically cleans up on
    // navigation, preventing the disposed-signal panics that spawn_local causes.
    Effect::new(move |_| {
        if let Some(result) = login_action.value().get() {
            match result {
                Ok(LoginResult::Success { .. }) => {
                    #[cfg(target_arch = "wasm32")]
                    {
                        let dest = redirect_url();
                        if dest.starts_with("/api/") {
                            if let Some(window) = web_sys::window() {
                                let _ = window.location().set_href(&dest);
                            }
                        } else if let Some(nav) = navigate.try_get_value() {
                            nav(&dest, Default::default());
                        }
                    }
                }
                Ok(LoginResult::TwoFactorRequired { email: user_email }) => {
                    set_view_state.set(LoginView::TwoFactor { email: user_email });
                    set_error.set(None);
                }
                Ok(LoginResult::VerificationRequired { email: user_email }) => {
                    set_verification_needed.set(true);
                    set_verification_email.set(user_email);
                }
                Ok(LoginResult::RateLimited { retry_after_secs }) => {
                    set_error.set(Some(format!(
                        "Too many login attempts. Please try again in {} seconds.",
                        retry_after_secs
                    )));
                }
                Ok(LoginResult::Error { message }) => {
                    set_error.set(Some(message));
                }
                Err(e) => {
                    set_error.set(Some(format!("Server error: {}", e)));
                }
            }
        }
    });

    // ── Effect: react to signup action result ───────────────────────────
    // signup_action returns (dispatched_email, result) — the email is threaded
    // through so the Effect uses the value that was actually sent to the server,
    // not a potentially-mutated live signal.
    Effect::new(move |_| {
        if let Some((dispatched_email, result)) = signup_action.value().get() {
            match result {
                Ok(SignupResult::AccountCreated { redirect }) => {
                    #[cfg(target_arch = "wasm32")]
                    if let Some(nav) = navigate.try_get_value() {
                        nav(&redirect, Default::default());
                    }
                    let _ = &redirect; // suppress unused warning on SSR
                }
                Ok(SignupResult::VerificationRequired { message }) => {
                    set_success_msg.set(Some(message));
                    set_view_state.set(LoginView::CheckEmail {
                        email: dispatched_email,
                    });
                }
                Ok(SignupResult::Error { message }) => {
                    set_error.set(Some(message));
                }
                Ok(SignupResult::RateLimited { .. }) => {
                    set_error.set(Some(
                        "Too many signup attempts. Please try again later.".to_string(),
                    ));
                }
                Err(e) => {
                    set_error.set(Some(format!("Server error: {}", e)));
                }
            }
        }
    });

    // ── Effect: react to passkey signup action result ────────────────────
    // Mirrors the signup_action Effect above. TokenIssued (self-hosted
    // SMTP-less) navigates straight to the WebAuthn-ceremony page instead
    // of setting cookies — passkey signup has no one-step AccountCreated
    // equivalent, see PasskeySignupStartResult's doc comment.
    Effect::new(move |_| {
        if let Some((dispatched_email, result)) = passkey_signup_action.value().get() {
            match result {
                Ok(PasskeySignupStartResult::TokenIssued { token }) => {
                    #[cfg(target_arch = "wasm32")]
                    if let Some(nav) = navigate.try_get_value() {
                        nav(
                            &format!("/auth/passkey-signup?token={token}"),
                            Default::default(),
                        );
                    }
                    let _ = &token; // suppress unused warning on SSR
                }
                Ok(PasskeySignupStartResult::VerificationRequired { message }) => {
                    set_success_msg.set(Some(message));
                    set_view_state.set(LoginView::CheckEmail {
                        email: dispatched_email,
                    });
                }
                Ok(PasskeySignupStartResult::Error { message }) => {
                    set_error.set(Some(message));
                }
                Ok(PasskeySignupStartResult::RateLimited { .. }) => {
                    set_error.set(Some(
                        "Too many signup attempts. Please try again later.".to_string(),
                    ));
                }
                Err(e) => {
                    set_error.set(Some(format!("Server error: {}", e)));
                }
            }
        }
    });

    // ── Effect: react to resend verification action result ──────────────
    Effect::new(move |_| {
        if let Some(result) = resend_action.value().get() {
            match result {
                Ok(()) => set_resend_success.set(true),
                Err(_) => set_error.set(Some(
                    "Failed to resend verification email.".to_string(),
                )),
            }
        }
    });

    // ── Already authenticated? Redirect away from login page ──────────
    // Matches React: Login.jsx line 124 — `if (isAuthenticated) { navigate(redirect) }`
    //
    // Uses spawn_local (not Resource::new) so it doesn't consume a serialized
    // resource ID. Resource IDs must be identical between SSR and client or
    // hydration markers will be misaligned, causing a tachys panic.
    //
    // Cannot use Action here: window.location().set_href() is a !Send browser
    // API (web_sys). Signal writes use try_set for deferred safety.
    #[cfg(target_arch = "wasm32")]
    {
        use crate::server_fns::sidebar::get_sidebar_user;
        let redirect_for_check = redirect_url;
        leptos::task::spawn_local(async move {
            if get_sidebar_user().await.is_ok()
                && let Some(window) = web_sys::window() {
                    let _ = window.location().set_href(&redirect_for_check());
                }
        });
    }

    // ── Auth config resource ────────────────────────────────────────────
    let auth_config = Resource::new(|| (), |_| get_auth_config());

    // ── Derived signals for conditional sections ────────────────────────
    let show_passkey_section = move || {
        auth_config
            .get()
            .and_then(|r| r.ok())
            .map(|c| c.passkeys)
            .unwrap_or(false)
    };

    let show_google_section = move || {
        auth_config
            .get()
            .and_then(|r| r.ok())
            .map(|c| c.google_oauth)
            .unwrap_or(false)
    };

    let is_self_hosted_no_smtp = move || {
        auth_config
            .get()
            .and_then(|r| r.ok())
            .map(|c| c.self_hosted && !c.smtp_configured)
            .unwrap_or(false)
    };

    // ── Reactive title & subtitle ───────────────────────────────────────
    let title = Signal::derive(move || {
        match view_state.get() {
            LoginView::Signup | LoginView::CheckEmail { .. } => "Create your account".to_string(),
            _ => "Welcome back".to_string(),
        }
    });

    // Pick a login subtitle once per mount. Seed = minute-rounded wall-clock
    // time, so a refresh-within-a-minute stays stable (no flicker on reload)
    // but a new visit a minute later rotates to a fresh line. Stored in a
    // `StoredValue` so the reactive subtitle signal reads the same index
    // on every tick.
    let subtitle_idx: StoredValue<usize> = StoredValue::new({
        #[cfg(target_arch = "wasm32")]
        {
            let minutes = (js_sys::Date::now() / 60_000.0) as usize;
            minutes % LOGIN_SUBTITLES.len()
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            0usize
        }
    });

    let subtitle = Signal::derive(move || {
        match view_state.get() {
            LoginView::Signup | LoginView::CheckEmail { .. } => {
                "Get started with Kyomi".to_string()
            }
            _ => LOGIN_SUBTITLES[subtitle_idx.try_get_value().unwrap_or(0)].to_string(),
        }
    });

    // ── Google OAuth handler ────────────────────────────────────────────
    // Cannot use Action: uses JsFuture::from(window.fetch_with_str(...)) and
    // window.location().set_href() which are !Send browser APIs. Signal writes
    // inside the async block use try_set for deferred-write safety.
    let on_google_click = Callback::new(move |()| {
        set_google_loading.set(true);
        set_error.set(None);

        leptos::task::spawn_local(async move {
            #[cfg(target_arch = "wasm32")]
            {
                use wasm_bindgen::prelude::*;

                let result: Result<(), String> = async {
                    let window = web_sys::window().ok_or("no window")?;
                    // Forward `oauth_continue` from the current URL so the
                    // server can stash it in the Google CSRF state and resume
                    // the MCP OAuth flow after Google sign-in completes.
                    // Raw-string extraction (no percent-decode) is safe — the
                    // token is base64url, see redirect_url() comment above.
                    let login_url = {
                        let search = window.location().search().unwrap_or_default();
                        let oc = search
                            .strip_prefix('?')
                            .unwrap_or(&search)
                            .split('&')
                            .find_map(|pair| pair.strip_prefix("oauth_continue="))
                            .filter(|s| !s.is_empty());
                        match oc {
                            Some(state) => format!("/api/v1/auth/google/login?oauth_continue={state}"),
                            None => "/api/v1/auth/google/login".to_string(),
                        }
                    };
                    let resp_val = wasm_bindgen_futures::JsFuture::from(
                        window.fetch_with_str(&login_url),
                    )
                    .await
                    .map_err(|e| format!("{e:?}"))?;

                    let resp: web_sys::Response = resp_val.dyn_into().map_err(|e| format!("{e:?}"))?;
                    let json_val = wasm_bindgen_futures::JsFuture::from(
                        resp.json().map_err(|e| format!("{e:?}"))?,
                    )
                    .await
                    .map_err(|e| format!("{e:?}"))?;

                    let auth_url = js_sys::Reflect::get(&json_val, &JsValue::from_str("authorization_url"))
                        .ok()
                        .and_then(|v| v.as_string())
                        .ok_or("Missing authorization_url in response")?;

                    window.location().set_href(&auth_url).map_err(|e| format!("{e:?}"))?;
                    Ok(())
                }
                .await;

                if let Err(e) = result {
                    set_error.try_set(Some(format!("Google login failed: {e}")));
                    set_google_loading.try_set(false);
                }
            }
        });
    });

    // ── Passkey handler ─────────────────────────────────────────────────
    // Cannot use Action: calls start_authentication() which uses JsFuture and
    // navigator.credentials.get() — !Send browser APIs. Signal writes inside
    // the async block use try_set for deferred-write safety.
    let on_passkey_click = Callback::new(move |()| {
        set_passkey_loading.set(true);
        set_error.set(None);

        leptos::task::spawn_local(async move {
            // Step 1: Get challenge from server
            let start_result = passkey_login_start().await;
            let (challenge_id, request_json) = match start_result {
                Ok(r) => (r.challenge_id, r.request_challenge),
                Err(e) => {
                    set_error.try_set(Some(format!("Failed to start passkey login: {}", e)));
                    set_passkey_loading.try_set(false);
                    return;
                }
            };

            // Step 2: Trigger browser WebAuthn prompt
            let assertion_json =
                match crate::utils::webauthn::start_authentication(&request_json).await {
                    Ok(json) => json,
                    Err(e) => {
                        set_error.try_set(Some(format!("Passkey authentication failed: {}", e)));
                        set_passkey_loading.try_set(false);
                        return;
                    }
                };

            // Step 3: Complete login on server
            let complete_result = passkey_login_complete(challenge_id, assertion_json).await;
            match complete_result {
                Ok(LoginResult::Success { .. }) => {
                    #[cfg(target_arch = "wasm32")]
                    {
                        let dest = redirect_url();
                        if dest.starts_with("/api/") {
                            if let Some(window) = web_sys::window() {
                                let _ = window.location().set_href(&dest);
                            }
                        } else if let Some(nav) = navigate.try_get_value() {
                            nav(&dest, Default::default());
                        }
                    }
                }
                Ok(LoginResult::VerificationRequired { email }) => {
                    set_verification_needed.try_set(true);
                    set_verification_email.try_set(email);
                    set_passkey_loading.try_set(false);
                }
                Ok(LoginResult::Error { message }) => {
                    set_error.try_set(Some(message));
                    set_passkey_loading.try_set(false);
                }
                Ok(_) => {
                    set_passkey_loading.try_set(false);
                }
                Err(e) => {
                    set_error.try_set(Some(format!("Server error: {}", e)));
                    set_passkey_loading.try_set(false);
                }
            }
        });
    });

    // ── Login form submit ───────────────────────────────────────────────
    // Dispatches login_action with dispatch-time values; the Effect above
    // handles navigation and state transitions on the result.
    let on_login_submit = {
        move |ev: leptos::ev::SubmitEvent| {
            ev.prevent_default();

            // Double-dispatch guard: Action's pending() prevents concurrent calls.
            if login_action.pending().get_untracked() {
                return;
            }

            let current_email = email.get_untracked();
            let current_password = password.get_untracked();
            let current_totp = totp_code.get_untracked();
            let totp_opt = if current_totp.is_empty() {
                None
            } else {
                Some(current_totp)
            };

            set_error.set(None);
            set_verification_needed.set(false);
            set_resend_success.set(false);

            login_action.dispatch((current_email, current_password, totp_opt));
        }
    };

    // ── Signup form submit ──────────────────────────────────────────────
    // Dispatches signup_action; the Effect above handles navigation and state.
    let on_signup_submit = {
        move |ev: leptos::ev::SubmitEvent| {
            ev.prevent_default();

            // Double-dispatch guard.
            if signup_action.pending().get_untracked() {
                return;
            }

            let current_email = signup_email.get_untracked();
            if current_email.trim().is_empty() {
                set_error.set(Some("Please enter your email address.".to_string()));
                return;
            }

            let self_hosted_no_smtp = is_self_hosted_no_smtp();
            let current_name = signup_name.get_untracked();
            let current_password = signup_password.get_untracked();

            if self_hosted_no_smtp {
                if current_name.trim().is_empty() {
                    set_error.set(Some("Please enter your name.".to_string()));
                    return;
                }
                if current_password.len() < 8 {
                    set_error.set(Some(
                        "Password must be at least 8 characters.".to_string(),
                    ));
                    return;
                }
            }

            set_error.set(None);

            let name_opt = if self_hosted_no_smtp && !current_name.trim().is_empty() {
                Some(current_name)
            } else {
                None
            };
            let password_opt = if self_hosted_no_smtp && !current_password.is_empty() {
                Some(current_password)
            } else {
                None
            };

            signup_action.dispatch((current_email, name_opt, password_opt));
        }
    };

    // ── Passkey signup click handler ────────────────────────────────────
    // Uses whatever email/name are already in the signup form. Name is only
    // ever populated in the self-hosted-no-smtp branch of SignupView (the
    // SaaS form doesn't collect one) — passed through as `None` otherwise.
    let on_passkey_signup_click = Callback::new(move |()| {
        // Double-dispatch guard.
        if passkey_signup_action.pending().get_untracked() {
            return;
        }

        let current_email = signup_email.get_untracked();
        if current_email.trim().is_empty() {
            set_error.set(Some("Please enter your email address.".to_string()));
            return;
        }

        set_error.set(None);

        let current_name = signup_name.get_untracked();
        let name_opt = if current_name.trim().is_empty() {
            None
        } else {
            Some(current_name)
        };

        passkey_signup_action.dispatch((current_email, name_opt));
    });

    // ── Resend verification handler ─────────────────────────────────────
    // Dispatches resend_action; the Effect above handles result state.
    let on_resend_verification = move |_| {
        // Double-dispatch guard.
        if resend_action.pending().get_untracked() {
            return;
        }

        set_resend_success.set(false);
        set_error.set(None);

        let ver_email = verification_email.get_untracked();
        resend_action.dispatch(ver_email);
    };

    // ── Render ───────────────────────────────────────────────────────────
    view! {
        <AuthLayout title=title subtitle=subtitle>
            <div class="space-y-6">
                {move || {
                    let current_view = view_state.get();
                    match current_view {
                        LoginView::Credentials => {
                            let login_loading = Signal::derive(move || login_action.pending().get());
                            let resend_loading = Signal::derive(move || resend_action.pending().get());
                            view! {
                                <CredentialsView
                                    email=email
                                    set_email=set_email
                                    password=password
                                    set_password=set_password
                                    loading=login_loading
                                    error=error
                                    set_error=set_error
                                    success_msg=success_msg
                                    show_passkey_section=show_passkey_section
                                    show_google_section=show_google_section
                                    passkey_loading=passkey_loading
                                    google_loading=google_loading
                                    on_passkey_click=on_passkey_click
                                    on_google_click=on_google_click
                                    on_login_submit=on_login_submit
                                    set_view_state=set_view_state
                                    verification_needed=verification_needed
                                    resend_loading=resend_loading
                                    resend_success=resend_success
                                    on_resend_verification=on_resend_verification
                                />
                            }.into_any()
                        }
                        LoginView::TwoFactor { ref email } => {
                            let tfa_email = email.clone();
                            let login_loading = Signal::derive(move || login_action.pending().get());
                            view! {
                                <TwoFactorView
                                    email=tfa_email
                                    totp_code=totp_code
                                    set_totp_code=set_totp_code
                                    loading=login_loading
                                    error=error
                                    set_error=set_error
                                    on_login_submit=on_login_submit
                                    set_view_state=set_view_state
                                />
                            }.into_any()
                        }
                        LoginView::Signup => {
                            let signup_loading = Signal::derive(move || signup_action.pending().get());
                            let passkey_signup_loading = Signal::derive(move || passkey_signup_action.pending().get());
                            view! {
                                <SignupView
                                    signup_email=signup_email
                                    set_signup_email=set_signup_email
                                    signup_name=signup_name
                                    set_signup_name=set_signup_name
                                    signup_password=signup_password
                                    set_signup_password=set_signup_password
                                    signup_loading=signup_loading
                                    error=error
                                    set_error=set_error
                                    is_self_hosted_no_smtp=is_self_hosted_no_smtp
                                    on_signup_submit=on_signup_submit
                                    set_view_state=set_view_state
                                    show_passkey_section=show_passkey_section
                                    passkey_signup_loading=passkey_signup_loading
                                    on_passkey_signup_click=on_passkey_signup_click
                                />
                            }.into_any()
                        }
                        LoginView::CheckEmail { ref email } => {
                            let check_email = email.clone();
                            view! {
                                <CheckEmailView
                                    email=check_email
                                    set_view_state=set_view_state
                                    set_error=set_error
                                    set_success_msg=set_success_msg
                                    set_signup_email=set_signup_email
                                />
                            }.into_any()
                        }
                    }
                }}
            </div>
        </AuthLayout>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Credentials View
// ─────────────────────────────────────────────────────────────────────────────

#[component]
fn CredentialsView(
    email: ReadSignal<String>,
    set_email: WriteSignal<String>,
    password: ReadSignal<String>,
    set_password: WriteSignal<String>,
    loading: Signal<bool>,
    error: ReadSignal<Option<String>>,
    set_error: WriteSignal<Option<String>>,
    success_msg: ReadSignal<Option<String>>,
    show_passkey_section: impl Fn() -> bool + Copy + Send + Sync + 'static,
    show_google_section: impl Fn() -> bool + Copy + Send + Sync + 'static,
    passkey_loading: ReadSignal<bool>,
    google_loading: ReadSignal<bool>,
    on_passkey_click: Callback<()>,
    on_google_click: Callback<()>,
    on_login_submit: impl Fn(leptos::ev::SubmitEvent) + Copy + Send + Sync + 'static,
    set_view_state: WriteSignal<LoginView>,
    verification_needed: ReadSignal<bool>,
    resend_loading: Signal<bool>,
    resend_success: ReadSignal<bool>,
    on_resend_verification: impl Fn(leptos::ev::MouseEvent) + Copy + Send + Sync + 'static,
) -> impl IntoView {
    // Derived: disable sign-in button when email or password is empty, or loading
    let sign_in_disabled = move || {
        loading.get() || email.get().trim().is_empty() || password.get().is_empty()
    };

    view! {
        <>
            // Passkey Sign In
            <Show when=show_passkey_section>
                <div class="space-y-3">
                    <PasskeySignInButton
                        loading=Signal::derive(move || passkey_loading.get())
                        disabled=Signal::derive(move || google_loading.get())
                        on_click=on_passkey_click
                    />
                </div>
            </Show>

            // Divider between passkey and Google
            <Show when=move || show_passkey_section() && show_google_section()>
                <AuthDivider text="or"/>
            </Show>

            // Google Sign In
            <Show when=show_google_section>
                <div class="space-y-3">
                    <GoogleSignInButton
                        loading=Signal::derive(move || google_loading.get())
                        disabled=Signal::derive(move || passkey_loading.get())
                        on_click=on_google_click
                    />
                </div>
            </Show>

            // Success / Error / Verification Alerts — placed above the form for a11y
            // (screen readers encounter them before the email field)
            <Show when=move || success_msg.get().is_some()>
                <Alert variant=AlertVariant::Success>
                    <AlertDescription>
                        {move || success_msg.get().unwrap_or_default()}
                    </AlertDescription>
                </Alert>
            </Show>

            <Show when=move || error.get().is_some() && !verification_needed.get()>
                <Alert variant=AlertVariant::Error>
                    <AlertDescription>
                        {move || error.get().unwrap_or_default()}
                    </AlertDescription>
                </Alert>
            </Show>

            <Show when=move || verification_needed.get()>
                <Alert variant=AlertVariant::Warning>
                    <AlertTitle>"Email Verification Required"</AlertTitle>
                    <AlertDescription>
                        <p class="mb-3">
                            "Please verify your email before signing in. Check your inbox for the verification link."
                        </p>
                        <Button
                            variant=ButtonVariant::Outline
                            size=ButtonSize::Sm
                            on:click=on_resend_verification
                            disabled=Signal::derive(move || resend_loading.get())
                        >
                            {move || {
                                if resend_loading.get() {
                                    "Sending...".to_string()
                                } else {
                                    "Resend Verification Email".to_string()
                                }
                            }}
                        </Button>
                        <Show when=move || resend_success.get()>
                            <p class="text-sm text-success-foreground mt-2">
                                "Verification email sent! Check your inbox."
                            </p>
                        </Show>
                        <Show when=move || error.get().is_some()>
                            <p class="text-sm text-error-foreground mt-2">
                                {move || error.get().unwrap_or_default()}
                            </p>
                        </Show>
                    </AlertDescription>
                </Alert>
            </Show>

            // Divider before email form — show if any auth option above is visible
            <Show when=move || show_passkey_section() || show_google_section()>
                <AuthDivider text="or sign in with email"/>
            </Show>

            // Email + Password Login
            <form on:submit=on_login_submit class="space-y-4">
                <div class="space-y-2">
                    <Label html_for="login-email">"Email"</Label>
                    <input
                        id="login-email"
                        name="email"
                        type="email"
                        autocomplete="email"
                        placeholder="name@company.com"
                        class=INPUT_CLASS
                        required=true
                        prop:value=move || email.get()
                        on:input=move |ev| set_email.set(event_target_value(&ev))
                    />
                </div>
                <div class="space-y-2">
                    <Label html_for="login-password">"Password"</Label>
                    <input
                        id="login-password"
                        name="password"
                        type="password"
                        autocomplete="current-password"
                        placeholder="Enter your password"
                        class=INPUT_CLASS
                        required=true
                        prop:value=move || password.get()
                        on:input=move |ev| set_password.set(event_target_value(&ev))
                    />
                </div>
                <Button
                    button_type="submit"
                    variant=ButtonVariant::Default
                    size=ButtonSize::Lg
                    disabled=Signal::derive(sign_in_disabled)
                    class="w-full"
                >
                    {move || {
                        if loading.get() {
                            view! {
                                <div class="flex items-center justify-center space-x-2">
                                    <Spinner class="text-primary-foreground"/>
                                    <span>"Signing in..."</span>
                                </div>
                            }.into_any()
                        } else {
                            view! { <span>"Sign In"</span> }.into_any()
                        }
                    }}
                </Button>
                <p class="text-xs text-muted-foreground text-center mt-3">
                    "New to Kyomi? "
                    <Button
                        variant=ButtonVariant::Link
                        size=ButtonSize::Sm
                        // 44px min-height floor for the tertiary link — scoped
                        // locally rather than on ButtonSize::Sm globally, since
                        // dense desktop toolbars also use Sm and would regress.
                        class="min-h-[44px]".to_string()
                        on:click=move |_| {
                            set_view_state.set(LoginView::Signup);
                            set_error.set(None);
                        }
                    >
                        "Create an account"
                    </Button>
                    " · "
                    // `inline-block py-3 px-2` gives a 44px tall hit area
                    // (12 + 12 + ~20 line-height) without enlarging the text.
                    <a
                        href="/account/recover"
                        class="inline-block py-3 px-2 text-primary hover:underline"
                    >
                        "Can't sign in?"
                    </a>
                </p>
            </form>
        </>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Two-Factor View
// ─────────────────────────────────────────────────────────────────────────────

#[component]
fn TwoFactorView(
    email: String,
    totp_code: ReadSignal<String>,
    set_totp_code: WriteSignal<String>,
    loading: Signal<bool>,
    error: ReadSignal<Option<String>>,
    set_error: WriteSignal<Option<String>>,
    on_login_submit: impl Fn(leptos::ev::SubmitEvent) + Copy + Send + Sync + 'static,
    set_view_state: WriteSignal<LoginView>,
) -> impl IntoView {
    let verify_disabled = move || loading.get() || totp_code.get().len() != 6;

    view! {
        <div class="text-center space-y-6">
            <div>
                <h3 class="text-lg font-semibold text-foreground mb-2">"Two-Factor Authentication"</h3>
                <p class="text-sm text-muted-foreground">
                    "Enter the 6-digit code from your authenticator app to complete sign in"
                </p>
                <p class="text-xs text-muted-foreground mt-1">
                    "Signing in as: "
                    <span class="font-medium">{email.clone()}</span>
                </p>
            </div>

            // Error message for 2FA step
            <Show when=move || error.get().is_some()>
                <Alert variant=AlertVariant::Error>
                    <AlertDescription>
                        {move || error.get().unwrap_or_default()}
                    </AlertDescription>
                </Alert>
            </Show>

            <form on:submit=on_login_submit class="space-y-5">
                <div class="space-y-2">
                    <Label html_for="totp-code">"Verification Code"</Label>
                    <input
                        id="totp-code"
                        name="totp-code"
                        type="text"
                        autocomplete="one-time-code"
                        placeholder="000000"
                        maxlength="6"
                        required=true
                        autofocus=true
                        class=format!("{} h-12 text-center text-2xl tracking-widest font-mono", INPUT_CLASS)
                        prop:value=move || totp_code.get()
                        on:input=move |ev| {
                            // Digits only
                            let val: String = event_target_value(&ev)
                                .chars()
                                .filter(|c| c.is_ascii_digit())
                                .collect();
                            set_totp_code.set(val);
                        }
                    />
                </div>
                <Button
                    button_type="submit"
                    variant=ButtonVariant::Default
                    size=ButtonSize::Lg
                    disabled=Signal::derive(verify_disabled)
                    class="w-full"
                >
                    {move || {
                        if loading.get() {
                            view! {
                                <div class="flex items-center justify-center space-x-2">
                                    <Spinner class="text-primary-foreground"/>
                                    <span>"Verifying..."</span>
                                </div>
                            }.into_any()
                        } else {
                            view! { <span>"Verify & Sign In"</span> }.into_any()
                        }
                    }}
                </Button>
            </form>

            <Button
                variant=ButtonVariant::Link
                on:click=move |_| {
                    set_view_state.set(LoginView::Credentials);
                    set_error.set(None);
                    set_totp_code.set(String::new());
                    // Action::pending() auto-resets when the action completes or
                    // when no action is in-flight — no manual loading reset needed.
                }
            >
                "Back to login"
            </Button>
        </div>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Signup View
// ─────────────────────────────────────────────────────────────────────────────

#[component]
fn SignupView(
    signup_email: ReadSignal<String>,
    set_signup_email: WriteSignal<String>,
    signup_name: ReadSignal<String>,
    set_signup_name: WriteSignal<String>,
    signup_password: ReadSignal<String>,
    set_signup_password: WriteSignal<String>,
    signup_loading: Signal<bool>,
    error: ReadSignal<Option<String>>,
    set_error: WriteSignal<Option<String>>,
    is_self_hosted_no_smtp: impl Fn() -> bool + Copy + Send + Sync + 'static,
    on_signup_submit: impl Fn(leptos::ev::SubmitEvent) + Copy + Send + Sync + 'static,
    set_view_state: WriteSignal<LoginView>,
    show_passkey_section: impl Fn() -> bool + Copy + Send + Sync + 'static,
    passkey_signup_loading: Signal<bool>,
    on_passkey_signup_click: Callback<()>,
) -> impl IntoView {
    let passkey_signup_disabled =
        Signal::derive(move || signup_email.get().trim().is_empty());

    let signup_disabled = move || {
        if is_self_hosted_no_smtp() {
            signup_loading.get()
                || signup_email.get().trim().is_empty()
                || signup_name.get().trim().is_empty()
                || signup_password.get().len() < 8
        } else {
            signup_loading.get() || signup_email.get().trim().is_empty()
        }
    };

    view! {
        <div class="space-y-5">
            // Passkey Sign Up — same visual slot the passkey/Google buttons
            // occupy on the Credentials view, gated by the same
            // show_passkey_section condition (WebAuthn availability + the
            // `passkeys` auth-config flag).
            <Show when=show_passkey_section>
                <div class="space-y-3">
                    <PasskeySignInButton
                        loading=passkey_signup_loading
                        disabled=passkey_signup_disabled
                        on_click=on_passkey_signup_click
                        label="Sign up with Passkey"
                        loading_label="Sending signup link..."
                    />
                </div>
            </Show>

            <Show when=show_passkey_section>
                <AuthDivider text="or sign up with email"/>
            </Show>

            <form on:submit=on_signup_submit class="space-y-5">
                <div class="space-y-2">
                    <Label html_for="signup-email">"Email address"</Label>
                    <input
                        id="signup-email"
                        name="email"
                        type="email"
                        autocomplete="email"
                        placeholder="name@company.com"
                        class=INPUT_CLASS
                        required=true
                        prop:value=move || signup_email.get()
                        on:input=move |ev| set_signup_email.set(event_target_value(&ev))
                    />
                </div>

                // Self-hosted without SMTP: also show name + password
                <Show when=is_self_hosted_no_smtp>
                    <div class="space-y-2">
                        <Label html_for="signup-name">"Name"</Label>
                        <input
                            id="signup-name"
                            name="name"
                            type="text"
                            autocomplete="name"
                            placeholder="Your name"
                            class=INPUT_CLASS
                            required=true
                            prop:value=move || signup_name.get()
                            on:input=move |ev| set_signup_name.set(event_target_value(&ev))
                        />
                    </div>
                    <div class="space-y-2">
                        <Label html_for="signup-password">"Password"</Label>
                        <input
                            id="signup-password"
                            name="password"
                            type="password"
                            autocomplete="new-password"
                            placeholder="At least 8 characters"
                            class=INPUT_CLASS
                            required=true
                            minlength="8"
                            prop:value=move || signup_password.get()
                            on:input=move |ev| set_signup_password.set(event_target_value(&ev))
                        />
                    </div>
                </Show>

                // Error
                <Show when=move || error.get().is_some()>
                    <Alert variant=AlertVariant::Error>
                        <AlertDescription>
                            {move || error.get().unwrap_or_default()}
                        </AlertDescription>
                    </Alert>
                </Show>

                <Button
                    button_type="submit"
                    variant=ButtonVariant::Default
                    size=ButtonSize::Lg
                    disabled=Signal::derive(signup_disabled)
                    class="w-full"
                >
                    {move || {
                        if signup_loading.get() {
                            view! {
                                <div class="flex items-center justify-center space-x-2">
                                    <Spinner class="text-primary-foreground"/>
                                    <span>
                                        {if is_self_hosted_no_smtp() {
                                            "Creating account..."
                                        } else {
                                            "Sending verification..."
                                        }}
                                    </span>
                                </div>
                            }.into_any()
                        } else {
                            let label = if is_self_hosted_no_smtp() {
                                "Create Account"
                            } else {
                                "Sign up with Email"
                            };
                            view! { <span>{label}</span> }.into_any()
                        }
                    }}
                </Button>

                <Show when=move || !is_self_hosted_no_smtp()>
                    <p class="text-xs text-muted-foreground text-center">
                        "We'll send you an email to verify your address, then you'll set up your password."
                    </p>
                </Show>

                <p class="text-xs text-muted-foreground text-center mt-4">
                    "Already have an account? "
                    <Button
                        variant=ButtonVariant::Link
                        size=ButtonSize::Sm
                        // 44px min-height floor — scoped locally, mirrors
                        // the "Create an account" link on the Credentials view.
                        class="min-h-[44px]".to_string()
                        on:click=move |_| {
                            set_view_state.set(LoginView::Credentials);
                            set_error.set(None);
                            set_signup_email.set(String::new());
                        }
                    >
                        "Sign in"
                    </Button>
                </p>
            </form>
        </div>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Check Email View
// ─────────────────────────────────────────────────────────────────────────────

#[component]
fn CheckEmailView(
    email: String,
    set_view_state: WriteSignal<LoginView>,
    set_error: WriteSignal<Option<String>>,
    set_success_msg: WriteSignal<Option<String>>,
    set_signup_email: WriteSignal<String>,
) -> impl IntoView {
    view! {
        <div class="text-center space-y-4">
            <div class="inline-flex items-center justify-center w-16 h-16 rounded-full bg-primary/10 mb-2">
                <Icon icon=phosphor_leptos::ENVELOPE attr:class="w-8 h-8 text-primary"/>
            </div>
            <h3 class="text-xl font-semibold text-foreground">"Check Your Email"</h3>
            <p class="text-muted-foreground">
                "We sent a verification link to "
                <strong>{email}</strong>
            </p>
            <p class="text-muted-foreground">
                "Click the link in the email to complete your signup and set up your account."
            </p>
            <p class="text-sm text-muted-foreground">
                "The link expires in 1 hour."
            </p>
            <div class="pt-4">
                <Button
                    variant=ButtonVariant::Link
                    on:click=move |_| {
                        set_view_state.set(LoginView::Credentials);
                        set_error.set(None);
                        set_success_msg.set(None);
                        set_signup_email.set(String::new());
                    }
                >
                    "Back to login"
                </Button>
            </div>
        </div>
    }
}
