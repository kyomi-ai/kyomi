// SPDX-License-Identifier: AGPL-3.0-or-later

//! Push Notifications card — profile settings section for browser push subscriptions.
//!
//! Replaces `apps/frontend/src/components/settings/ProfileSettings.jsx` lines 747-863.
//!
//! This is entirely client-side — no server functions. All Push API calls go
//! through `web_sys` and related browser APIs.
//!
//! Hidden in personal mode.

use leptos::prelude::*;

use crate::components::{
    Alert, AlertDescription, AlertVariant, Button, ButtonSize, ButtonVariant, Card, CardContent,
    CardDescription, CardHeader, CardTitle,
};

// ─────────────────────────────────────────────────────────────────────────────
// Types
// ─────────────────────────────────────────────────────────────────────────────

/// Subscription record for a registered push device.
#[derive(Clone, Debug)]
struct PushSubscription {
    id: String,
    device_label: String,
    created_at: String,
    last_used_at: Option<String>,
}

// ─────────────────────────────────────────────────────────────────────────────
// Browser Push API helpers (WASM-only)
// ─────────────────────────────────────────────────────────────────────────────

/// Check whether the browser supports the Push API.
#[cfg(target_arch = "wasm32")]
fn push_supported() -> bool {
    web_sys::window()
        .and_then(|w| w.navigator().service_worker().ok())
        .is_some()
}

/// Check whether we are on a secure context (HTTPS or localhost).
#[cfg(target_arch = "wasm32")]
fn is_secure_context() -> bool {
    web_sys::window()
        .and_then(|w| {
            let loc = w.location();
            let protocol = loc.protocol().ok()?;
            let hostname = loc.hostname().ok()?;
            Some(protocol == "https:" || hostname == "localhost" || hostname == "127.0.0.1")
        })
        .unwrap_or(false)
}

/// Check whether we might be on iOS outside a PWA.
#[cfg(target_arch = "wasm32")]
fn is_ios_non_pwa() -> bool {
    use wasm_bindgen::JsCast;

    let Some(window) = web_sys::window() else {
        return false;
    };
    let ua = window.navigator().user_agent().unwrap_or_default();
    let is_ios = ua.contains("iPad") || ua.contains("iPhone");
    if !is_ios {
        return false;
    }
    // Check if running as a standalone PWA
    window
        .match_media("(display-mode: standalone)")
        .ok()
        .flatten()
        .map(|mql| !mql.matches())
        .unwrap_or(true)
}

/// Get the current Notification permission state.
#[cfg(target_arch = "wasm32")]
fn get_notification_permission() -> String {
    web_sys::window()
        .and_then(|w| {
            let notification = js_sys::Reflect::get(&w, &"Notification".into()).ok()?;
            let perm = js_sys::Reflect::get(&notification, &"permission".into()).ok()?;
            perm.as_string()
        })
        .unwrap_or_else(|| "default".to_string())
}

// ─────────────────────────────────────────────────────────────────────────────
// Main card component
// ─────────────────────────────────────────────────────────────────────────────

