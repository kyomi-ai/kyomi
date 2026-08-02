// SPDX-License-Identifier: AGPL-3.0-or-later

//! Passkey Manager card — security settings section for passkey management.
//!
//! Replaces `apps/frontend/src/components/PasskeyManager.jsx` (441 lines).
//!
//! Shows:
//! - List of registered passkeys with name, created date, last used date
//! - "Add Passkey" button that triggers WebAuthn registration flow
//! - Rename button (opens Modal with input)
//! - Delete button (with ConfirmDialog)
//! - Device detection icons based on the passkey name
//! - Empty state with illustration when no passkeys exist
//! - Tip alert when only one passkey is registered

use leptos::prelude::*;
use leptos::server_fn::ServerFnError;
use phosphor_leptos::{Icon, IconWeight};
use crate::components::{
    Alert, AlertDescription, AlertVariant, Button, ButtonSize, ButtonVariant, Card, CardContent,
    CardDescription, CardHeader, CardTitle, ConfirmDialog, EmptyState, Modal, ModalSize, INPUT_CLASS,
};
use crate::server_fns::security::{
    delete_passkey, list_passkeys, rename_passkey, PasskeyInfo,
};
#[cfg(target_arch = "wasm32")]
use crate::server_fns::security::{complete_passkey_registration, start_passkey_registration};

/// Format an RFC 3339 date string as "Mon DD, YYYY".
///
/// Uses JS `Date.toLocaleDateString()` on the client (matches React exactly),
/// and falls back to a truncated ISO string on the server.
fn format_date(date_str: &str) -> String {
    #[cfg(target_arch = "wasm32")]
    {
        use js_sys::{Date, Intl, Object, Reflect};
        use wasm_bindgen::JsValue;

        let date = Date::new(&JsValue::from_str(date_str));
        if date.get_time().is_nan() {
            return "Unknown".to_string();
        }

        let options = Object::new();
        let _ = Reflect::set(&options, &"month".into(), &"short".into());
        let _ = Reflect::set(&options, &"day".into(), &"numeric".into());
        let _ = Reflect::set(&options, &"year".into(), &"numeric".into());

        let locale = js_sys::Array::of1(&"en-US".into());
        let formatter = Intl::DateTimeFormat::new(&locale, &options);
        let format_fn = formatter.format();
        format_fn
            .call1(&wasm_bindgen::JsValue::NULL, &date)
            .ok()
            .and_then(|v| v.as_string())
            .unwrap_or_else(|| "Unknown".to_string())
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        date_str
            .get(..10)
            .unwrap_or("Unknown")
            .to_string()
    }
}

