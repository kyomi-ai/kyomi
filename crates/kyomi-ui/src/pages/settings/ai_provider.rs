// SPDX-License-Identifier: AGPL-3.0-or-later

//! AI Provider settings card — localStorage-only configuration.
//!
//! Replaces `apps/frontend/src/components/settings/AIProviderSettings.jsx`.
//! Allows users to configure their AI provider (Anthropic, OpenAI, Gemini)
//! with API key, model override, and base URL. All data is stored in
//! localStorage under the key `kyomi_llm_config`.

use leptos::prelude::*;
use leptos_icons::Icon;

use crate::components::{
    Alert, AlertDescription, Button, ButtonVariant, Card, CardContent, CardDescription,
    CardHeader, CardTitle, Label, INPUT_CLASS,
};
use crate::components::toast::{toast_error, toast_success};
use crate::server_fns::ai_provider::test_ai_provider;

// ─────────────────────────────────────────────────────────────────────────────
// Provider definitions — mirrors the React `PROVIDERS` object
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(target_arch = "wasm32")]
const STORAGE_KEY: &str = "kyomi_llm_config";

#[derive(PartialEq)]
struct ProviderInfo {
    id: &'static str,
    label: &'static str,
    default_model: &'static str,
    default_base_url: &'static str,
}

const PROVIDERS: &[ProviderInfo] = &[
    ProviderInfo {
        id: "anthropic",
        label: "Anthropic",
        default_model: "claude-sonnet-4-20250514",
        default_base_url: "https://api.anthropic.com",
    },
    ProviderInfo {
        id: "openai",
        label: "OpenAI",
        default_model: "gpt-4o",
        default_base_url: "https://api.openai.com/v1",
    },
    ProviderInfo {
        id: "gemini",
        label: "Gemini",
        default_model: "gemini-2.5-pro",
        default_base_url: "https://generativelanguage.googleapis.com/v1beta",
    },
];

fn find_provider(id: &str) -> &'static ProviderInfo {
    PROVIDERS.iter().find(|p| p.id == id).unwrap_or(&PROVIDERS[0])
}

// ─────────────────────────────────────────────────────────────────────────────
// localStorage helpers (wasm32 only)
// ─────────────────────────────────────────────────────────────────────────────

/// Load the saved LLM config from localStorage. Returns (provider, api_key, model_override, base_url_override).
#[cfg(target_arch = "wasm32")]
fn load_config() -> (String, String, String, String) {
    use wasm_bindgen::JsValue;

    let storage = web_sys::window()
        .and_then(|w| w.local_storage().ok().flatten());

    let Some(storage) = storage else {
        return default_config();
    };

    let raw = match storage.get_item(STORAGE_KEY) {
        Ok(Some(val)) => val,
        _ => return default_config(),
    };

    // Parse JSON manually via js_sys
    let parsed = match js_sys::JSON::parse(&raw) {
        Ok(obj) => obj,
        Err(_) => return default_config(),
    };

    let get_str = |key: &str| -> String {
        let val = js_sys::Reflect::get(&parsed, &JsValue::from_str(key)).unwrap_or(JsValue::NULL);
        val.as_string().unwrap_or_default()
    };

    let provider = get_str("provider");
    let provider = if provider.is_empty() {
        "anthropic".to_string()
    } else {
        provider
    };

    (
        provider,
        get_str("api_key"),
        get_str("model_override"),
        get_str("base_url_override"),
    )
}

#[cfg(not(target_arch = "wasm32"))]
fn load_config() -> (String, String, String, String) {
    default_config()
}

fn default_config() -> (String, String, String, String) {
    ("anthropic".to_string(), String::new(), String::new(), String::new())
}

/// Save the LLM config to localStorage as JSON.
#[cfg(target_arch = "wasm32")]
fn save_config_to_storage(provider: &str, api_key: &str, model_override: &str, base_url_override: &str) -> bool {
    use wasm_bindgen::JsValue;

    let storage = web_sys::window()
        .and_then(|w| w.local_storage().ok().flatten());

    let Some(storage) = storage else {
        return false;
    };

    let obj = js_sys::Object::new();
    let _ = js_sys::Reflect::set(&obj, &JsValue::from_str("provider"), &JsValue::from_str(provider));
    let _ = js_sys::Reflect::set(&obj, &JsValue::from_str("api_key"), &JsValue::from_str(api_key));

    // Store model_override and base_url_override as null if empty (matches React behavior)
    if model_override.is_empty() {
        let _ = js_sys::Reflect::set(&obj, &JsValue::from_str("model_override"), &JsValue::NULL);
    } else {
        let _ = js_sys::Reflect::set(&obj, &JsValue::from_str("model_override"), &JsValue::from_str(model_override));
    }

    if base_url_override.is_empty() {
        let _ = js_sys::Reflect::set(&obj, &JsValue::from_str("base_url_override"), &JsValue::NULL);
    } else {
        let _ = js_sys::Reflect::set(&obj, &JsValue::from_str("base_url_override"), &JsValue::from_str(base_url_override));
    }

    let json = match js_sys::JSON::stringify(&obj) {
        Ok(s) => s,
        Err(_) => return false,
    };

    storage.set_item(STORAGE_KEY, &String::from(json)).is_ok()
}

#[cfg(not(target_arch = "wasm32"))]
fn save_config_to_storage(_provider: &str, _api_key: &str, _model_override: &str, _base_url_override: &str) -> bool {
    false
}

// ─────────────────────────────────────────────────────────────────────────────
// Component
// ─────────────────────────────────────────────────────────────────────────────