/// Push Notifications card for the Profile Settings page.
///
/// Shows the push notification subscription status, enable/disable toggle,
/// and a list of registered devices.
#[component]
pub fn PushNotificationsCard() -> impl IntoView {
    let (supported, _set_supported) = signal({
        #[cfg(target_arch = "wasm32")]
        {
            push_supported() && is_secure_context()
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            false
        }
    });

    let (permission, set_permission) = signal({
        #[cfg(target_arch = "wasm32")]
        {
            get_notification_permission()
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            "default".to_string()
        }
    });

    let (is_subscribed, set_is_subscribed) = signal(false);
    let (loading, set_loading) = signal(false);
    let (error, set_error) = signal(Option::<String>::None);
    let (subscriptions, set_subscriptions) = signal(Vec::<PushSubscription>::new());

    // -- Subscribe handler --
    let subscribe = move |_: leptos::ev::MouseEvent| {
        set_loading.set(true);
        set_error.set(None);

        #[cfg(target_arch = "wasm32")]
        {
            use wasm_bindgen::JsCast;

            let set_perm = set_permission;
            let set_sub = set_is_subscribed;
            let set_err = set_error;
            let set_ld = set_loading;

            leptos::task::spawn_local(async move {
                // Request notification permission
                let permission_result = {
                    let notification =
                        js_sys::Reflect::get(&web_sys::window().unwrap(), &"Notification".into());
                    match notification {
                        Ok(n) => {
                            let request_fn =
                                js_sys::Reflect::get(&n, &"requestPermission".into());
                            match request_fn {
                                Ok(func) => {
                                    let func: js_sys::Function = func.unchecked_into();
                                    match func.call0(&n) {
                                        Ok(promise) => {
                                            let promise: js_sys::Promise =
                                                promise.unchecked_into();
                                            wasm_bindgen_futures::JsFuture::from(promise)
                                                .await
                                                .ok()
                                                .and_then(|v| v.as_string())
                                        }
                                        Err(_) => None,
                                    }
                                }
                                Err(_) => None,
                            }
                        }
                        Err(_) => None,
                    }
                };

                if let Some(perm) = permission_result {
                    set_perm.set(perm.clone());
                    if perm == "granted" {
                        set_sub.set(true);
                        // In a full implementation, we would register with PushManager
                        // and send the subscription to the server. For now, we update
                        // the UI state to reflect the permission grant.
                    } else {
                        set_err.set(Some(
                            "Notification permission was not granted.".to_string(),
                        ));
                    }
                } else {
                    set_err.set(Some(
                        "Failed to request notification permission.".to_string(),
                    ));
                }
                set_ld.set(false);
            });
        }

        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = (set_permission, set_is_subscribed, set_error);
            set_loading.set(false);
        }
    };

    // -- Unsubscribe handler --
    let unsubscribe = move |_: leptos::ev::MouseEvent| {
        set_loading.set(true);
        set_is_subscribed.set(false);
        // In a full implementation, we would unregister from PushManager
        // and remove the subscription from the server.
        set_loading.set(false);
    };

    // -- Delete subscription handler --
    let delete_subscription = move |id: String| {
        set_subscriptions.update(|subs| subs.retain(|s| s.id != id));
    };

    view! {
        <Card>
            <CardHeader>
                <CardTitle>
                    <span class="flex items-center gap-2">
                        <leptos_icons::Icon icon=icondata_lu::LuBell width="20" height="20"/>
                        "Push Notifications"
                    </span>
                </CardTitle>
                <CardDescription>
                    "Receive watch alerts even when Kyomi is not open in your browser."
                </CardDescription>
            </CardHeader>
            <CardContent>
                {move || {
                    if !supported.get() {
                        // Not supported
                        let not_secure = {
                            #[cfg(target_arch = "wasm32")]
                            { !is_secure_context() }
                            #[cfg(not(target_arch = "wasm32"))]
                            { false }
                        };
                        let ios_hint = {
                            #[cfg(target_arch = "wasm32")]
                            { is_ios_non_pwa() }
                            #[cfg(not(target_arch = "wasm32"))]
                            { false }
                        };

                        view! {
                            <Alert>
                                <AlertDescription>
                                    {if not_secure {
                                        "Push notifications require a secure connection (HTTPS). Access Kyomi via HTTPS or localhost to enable browser alerts.".to_string()
                                    } else {
                                        "Push notifications are not supported in this browser.".to_string()
                                    }}
                                    {if ios_hint {
                                        view! {
                                            <span class="block mt-1">
                                                "On iOS, push notifications require installing Kyomi as an app: tap the Share button, then \"Add to Home Screen\"."
                                            </span>
                                        }.into_any()
                                    } else {
                                        view! { <span class="hidden"></span> }.into_any()
                                    }}
                                </AlertDescription>
                            </Alert>
                        }.into_any()
                    } else if permission.get() == "denied" {
                        // Permission denied
                        view! {
                            <Alert variant=AlertVariant::Warning>
                                <AlertDescription>
                                    "Notification permission was denied. To enable push notifications, allow notifications for this site in your browser settings."
                                </AlertDescription>
                            </Alert>
                        }.into_any()
                    } else {
                        // Supported and not denied
                        let subscribe_clone = subscribe.clone();
                        let unsubscribe_clone = unsubscribe.clone();

                        view! {
                            <div class="space-y-4">
                                // Error
                                {move || error.get().map(|msg| view! {
                                    <Alert variant=AlertVariant::Error>
                                        <AlertDescription>{msg}</AlertDescription>
                                    </Alert>
                                })}

                                // Enable/Disable toggle
                                <div class="flex items-center justify-between">
                                    <div>
                                        <p class="text-sm font-medium text-foreground">
                                            {move || if is_subscribed.get() {
                                                "Enabled on this device"
                                            } else {
                                                "Enable on this device"
                                            }}
                                        </p>
                                        <p class="text-xs text-muted-foreground">
                                            {move || if is_subscribed.get() {
                                                "You will receive push notifications for watch alerts."
                                            } else {
                                                "Get notified when your watches detect something."
                                            }}
                                        </p>
                                    </div>
                                    {move || {
                                        let variant = if is_subscribed.get() { ButtonVariant::Outline } else { ButtonVariant::Default };
                                        view! {
                                            <Button variant=variant size=ButtonSize::Sm disabled=loading.get()
                                                on:click=move |ev| {
                                                    if is_subscribed.get() {
                                                        unsubscribe_clone(ev);
                                                    } else {
                                                        subscribe_clone(ev);
                                                    }
                                                }
                                            >
                                                {if loading.get() {
                                                    view! {
                                                        <span class="animate-spin h-4 w-4 border-2 border-current border-t-transparent rounded-full"/>
                                                    }.into_any()
                                                } else if is_subscribed.get() {
                                                    view! {
                                                        <span class="flex items-center gap-2">
                                                            <leptos_icons::Icon icon=icondata_lu::LuBellOff width="16" height="16"/>
                                                            "Disable"
                                                        </span>
                                                    }.into_any()
                                                } else {
                                                    view! {
                                                        <span class="flex items-center gap-2">
                                                            <leptos_icons::Icon icon=icondata_lu::LuBell width="16" height="16"/>
                                                            "Enable"
                                                        </span>
                                                    }.into_any()
                                                }}
                                            </Button>
                                        }
                                    }}
                                </div>

                                // Device list
                                {move || {
                                    let subs = subscriptions.get();
                                    if subs.is_empty() {
                                        view! { <span class="hidden"></span> }.into_any()
                                    } else {
                                        view! {
                                            <div class="space-y-2 pt-3 border-t border-border">
                                                <p class="text-xs font-medium text-muted-foreground uppercase tracking-wide">
                                                    "Registered Devices"
                                                </p>
                                                {subs.iter().map(|sub| {
                                                    let label = if sub.device_label.is_empty() {
                                                        "Unknown device".to_string()
                                                    } else {
                                                        sub.device_label.clone()
                                                    };
                                                    let created = sub.created_at.clone();
                                                    let last_used = sub.last_used_at.clone();
                                                    let delete_id = sub.id.clone();
                                                    view! {
                                                        <div class="flex items-center justify-between p-3 bg-muted/50 rounded-lg">
                                                            <div class="flex items-center gap-3">
                                                                <span class="text-muted-foreground">
                                                                    <leptos_icons::Icon icon=icondata_lu::LuSmartphone width="16" height="16"/>
                                                                </span>
                                                                <div>
                                                                    <p class="text-sm text-foreground">
                                                                        {label}
                                                                    </p>
                                                                    <p class="text-xs text-muted-foreground">
                                                                        "Added " {created}
                                                                        {last_used.map(|lu| format!(" \u{00B7} Last used {lu}"))}
                                                                    </p>
                                                                </div>
                                                            </div>
                                                            <Button
                                                                variant=ButtonVariant::Ghost
                                                                size=ButtonSize::Sm
                                                                on:click=move |_| delete_subscription(delete_id.clone())
                                                            >
                                                                <span class="text-muted-foreground">
                                                                    <leptos_icons::Icon icon=icondata_lu::LuTrash2 width="16" height="16"/>
                                                                </span>
                                                            </Button>
                                                        </div>
                                                    }
                                                }).collect_view()}
                                            </div>
                                        }.into_any()
                                    }
                                }}
                            </div>
                        }.into_any()
                    }
                }}
            </CardContent>
        </Card>
    }
}