/// Format an RFC 3339 date string as a relative time (e.g. "5 min ago", "2 days ago").
///
/// Falls back to `format_date()` for dates older than 7 days.
fn format_relative_time(date_str: &str) -> String {
    #[cfg(target_arch = "wasm32")]
    {
        use js_sys::Date;
        use wasm_bindgen::JsValue;

        let date = Date::new(&JsValue::from_str(date_str));
        if date.get_time().is_nan() {
            return "Unknown".to_string();
        }

        let now = Date::new_0();
        let diff_ms = now.get_time() - date.get_time();
        let diff_mins = (diff_ms / 60_000.0) as i64;
        let diff_hours = (diff_ms / 3_600_000.0) as i64;
        let diff_days = (diff_ms / 86_400_000.0) as i64;

        if diff_mins < 1 {
            "Just now".to_string()
        } else if diff_mins < 60 {
            format!("{diff_mins} min ago")
        } else if diff_hours < 24 {
            if diff_hours == 1 {
                "1 hour ago".to_string()
            } else {
                format!("{diff_hours} hours ago")
            }
        } else if diff_days < 7 {
            if diff_days == 1 {
                "1 day ago".to_string()
            } else {
                format!("{diff_days} days ago")
            }
        } else {
            format_date(date_str)
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        format_date(date_str)
    }
}

/// Determine which icon to show based on device name.
///
/// Matches the React `getDeviceIcon()` logic:
/// - "iphone", "android", "ipad" -> Smartphone
/// - "mac", "windows", "linux" -> Monitor
/// - default -> Key
enum DeviceIconKind {
    Key,
    Smartphone,
    Monitor,
}

fn detect_device_icon(name: &str) -> DeviceIconKind {
    let lower = name.to_lowercase();
    if lower.contains("iphone") || lower.contains("android") || lower.contains("ipad") {
        DeviceIconKind::Smartphone
    } else if lower.contains("mac") || lower.contains("windows") || lower.contains("linux") {
        DeviceIconKind::Monitor
    } else {
        DeviceIconKind::Key
    }
}

/// Check if the browser supports WebAuthn (passkeys).
///
/// Returns true if `navigator.credentials` and `PublicKeyCredential` are available.
/// This check only runs in the browser (WASM).
fn check_webauthn_support() -> bool {
    #[cfg(target_arch = "wasm32")]
    {
        use wasm_bindgen::JsValue;

        let window = match web_sys::window() {
            Some(w) => w,
            None => return false,
        };

        // Check if PublicKeyCredential exists on window
        let pk_cred = js_sys::Reflect::get(&window, &JsValue::from_str("PublicKeyCredential"));
        matches!(pk_cred, Ok(val) if !val.is_undefined() && !val.is_null())
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        // On SSR, assume supported (client will re-check during hydration)
        true
    }
}

/// Start WebAuthn registration in the browser using `navigator.credentials.create()`.
///
/// Takes the JSON options string from `start_passkey_registration()`,
/// calls the browser API, and returns the credential JSON to send back.
#[cfg(target_arch = "wasm32")]
async fn browser_create_credential(options_json: &str) -> Result<String, String> {
    use js_sys::{Object, Promise, Reflect, Uint8Array};
    use wasm_bindgen::prelude::*;
    use wasm_bindgen_futures::JsFuture;

    // Parse the server response to get challenge_id and options
    let server_response: serde_json::Value =
        serde_json::from_str(options_json).map_err(|e| format!("Parse options: {e}"))?;

    let challenge_id = server_response["challenge_id"]
        .as_str()
        .ok_or("Missing challenge_id")?
        .to_string();

    let options = &server_response["options"];

    // Convert to JS and fix up the binary fields (challenge, user.id, excludeCredentials[].id)
    // that need to be ArrayBuffer, not base64url strings.
    let js_options = js_value_from_json(options)
        .map_err(|e| format!("Convert options to JS: {e:?}"))?;

    let js_pub_key = Reflect::get(&js_options, &"publicKey".into())
        .unwrap_or(js_options.clone());

    // Decode base64url challenge -> ArrayBuffer
    if let Ok(challenge_val) = Reflect::get(&js_pub_key, &"challenge".into())
        && let Some(challenge_str) = challenge_val.as_string() {
            let bytes = base64url_decode(&challenge_str)?;
            let arr = Uint8Array::from(&bytes[..]);
            let _ = Reflect::set(&js_pub_key, &"challenge".into(), &arr.buffer());
        }

    // Decode user.id -> ArrayBuffer
    if let Ok(user) = Reflect::get(&js_pub_key, &"user".into())
        && let Ok(user_id_val) = Reflect::get(&user, &"id".into())
            && let Some(user_id_str) = user_id_val.as_string() {
                let bytes = base64url_decode(&user_id_str)?;
                let arr = Uint8Array::from(&bytes[..]);
                let _ = Reflect::set(&user, &"id".into(), &arr.buffer());
            }

    // Decode excludeCredentials[].id -> ArrayBuffer
    if let Ok(exclude_creds) = Reflect::get(&js_pub_key, &"excludeCredentials".into())
        && js_sys::Array::is_array(&exclude_creds) {
            let arr = js_sys::Array::from(&exclude_creds);
            for i in 0..arr.length() {
                let cred = arr.get(i);
                if let Ok(id_val) = Reflect::get(&cred, &"id".into())
                    && let Some(id_str) = id_val.as_string() {
                        let bytes = base64url_decode(&id_str)?;
                        let u8arr = Uint8Array::from(&bytes[..]);
                        let _ = Reflect::set(&cred, &"id".into(), &u8arr.buffer());
                    }
            }
        }

    // Build the CredentialCreationOptions object with { publicKey: ... }
    let create_options = Object::new();
    let _ = Reflect::set(&create_options, &"publicKey".into(), &js_pub_key);

    // Call navigator.credentials.create(options)
    let window = web_sys::window().ok_or("No window")?;
    let navigator = window.navigator();
    let credentials = Reflect::get(&navigator, &"credentials".into())
        .map_err(|_| "No credentials API")?;

    let create_fn = Reflect::get(&credentials, &"create".into())
        .map_err(|_| "No create method")?;

    let create_fn: js_sys::Function = create_fn
        .dyn_into()
        .map_err(|_| "create is not a function")?;

    let promise: Promise = create_fn
        .call1(&credentials, &create_options)
        .map_err(|e| format!("credentials.create() failed: {e:?}"))?
        .dyn_into()
        .map_err(|_| "create did not return a Promise")?;

    let result = JsFuture::from(promise)
        .await
        .map_err(|e| {
            // Extract WebAuthn-specific error names for user-friendly messages
            let err_name = Reflect::get(&e, &"name".into())
                .ok()
                .and_then(|v| v.as_string())
                .unwrap_or_default();
            match err_name.as_str() {
                "InvalidStateError" => "A passkey already exists for this device. Please try with a different device or remove the existing passkey first.".to_string(),
                "NotAllowedError" => "Passkey creation was cancelled or timed out. Please try again.".to_string(),
                "AbortError" => "Passkey creation was cancelled. Please try again.".to_string(),
                "NotSupportedError" => "Your device does not support this type of passkey.".to_string(),
                _ => format!("WebAuthn error: {e:?}"),
            }
        })?;

    // Extract the credential response fields and encode as base64url
    let credential_id = Reflect::get(&result, &"id".into())
        .map_err(|_| "Missing credential id")?
        .as_string()
        .ok_or("credential id is not a string")?;

    let raw_id = Reflect::get(&result, &"rawId".into())
        .map_err(|_| "Missing rawId")?;
    let raw_id_b64 = arraybuffer_to_base64url(&raw_id)?;

    let response = Reflect::get(&result, &"response".into())
        .map_err(|_| "Missing response")?;

    let client_data_json = Reflect::get(&response, &"clientDataJSON".into())
        .map_err(|_| "Missing clientDataJSON")?;
    let client_data_b64 = arraybuffer_to_base64url(&client_data_json)?;

    let attestation_object = Reflect::get(&response, &"attestationObject".into())
        .map_err(|_| "Missing attestationObject")?;
    let attestation_b64 = arraybuffer_to_base64url(&attestation_object)?;

    // Get credential type
    let cred_type = Reflect::get(&result, &"type".into())
        .ok()
        .and_then(|v| v.as_string())
        .unwrap_or_else(|| "public-key".to_string());

    // Get authenticator attachment (if available)
    let authenticator_attachment = Reflect::get(&result, &"authenticatorAttachment".into())
        .ok()
        .and_then(|v| v.as_string());

    // Get transports (if available via getTransports())
    let transports: Option<Vec<String>> = {
        let get_transports = Reflect::get(&response, &"getTransports".into()).ok();
        get_transports.and_then(|gt| {
            if gt.is_function() {
                let func: js_sys::Function = gt.dyn_into().ok()?;
                let result = func.call0(&response).ok()?;
                let arr = js_sys::Array::from(&result);
                let mut transports = Vec::new();
                for i in 0..arr.length() {
                    if let Some(s) = arr.get(i).as_string() {
                        transports.push(s);
                    }
                }
                Some(transports)
            } else {
                None
            }
        })
    };

    // Build the RegisterPublicKeyCredential JSON that webauthn-rs expects
    let mut credential_json = serde_json::json!({
        "id": credential_id,
        "rawId": raw_id_b64,
        "type": cred_type,
        "response": {
            "clientDataJSON": client_data_b64,
            "attestationObject": attestation_b64,
        },
    });

    if let Some(attachment) = authenticator_attachment {
        credential_json["authenticatorAttachment"] = serde_json::json!(attachment);
    }

    if let Some(transports) = transports {
        credential_json["response"]["transports"] = serde_json::json!(transports);
    }

    // Combine with challenge_id for the server
    let final_json = serde_json::json!({
        "challenge_id": challenge_id,
        "credential": credential_json,
    });

    serde_json::to_string(&final_json).map_err(|e| format!("Serialize credential: {e}"))
}

/// Decode a base64url string (with or without padding) to bytes.
#[cfg(target_arch = "wasm32")]
fn base64url_decode(input: &str) -> Result<Vec<u8>, String> {
    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
    URL_SAFE_NO_PAD
        .decode(input.trim_end_matches('='))
        .map_err(|e| format!("base64url decode: {e}"))
}

/// Convert an ArrayBuffer (JS) to a base64url-no-pad string.
#[cfg(target_arch = "wasm32")]
fn arraybuffer_to_base64url(value: &wasm_bindgen::JsValue) -> Result<String, String> {
    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
    use js_sys::Uint8Array;

    let arr = Uint8Array::new(value);
    let bytes = arr.to_vec();
    Ok(URL_SAFE_NO_PAD.encode(&bytes))
}

/// Convert a serde_json::Value to a JsValue.
#[cfg(target_arch = "wasm32")]
fn js_value_from_json(value: &serde_json::Value) -> Result<wasm_bindgen::JsValue, wasm_bindgen::JsValue> {
    let json_str = serde_json::to_string(value)
        .map_err(|e| wasm_bindgen::JsValue::from_str(&e.to_string()))?;
    js_sys::JSON::parse(&json_str)
}

/// Passkey Manager component.
///
/// Lists registered passkeys and allows adding, renaming, and deleting them.
/// The WebAuthn browser API interaction is handled via `web_sys`/`js_sys`.
#[component]
pub fn PasskeyManager() -> impl IntoView {
    // ── Server data ──────────────────────────────────────────────────────
    let passkeys_resource = Resource::new(|| (), |_| list_passkeys());

    // ── UI state ─────────────────────────────────────────────────────────
    let loading = RwSignal::new(true);
    let passkeys = RwSignal::new(Vec::<PasskeyInfo>::new());
    let error = RwSignal::new(Option::<String>::None);
    let success = RwSignal::new(Option::<String>::None);
    let is_supported = RwSignal::new(true);

    // Rename modal state
    let rename_modal_open = RwSignal::new(false);
    let rename_credential_id = RwSignal::new(Option::<String>::None);
    let rename_device_name = RwSignal::new(String::new());

    // Add passkey modal state
    let add_modal_open = RwSignal::new(false);
    let add_device_name = RwSignal::new(String::new());
    let add_loading = RwSignal::new(false);

    // Delete confirm dialog state
    let dialog_open = RwSignal::new(false);
    let dialog_title = RwSignal::new(String::new());
    let dialog_message = RwSignal::new(String::new());
    let pending_delete_id = RwSignal::new(Option::<String>::None);

    // ── Check WebAuthn support ───────────────────────────────────────────
    Effect::new(move || {
        is_supported.set(check_webauthn_support());
    });

    // ── Sync resource into signals ───────────────────────────────────────
    Effect::new(move || {
        if let Some(result) = passkeys_resource.get() {
            loading.set(false);
            match result {
                Ok(data) => {
                    error.set(None);
                    passkeys.set(data);
                }
                Err(e) => {
                    error.set(Some(format!("Failed to load passkeys: {e}")));
                }
            }
        }
    });

    // ── Clear success message after timeout ──────────────────────────────
    Effect::new(move || {
        if success.get().is_some() {
            #[cfg(target_arch = "wasm32")]
            {
                use gloo_timers::callback::Timeout;
                let timeout = Timeout::new(5_000, move || {
                    success.try_set(None);
                });
                timeout.forget();
            }
        }
    });

    // ── Actions ──────────────────────────────────────────────────────────
    // `rename_passkey` and `delete_passkey` are plain server fns with no
    // `!Send` involvement, so both are `Action`s (KYO-227) — reactive
    // fallout lives in `Effect`s that stop running once the component's
    // scope is gone, instead of `try_set` calls scattered through a
    // detached future. `handle_add_passkey` below stays on `spawn_local`:
    // it drives WebAuthn's `navigator.credentials.create()` via `JsFuture`,
    // which is `!Send` and can't be awaited inside `Action::new`.
    let rename_action: Action<(String, String), Result<String, ServerFnError>> =
        Action::new(move |(cred_id, name): &(String, String)| {
            let cred_id = cred_id.clone();
            let name = name.clone();
            async move { rename_passkey(cred_id, name).await }
        });
    Effect::new(move |_| {
        if let Some(result) = rename_action.value().get() {
            match result {
                Ok(message) => {
                    success.set(Some(message));
                    rename_modal_open.set(false);
                    rename_credential_id.set(None);
                    rename_device_name.set(String::new());
                    // Reload passkeys
                    loading.set(true);
                    passkeys_resource.refetch();
                }
                Err(e) => {
                    error.set(Some(e.to_string()));
                }
            }
        }
    });

    // `cred_id` is the Action's input, threaded from `on_confirm_delete`'s
    // dispatch below — not read live inside the async body or the Effect.
    let delete_action: Action<String, Result<String, ServerFnError>> =
        Action::new(move |cred_id: &String| {
            let cred_id = cred_id.clone();
            async move { delete_passkey(cred_id).await }
        });
    Effect::new(move |_| {
        if let Some(result) = delete_action.value().get() {
            match result {
                Ok(message) => {
                    success.set(Some(message));
                    passkeys_resource.refetch();
                }
                Err(e) => {
                    error.set(Some(e.to_string()));
                    loading.set(false);
                }
            }
        }
    });

    // ── Refresh handler ──────────────────────────────────────────────────
    let handle_refresh = move |_| {
        loading.set(true);
        error.set(None);
        passkeys_resource.refetch();
    };

    // ── Add passkey handler ──────────────────────────────────────────────
    let handle_add_passkey = move |_: leptos::ev::MouseEvent| {
        add_loading.set(true);
        error.set(None);
        success.set(None);

        let device_name = add_device_name.get_untracked();

        // Exempt from the Action conversion: `add_passkey_flow` calls
        // `browser_create_credential`, which drives WebAuthn via
        // `navigator.credentials.create()` awaited through `JsFuture` — a
        // `!Send` browser API on wasm32. `Action::new` requires a `Send`
        // future, so this stays `spawn_local` (KYO-227 scope).
        leptos::task::spawn_local(async move {
            let result = add_passkey_flow(&device_name).await;

            add_loading.try_set(false);

            match result {
                Ok(msg) => {
                    success.try_set(Some(msg));
                    add_modal_open.try_set(false);
                    add_device_name.try_set(String::new());
                    // Reload passkeys
                    loading.try_set(true);
                    passkeys_resource.refetch();
                }
                Err(e) => {
                    error.try_set(Some(e));
                }
            }
        });
    };

    // ── Rename handler ───────────────────────────────────────────────────
    let handle_rename = move |_: leptos::ev::MouseEvent| {
        let name = rename_device_name.get_untracked();
        if name.trim().is_empty() {
            error.set(Some("Device name cannot be empty".to_string()));
            return;
        }

        let cred_id = match rename_credential_id.get_untracked() {
            Some(id) => id,
            None => return,
        };

        error.set(None);
        success.set(None);

        rename_action.dispatch((cred_id, name.trim().to_string()));
    };

    // ── Delete flow ──────────────────────────────────────────────────────
    let open_delete_dialog = move |cred_id: String, device_name: String| {
        dialog_title.set("Delete Passkey?".to_string());
        dialog_message.set(format!(
            "Are you sure you want to delete \"{}\"? You will no longer be able to sign in with this passkey.",
            device_name
        ));
        pending_delete_id.set(Some(cred_id));
        dialog_open.set(true);
    };

    let on_confirm_delete = Callback::new(move |()| {
        dialog_open.set(false);
        let cred_id = match pending_delete_id.get_untracked() {
            Some(id) => id,
            None => return,
        };
        pending_delete_id.set(None);

        loading.set(true);
        error.set(None);
        success.set(None);

        delete_action.dispatch(cred_id);
    });

    let on_cancel_delete = Callback::new(move |()| {
        dialog_open.set(false);
        pending_delete_id.set(None);
    });

    // ── Open rename modal ────────────────────────────────────────────────
    let open_rename_modal = move |cred_id: String, current_name: String| {
        rename_credential_id.set(Some(cred_id));
        rename_device_name.set(current_name);
        rename_modal_open.set(true);
    };

    view! {
        <Card>
            <CardHeader>
                <div class="flex items-center justify-between">
                    <div>
                        <CardTitle>"Passkeys"</CardTitle>
                        <CardDescription>
                            "Passkeys let you sign in securely without a password using your device's biometrics."
                        </CardDescription>
                    </div>
                    <button
                        class="inline-flex items-center justify-center gap-2 whitespace-nowrap rounded-md text-sm font-medium transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:pointer-events-none disabled:opacity-50 border border-input bg-background text-foreground shadow-sm hover:bg-secondary hover:text-accent-foreground h-9 w-9"
                        on:click=handle_refresh
                        disabled=move || loading.get()
                        title="Refresh passkeys"
                    >
                        <span class=move || if loading.get() { "animate-spin" } else { "" }>
                            <Icon icon=phosphor_leptos::ARROWS_CLOCKWISE size="16px"/>
                        </span>
                    </button>
                </div>
            </CardHeader>
            <CardContent>
                // Error alert
                <Show when=move || error.get().is_some()>
                    <div class="mb-6">
                        <Alert variant=AlertVariant::Error>
                            <AlertDescription>
                                {move || error.get().unwrap_or_default()}
                            </AlertDescription>
                        </Alert>
                    </div>
                </Show>

                // Success alert
                <Show when=move || success.get().is_some()>
                    <div class="mb-6">
                        <Alert variant=AlertVariant::Success>
                            <AlertDescription>
                                {move || success.get().unwrap_or_default()}
                            </AlertDescription>
                        </Alert>
                    </div>
                </Show>

                <div class="mb-6">
                    // Loading state (no passkeys loaded yet)
                    <Show
                        when=move || !(loading.get() && passkeys.get().is_empty())
                        fallback=|| view! {
                            <div class="text-center py-8">
                                <span class="animate-spin h-8 w-8 border-2 border-primary border-t-transparent rounded-full inline-block"/>
                                <p class="text-muted-foreground mt-2">"Loading passkeys..."</p>
                            </div>
                        }
                    >
                        // Empty state
                        <Show
                            when=move || !passkeys.get().is_empty()
                            fallback=move || {
                                view! {
                                    <EmptyState
                                        icon=std::sync::Arc::new(|| view! {
                                            <Icon icon=phosphor_leptos::KEY weight=IconWeight::Duotone size="64px"/>
                                        }.into_any())
                                        title="No passkeys registered yet"
                                        description="Add a passkey for passwordless sign-in"
                                        action=std::sync::Arc::new(move || view! {
                                            <Show when=move || is_supported.get()>
                                                <Button on:click=move |_| add_modal_open.set(true)>
                                                    <span class="mr-2">
                                                        <Icon icon=phosphor_leptos::PLUS size="16px"/>
                                                    </span>
                                                    "Add Your First Passkey"
                                                </Button>
                                            </Show>
                                        }.into_any())
                                        class="border-2 border-dashed bg-muted"
                                    />
                                }
                            }
                        >
                            // Passkeys table
                            <div class="overflow-x-auto">
                                <table class="min-w-full divide-y divide-border">
                                    <thead class="bg-muted">
                                        <tr>
                                            <th class="px-6 py-3 text-left text-xs font-medium text-muted-foreground uppercase tracking-wider">
                                                "Device"
                                            </th>
                                            <th class="px-6 py-3 text-left text-xs font-medium text-muted-foreground uppercase tracking-wider">
                                                "Created"
                                            </th>
                                            <th class="px-6 py-3 text-left text-xs font-medium text-muted-foreground uppercase tracking-wider">
                                                "Last Used"
                                            </th>
                                            <th class="px-6 py-3 text-right text-xs font-medium text-muted-foreground uppercase tracking-wider">
                                                "Actions"
                                            </th>
                                        </tr>
                                    </thead>
                                    <tbody class="bg-background divide-y divide-border">
                                        <For
                                            each=move || passkeys.get()
                                            key=|pk| pk.credential_id.clone()
                                            let:passkey
                                        >
                                            {
                                                let passkey_count = Memo::new(move |_| passkeys.get().len());
                                                let pk_for_rename = passkey.clone();
                                                let pk_for_delete = passkey.clone();
                                                view! {
                                                    <PasskeyRow
                                                        passkey=passkey
                                                        passkey_count=passkey_count
                                                        on_rename=Callback::new(move |()| {
                                                            open_rename_modal(
                                                                pk_for_rename.credential_id.clone(),
                                                                pk_for_rename.name.clone(),
                                                            );
                                                        })
                                                        on_delete=Callback::new(move |()| {
                                                            open_delete_dialog(
                                                                pk_for_delete.credential_id.clone(),
                                                                pk_for_delete.name.clone(),
                                                            );
                                                        })
                                                    />
                                                }
                                            }
                                        </For>
                                    </tbody>
                                </table>
                            </div>
                        </Show>
                    </Show>
                </div>

                // Add Passkey button (when passkeys exist and browser supports it)
                <Show when=move || is_supported.get() && !passkeys.get().is_empty()>
                    <div class="flex justify-start">
                        <Button on:click=move |_| add_modal_open.set(true)>
                            <span class="mr-2">
                                <Icon icon=phosphor_leptos::PLUS size="16px"/>
                            </span>
                            "Add Passkey"
                        </Button>
                    </div>
                </Show>

                // Not supported warning
                <Show when=move || !is_supported.get()>
                    <Alert variant=AlertVariant::Warning>
                        <AlertDescription>
                            "Passkeys are not supported on this device or browser. Try using a modern browser like Chrome, Safari, or Edge."
                        </AlertDescription>
                    </Alert>
                </Show>

                // Tip when only one passkey
                <Show when=move || passkeys.get().len() == 1>
                    <div class="mt-6">
                        <Alert variant=AlertVariant::Info>
                            <AlertDescription>
                                <strong>"Tip:"</strong>
                                " Add a second passkey on another device to ensure you can always access your account."
                            </AlertDescription>
                        </Alert>
                    </div>
                </Show>
            </CardContent>
        </Card>

        // Delete Confirm Dialog
        <ConfirmDialog
            open=Signal::from(dialog_open)
            title=dialog_title.get_untracked()
            message=dialog_message.get_untracked()
            confirm_text="Delete"
            on_confirm=on_confirm_delete
            on_cancel=on_cancel_delete
        />

        // Add Passkey Modal
        <Modal
            show=Signal::from(add_modal_open)
            on_close=Callback::new(move |()| {
                add_modal_open.set(false);
                add_device_name.set(String::new());
            })
            title="Add Passkey"
            size=ModalSize::Md
        >
            <div class="space-y-4">
                <p class="text-muted-foreground">
                    "You'll be prompted to use your device's biometrics (fingerprint, face, or PIN) to create a new passkey."
                </p>
                <div>
                    <label class="block text-sm font-medium text-foreground mb-2">
                        "Device Name (optional)"
                    </label>
                    <input
                        type="text"
                        class=INPUT_CLASS
                        placeholder="e.g., My MacBook Pro"
                        maxlength="100"
                        prop:value=move || add_device_name.get()
                        on:input=move |ev| {
                            add_device_name.set(event_target_value(&ev));
                        }
                    />
                    <p class="text-xs text-muted-foreground mt-1">
                        "If left empty, we'll auto-detect your device name."
                    </p>
                </div>
                <div class="flex justify-end gap-3 pt-4">
                    <Button
                        variant=ButtonVariant::Outline
                        disabled=add_loading.get_untracked()
                        on:click=move |_| {
                            add_modal_open.set(false);
                            add_device_name.set(String::new());
                        }
                    >
                        "Cancel"
                    </Button>
                    <Button
                        disabled=add_loading.get_untracked()
                        on:click=handle_add_passkey
                    >
                        {move || if add_loading.get() { "Adding..." } else { "Add Passkey" }}
                    </Button>
                </div>
            </div>
        </Modal>

        // Rename Passkey Modal
        <Modal
            show=Signal::from(rename_modal_open)
            on_close=Callback::new(move |()| {
                rename_modal_open.set(false);
                rename_credential_id.set(None);
                rename_device_name.set(String::new());
            })
            title="Rename Passkey"
            size=ModalSize::Md
        >
            <div class="space-y-4">
                <div>
                    <label class="block text-sm font-medium text-foreground mb-2">
                        "Device Name"
                    </label>
                    <input
                        type="text"
                        class=INPUT_CLASS
                        placeholder="e.g., My iPhone"
                        maxlength="100"
                        prop:value=move || rename_device_name.get()
                        on:input=move |ev| {
                            rename_device_name.set(event_target_value(&ev));
                        }
                    />
                </div>
                <div class="flex justify-end gap-3 pt-4">
                    <Button
                        variant=ButtonVariant::Outline
                        disabled=rename_action.pending().get_untracked()
                        on:click=move |_| {
                            rename_modal_open.set(false);
                            rename_credential_id.set(None);
                            rename_device_name.set(String::new());
                        }
                    >
                        "Cancel"
                    </Button>
                    <Button
                        disabled=rename_action.pending().get_untracked()
                        on:click=handle_rename
                    >
                        {move || if rename_action.pending().get() { "Saving..." } else { "Save" }}
                    </Button>
                </div>
            </div>
        </Modal>
    }
}

/// Orchestrate the full add-passkey flow:
/// 1. Call server to start registration (get challenge + options)
/// 2. Call browser WebAuthn API to create credential
/// 3. Call server to complete registration
async fn add_passkey_flow(device_name: &str) -> Result<String, String> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = device_name;
        Err("Passkey registration requires a browser".to_string())
    }

    #[cfg(target_arch = "wasm32")]
    {
        // Step 1: Start registration on server
        let options_json = start_passkey_registration(device_name.to_string())
            .await
            .map_err(|e| e.to_string())?;

        // Step 2: Browser creates credential
        let credential_json = browser_create_credential(&options_json).await?;

        // Step 3: Complete registration on server
        complete_passkey_registration(credential_json)
            .await
            .map_err(|e| e.to_string())
    }
}

