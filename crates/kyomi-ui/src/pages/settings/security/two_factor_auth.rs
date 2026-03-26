// SPDX-License-Identifier: AGPL-3.0-or-later

//! Two-Factor Authentication card — security settings section for TOTP 2FA.
//!
//! Replaces `apps/frontend/src/components/TwoFactorAuth.jsx` (307 lines).
//!
//! Shows:
//! - Status card: enabled (green badge) / disabled (gray badge) with enable/disable button
//! - Setup flow: QR code display, manual secret entry, verification code input
//! - Uses StatusBadge for enabled/disabled indicator
//! - Uses Card, Button, Alert, Label, INPUT_CLASS

use leptos::prelude::*;
use leptos_icons::Icon;

use crate::components::{
    Alert, AlertDescription, AlertVariant, Button, ButtonVariant, Card, CardContent,
    CardDescription, CardHeader, CardTitle, Label, Skeleton, StatusBadge, StatusBadgeVariant,
    INPUT_CLASS,
};
use crate::server_fns::security::{disable_totp, enable_totp, get_totp_status, setup_totp};

/// View state for the TwoFactorAuth component.
#[derive(Clone, Copy, PartialEq)]
enum View {
    /// Shows status card with enable/disable button.
    Status,
    /// Shows QR code, secret, and verification code input.
    Setup,
}