#[component]
pub fn AiProviderCard() -> impl IntoView {
    let (initial_provider, initial_key, initial_model, initial_url) = load_config();

    let (provider, set_provider) = signal(initial_provider);
    let (api_key, set_api_key) = signal(initial_key);
    let (model_override, set_model_override) = signal(initial_model);
    let (base_url_override, set_base_url_override) = signal(initial_url);
    let (show_api_key, set_show_api_key) = signal(false);

    let _provider_options: Vec<(&'static str, &'static str)> = PROVIDERS
        .iter()
        .map(|p| (p.id, p.label))
        .collect();

    // Derive the selected provider info for placeholders
    let selected_provider = Memo::new(move |_| find_provider(&provider.get()));

    let handle_save = move |_| {
        let key = api_key.get();
        if key.trim().is_empty() {
            toast_error("Please enter an API key.");
            return;
        }

        let saved = save_config_to_storage(
            &provider.get(),
            key.trim(),
            model_override.get().trim(),
            base_url_override.get().trim(),
        );

        if saved {
            toast_success("AI provider configuration saved.");
        } else {
            toast_error("Failed to save configuration.");
        }
    };

    let (testing, set_testing) = signal(false);

    let handle_test = move |_| {
        let key = api_key.get();
        if key.trim().is_empty() {
            toast_error("Please enter an API key first.");
            return;
        }

        let prov = provider.get();
        let model = model_override.get();
        let url = base_url_override.get();

        set_testing.set(true);
        leptos::task::spawn_local(async move {
            match test_ai_provider(prov, key, url, model).await {
                Ok(result) => {
                    if result.success {
                        toast_success(&result.message);
                    } else {
                        toast_error(&result.message);
                    }
                }
                Err(e) => {
                    toast_error(&format!("Connection test failed: {e}"));
                }
            }
            set_testing.set(false);
        });
    };

    view! {
        <Card>
            <CardHeader>
                <CardTitle>"AI Provider"</CardTitle>
                <CardDescription>"Configure which AI provider to use for chat and automated watches."</CardDescription>
            </CardHeader>
            <CardContent>
                <div class="space-y-4">
                    // Info alert
                    <Alert>
                        <AlertDescription>
                            "Optional — only needed for built-in chat and automated watches. MCP tools work without an API key."
                        </AlertDescription>
                    </Alert>

                    // Provider select — uses raw <select> with prop:value for reactivity
                    // (StyledSelect accepts a static String, not a reactive Signal)
                    <div class="space-y-2">
                        <Label>"Provider"</Label>
                        <select
                            class=crate::components::select::SELECT_CLASS
                            style=crate::components::select::CHEVRON_STYLE
                            prop:value=move || provider.get()
                            on:change=move |ev| set_provider.set(event_target_value(&ev))
                        >
                            {PROVIDERS.iter().map(|p| view! {
                                <option value=p.id>{p.label}</option>
                            }).collect_view()}
                        </select>
                    </div>

                    // API Key with show/hide toggle
                    <div class="space-y-2">
                        <Label>"API Key"</Label>
                        <div class="relative">
                            <input
                                type=move || if show_api_key.get() { "text" } else { "password" }
                                class=format!("{INPUT_CLASS} pr-10")
                                placeholder=move || format!("Enter your {} API key", selected_provider.get().label)
                                prop:value=move || api_key.get()
                                on:input=move |ev| set_api_key.set(event_target_value(&ev))
                            />
                            <button
                                type="button"
                                class="absolute right-2 top-1/2 -translate-y-1/2 p-1 text-muted-foreground hover:text-foreground transition-colors"
                                aria-label=move || if show_api_key.get() { "Hide API key" } else { "Show API key" }
                                on:click=move |_| set_show_api_key.update(|v| *v = !*v)
                            >
                                {move || {
                                    if show_api_key.get() {
                                        view! { <Icon icon=icondata_lu::LuEyeOff width="16" height="16"/> }.into_any()
                                    } else {
                                        view! { <Icon icon=icondata_lu::LuEye width="16" height="16"/> }.into_any()
                                    }
                                }}
                            </button>
                        </div>
                    </div>

                    // Model Override
                    <div class="space-y-2">
                        <Label>
                            "Model Override "
                            <span class="text-muted-foreground font-normal">"(optional)"</span>
                        </Label>
                        <input
                            type="text"
                            class=INPUT_CLASS
                            placeholder=move || selected_provider.get().default_model
                            prop:value=move || model_override.get()
                            on:input=move |ev| set_model_override.set(event_target_value(&ev))
                        />
                        <p class="text-xs text-muted-foreground">
                            "Leave blank to use the default model for the selected provider."
                        </p>
                    </div>

                    // Base URL Override
                    <div class="space-y-2">
                        <Label>
                            "Base URL Override "
                            <span class="text-muted-foreground font-normal">"(optional)"</span>
                        </Label>
                        <input
                            type="text"
                            class=INPUT_CLASS
                            placeholder=move || selected_provider.get().default_base_url
                            prop:value=move || base_url_override.get()
                            on:input=move |ev| set_base_url_override.set(event_target_value(&ev))
                        />
                        <p class="text-xs text-muted-foreground">
                            "Override the API base URL for proxies or custom endpoints."
                        </p>
                    </div>

                    // Action buttons
                    <div class="flex items-center gap-3 pt-2">
                        <Button on:click=handle_save>
                            "Save"
                        </Button>
                        <Button variant=ButtonVariant::Outline on:click=handle_test disabled=testing>
                            <Icon icon=icondata_lu::LuZap width="16" height="16"/>
                            {move || if testing.get() { "Testing..." } else { "Test Connection" }}
                        </Button>
                    </div>
                </div>
            </CardContent>
        </Card>
    }
}