/// A single row in the passkeys table.
#[component]
fn PasskeyRow(
    passkey: PasskeyInfo,
    passkey_count: Memo<usize>,
    on_rename: Callback<()>,
    on_delete: Callback<()>,
) -> impl IntoView {
    let name = passkey.name.clone();
    let created = passkey
        .created_at
        .as_deref()
        .map(format_date)
        .unwrap_or_else(|| "Unknown".to_string());
    let last_used = passkey
        .last_used
        .as_deref()
        .map(format_relative_time)
        .unwrap_or_else(|| "Never".to_string());

    let icon_kind = detect_device_icon(&name);

    view! {
        <tr>
            // Device column
            <td class="px-6 py-4 whitespace-nowrap">
                <div class="flex items-center">
                    <div class="flex-shrink-0">
                        <span class="h-5 w-5 text-muted-foreground">
                            {match icon_kind {
                                DeviceIconKind::Smartphone => view! {
                                    <Icon icon=phosphor_leptos::DEVICE_MOBILE size="20px"/>
                                }.into_any(),
                                DeviceIconKind::Monitor => view! {
                                    <Icon icon=phosphor_leptos::MONITOR size="20px"/>
                                }.into_any(),
                                DeviceIconKind::Key => view! {
                                    <Icon icon=phosphor_leptos::KEY size="20px"/>
                                }.into_any(),
                            }}
                        </span>
                    </div>
                    <div class="ml-3">
                        <div class="text-sm font-medium text-foreground">
                            {name}
                        </div>
                    </div>
                </div>
            </td>

            // Created column
            <td class="px-6 py-4 whitespace-nowrap text-sm text-muted-foreground">
                {created}
            </td>

            // Last Used column
            <td class="px-6 py-4 whitespace-nowrap text-sm text-muted-foreground">
                {last_used}
            </td>

            // Actions column
            <td class="px-6 py-4 whitespace-nowrap text-right">
                <div class="flex justify-end gap-2">
                    // Rename button
                    <Button
                        variant=ButtonVariant::Ghost
                        size=ButtonSize::Icon
                        on:click=move |_| on_rename.run(())
                    >
                        <span title="Rename passkey">
                            <Icon icon=phosphor_leptos::PENCIL_SIMPLE size="16px"/>
                        </span>
                    </Button>

                    // Delete button (only if more than 1 passkey)
                    <Show when={move || passkey_count.get() >= 2}>
                        <button
                            class="inline-flex items-center justify-center gap-2 whitespace-nowrap rounded-md text-sm font-medium transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring h-9 w-9 text-error-foreground hover:text-error-foreground hover:bg-error"
                            on:click=move |_| on_delete.run(())
                            title="Delete passkey"
                        >
                            <Icon icon=phosphor_leptos::TRASH size="16px"/>
                        </button>
                    </Show>
                </div>
            </td>
        </tr>
    }
}