/// Two-Factor Authentication component.
///
/// Loads TOTP status on mount, then shows the status card. User can start
/// setup flow (QR code + verification) or disable flow (code confirmation).
#[component]
pub fn TwoFactorAuth() -> impl IntoView {
    // ── Server data ──────────────────────────────────────────────────────
    let totp_status = Resource::new(|| (), |_| get_totp_status());

    // ── UI state ─────────────────────────────────────────────────────────
    let current_view = RwSignal::new(View::Status);
    let verification_code = RwSignal::new(String::new());
    let setup_data = RwSignal::new(Option::<crate::server_fns::security::TotpSetup>::None);
    let setup_loading = RwSignal::new(false);
    let enable_loading = RwSignal::new(false);
    let disable_loading = RwSignal::new(false);
    let error = RwSignal::new(Option::<String>::None);
    let success = RwSignal::new(Option::<String>::None);
    let copied = RwSignal::new(false);

    // ── Derived signals ──────────────────────────────────────────────────
    let is_enabled = move || {
        totp_status
            .get()
            .and_then(|r| r.ok())
            .map(|s| s.enabled)
            .unwrap_or(false)
    };

    // ── Handlers ─────────────────────────────────────────────────────────
    let handle_setup = move |_| {
        setup_loading.set(true);
        error.set(None);
        success.set(None);

        leptos::task::spawn_local(async move {
            match setup_totp().await {
                Ok(data) => {
                    setup_data.set(Some(data));
                    current_view.set(View::Setup);
                }
                Err(e) => {
                    error.set(Some(e.to_string()));
                }
            }
            setup_loading.set(false);
        });
    };

    let handle_enable = move |_| {
        let code = verification_code.get();
        if code.trim().is_empty() {
            error.set(Some("Please enter the verification code".to_string()));
            return;
        }

        enable_loading.set(true);
        error.set(None);

        leptos::task::spawn_local(async move {
            match enable_totp(code).await {
                Ok(message) => {
                    success.set(Some(message));
                    current_view.set(View::Status);
                    setup_data.set(None);
                    verification_code.set(String::new());
                    totp_status.refetch();
                }
                Err(e) => {
                    error.set(Some(e.to_string()));
                }
            }
            enable_loading.set(false);
        });
    };

    let handle_disable = move |_| {
        disable_loading.set(true);
        error.set(None);
        success.set(None);

        leptos::task::spawn_local(async move {
            match disable_totp().await {
                Ok(message) => {
                    success.set(Some(message));
                    totp_status.refetch();
                }
                Err(e) => {
                    error.set(Some(e.to_string()));
                }
            }
            disable_loading.set(false);
        });
    };

    let handle_cancel = move |_| {
        current_view.set(View::Status);
        setup_data.set(None);
        verification_code.set(String::new());
        error.set(None);
    };

    let handle_copy_secret = move |_| {
        let Some(data) = setup_data.get() else {
            return;
        };
        let secret = data.secret.clone();

        #[cfg(target_arch = "wasm32")]
        {
            leptos::task::spawn_local(async move {
                if let Some(window) = web_sys::window() {
                    let clipboard = window.navigator().clipboard();
                    let promise = clipboard.write_text(&secret);
                    let _ = wasm_bindgen_futures::JsFuture::from(promise).await;
                    copied.set(true);
                    // Reset after 2 seconds
                    gloo_timers::future::TimeoutFuture::new(2_000).await;
                    copied.set(false);
                }
            });
        }

        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = (secret, copied);
        }
    };

    view! {
        <Transition fallback=|| view! { <Skeleton class="h-24 w-full" /> }>
            // Error alert
            {move || error.get().map(|msg| view! {
                <Alert variant=AlertVariant::Error>
                    <AlertDescription>{msg}</AlertDescription>
                </Alert>
            })}

            // Success alert
            {move || success.get().map(|msg| view! {
                <Alert variant=AlertVariant::Success>
                    <AlertDescription>{msg}</AlertDescription>
                </Alert>
            })}

            // ── Status view ──────────────────────────────────────────
            <Show when=move || current_view.get() == View::Status>
                <Card>
                    <CardHeader>
                        <div class="flex items-center justify-between">
                            <div>
                                <CardTitle>
                                    "Two-Factor Authentication"
                                    {move || {
                                        if is_enabled() {
                                            Some(view! {
                                                <StatusBadge
                                                    variant=StatusBadgeVariant::Success
                                                    class="ml-3 align-middle"
                                                >
                                                    "Enabled"
                                                </StatusBadge>
                                            })
                                        } else {
                                            None
                                        }
                                    }}
                                </CardTitle>
                                <CardDescription>
                                    {move || {
                                        if is_enabled() {
                                            "Protect your account with TOTP codes from authenticator apps"
                                        } else {
                                            "Add an extra layer of security with time-based codes"
                                        }
                                    }}
                                </CardDescription>
                            </div>
                            <div class="flex items-center space-x-2">
                                <Show
                                    when=move || is_enabled()
                                    fallback=move || view! {
                                        <Button
                                            attr:disabled=move || setup_loading.get()
                                            on:click=handle_setup
                                        >
                                            <Icon icon=icondata_lu::LuShield width="16" height="16"/>
                                            <span>
                                                {move || if setup_loading.get() { "Setting up..." } else { "Setup 2FA" }}
                                            </span>
                                        </Button>
                                    }
                                >
                                    <Button
                                        variant=ButtonVariant::Outline
                                        attr:disabled=move || disable_loading.get()
                                        on:click=handle_disable
                                    >
                                        {move || if disable_loading.get() { "Disabling..." } else { "Disable 2FA" }}
                                    </Button>
                                </Show>
                            </div>
                        </div>
                    </CardHeader>

                    <Show when=move || !is_enabled()>
                        <CardContent>
                            <Alert variant=AlertVariant::Info>
                                <AlertDescription>
                                    <p class="font-medium mb-1">"Why enable 2FA?"</p>
                                    <ul class="list-disc list-inside space-y-1">
                                        <li>"Adds an extra layer of security to your account"</li>
                                        <li>"Works with Google Authenticator, Authy, and other TOTP apps"</li>
                                        <li>"Protects your account even if your password is compromised"</li>
                                    </ul>
                                </AlertDescription>
                            </Alert>
                        </CardContent>
                    </Show>
                </Card>
            </Show>

            // ── Setup view ───────────────────────────────────────────
            <Show when=move || current_view.get() == View::Setup>
                {move || setup_data.get().map(|data| view! {
                    <Card>
                        <CardHeader>
                            <CardTitle>"Setup Two-Factor Authentication"</CardTitle>
                            <CardDescription>
                                "Scan the QR code with your authenticator app (Google Authenticator, Authy, etc.) or enter the key manually"
                            </CardDescription>
                        </CardHeader>
                        <CardContent>
                            <div class="space-y-6">
                                <div class="flex flex-col items-center space-y-4">
                                    <div class="p-4 bg-background border border-border rounded-lg">
                                        <img
                                            src=data.qr_uri.clone()
                                            alt="2FA QR Code"
                                            class="w-48 h-48"
                                        />
                                    </div>

                                    <div class="w-full max-w-md">
                                        <Label>"Or enter this key manually:"</Label>
                                        <div class="flex items-center space-x-2 mt-2">
                                            <input
                                                type="text"
                                                class=format!("{INPUT_CLASS} flex-1 font-mono")
                                                prop:value=data.secret.clone()
                                                readonly
                                            />
                                            <button
                                                type="button"
                                                class="h-9 w-9 inline-flex items-center justify-center rounded-md text-foreground hover:bg-accent hover:text-accent-foreground"
                                                on:click=handle_copy_secret
                                                aria-label="Copy to clipboard"
                                            >
                                                <Show
                                                    when=move || copied.get()
                                                    fallback=|| view! {
                                                        <Icon icon=icondata_lu::LuCopy width="16" height="16"/>
                                                    }
                                                >
                                                    <Icon icon=icondata_lu::LuCheck width="16" height="16"/>
                                                </Show>
                                            </button>
                                        </div>
                                    </div>
                                </div>

                                <div class="border-t border-border pt-6">
                                    <div class="max-w-md mx-auto space-y-4">
                                        <div class="space-y-2">
                                            <Label html_for="verification-code">
                                                "Enter the 6-digit code from your authenticator app:"
                                            </Label>
                                            <input
                                                id="verification-code"
                                                type="text"
                                                class=format!("{INPUT_CLASS} text-center text-lg font-mono tracking-wider")
                                                placeholder="000000"
                                                maxlength="6"
                                                prop:value=move || verification_code.get()
                                                on:input=move |ev| {
                                                    verification_code.set(event_target_value(&ev));
                                                }
                                            />
                                        </div>

                                        <div class="flex space-x-3">
                                            <Button
                                                variant=ButtonVariant::Outline
                                                class="flex-1"
                                                on:click=handle_cancel
                                            >
                                                "Cancel"
                                            </Button>
                                            <Button
                                                class="flex-1"
                                                attr:disabled=move || {
                                                    enable_loading.get() || verification_code.get().len() != 6
                                                }
                                                on:click=handle_enable
                                            >
                                                {move || if enable_loading.get() { "Enabling..." } else { "Enable 2FA" }}
                                            </Button>
                                        </div>
                                    </div>
                                </div>
                            </div>
                        </CardContent>
                    </Card>
                })}
            </Show>

        </Transition>
    }
}
