// SPDX-License-Identifier: AGPL-3.0-or-later

//! Datasource OAuth callback page — processes the OAuth redirect after provider consent.
//!
//! Route: `/auth/oauth/:provider/callback?code=xxx&state=xxx`
//!
//! External OAuth providers (Snowflake, Databricks, BigQuery Enterprise,
//! Microsoft Enterprise) redirect here after user consent for datasource
//! credential linking. Auto-processes the callback on mount. On success,
//! redirects to the datasources settings tab so the user sees the newly-
//! linked provider account.

use leptos::prelude::*;
#[cfg(target_arch = "wasm32")]
use leptos_router::hooks::use_navigate;
use leptos_router::hooks::use_params_map;
use phosphor_leptos::Icon;
use crate::components::{ButtonLink, ButtonSize, ButtonVariant};
use crate::pages::auth::auth_layout::AuthLayout;

// ─────────────────────────────────────────────────────────────────────────────
// Page state
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq)]
enum LinkStatus {
    Processing,
    Success,
    Error,
}

/// Outcome of processing the datasource OAuth callback.
struct LinkOutcome {
    status: LinkStatus,
    message: String,
    /// Where to redirect after a short delay, if applicable.
    redirect_url: Option<String>,
}

/// Capitalize the first character of a provider string for display.
///
/// E.g. "snowflake" → "Snowflake", "bigquery-enterprise" → "Bigquery-enterprise".
fn capitalize_provider(provider: &str) -> String {
    let mut chars = provider.chars();
    match chars.next() {
        None => String::new(),
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
    }
}

/// Process the datasource OAuth callback result and produce the next page state.
/// Not cfg-gated so the compiler sees all `LinkStatus` variants constructed.
async fn process_datasource_oauth_callback(
    provider: String,
    code: Option<String>,
    state_param: Option<String>,
    error: Option<String>,
) -> LinkOutcome {
    use crate::server_fns::auth::{datasource_oauth_callback, DatasourceOAuthCallbackResult};

    let provider_display = capitalize_provider(&provider);

    // Check for error param (providers return this on denial, e.g. access_denied)
    if let Some(err) = error {
        return LinkOutcome {
            status: LinkStatus::Error,
            message: format!("{provider_display} OAuth error: {err}"),
            redirect_url: None,
        };
    }

    // Validate required params — code is mandatory; state is validated server-side
    let Some(code) = code else {
        return LinkOutcome {
            status: LinkStatus::Error,
            message: "Missing authorization code parameter".to_string(),
            redirect_url: None,
        };
    };

    // Call the server function
    match datasource_oauth_callback(provider, code, state_param).await {
        Ok(DatasourceOAuthCallbackResult::Success { .. }) => LinkOutcome {
            status: LinkStatus::Success,
            message: format!("Your {provider_display} account has been linked successfully."),
            redirect_url: Some("/settings?tab=datasources".to_string()),
        },
        Ok(DatasourceOAuthCallbackResult::Error { message: msg }) => LinkOutcome {
            status: LinkStatus::Error,
            message: msg,
            redirect_url: None,
        },
        Err(e) => LinkOutcome {
            status: LinkStatus::Error,
            message: e
                .to_string()
                .strip_prefix("error running server function: ")
                .unwrap_or(&e.to_string())
                .to_string(),
            redirect_url: None,
        },
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Main component
// ─────────────────────────────────────────────────────────────────────────────

#[component]
pub fn DatasourceOAuthCallbackPage() -> impl IntoView {
    #[cfg(target_arch = "wasm32")]
    let navigate = use_navigate();

    // Extract the `:provider` path parameter from the route.
    let params = use_params_map();
    let provider = params.read_untracked().get("provider").unwrap_or_default();
    let provider_display = capitalize_provider(&provider);

    let (status, set_status) = signal(LinkStatus::Processing);
    let (message, set_message) = signal(format!("Linking {provider_display} Account"));

    // Help text appears after 10s if the redirect hasn't fired. The timer
    // and signal only exist on the wasm target — on SSR there's nothing to
    // wait for, so we expose a constant `false` Signal of the same shape.
    #[cfg(target_arch = "wasm32")]
    let show_help_text = {
        let (show, set) = signal(false);
        use gloo_timers::callback::Timeout;
        Timeout::new(10_000, move || { set.try_set(true); }).forget();
        Signal::derive(move || show.get())
    };
    #[cfg(not(target_arch = "wasm32"))]
    let show_help_text: Signal<bool> = Signal::derive(|| false);

    // Process the link callback on mount (browser-only: read URL params).
    // Uses the browser's native URLSearchParams — same pattern as the other
    // auth completion pages. The backend-side SSR path gets None defaults.
    #[cfg(target_arch = "wasm32")]
    let (code, state_param, error) = {
        let search_params = web_sys::window()
            .and_then(|w| w.location().search().ok())
            .and_then(|s| web_sys::UrlSearchParams::new_with_str(&s).ok());
        match search_params {
            Some(p) => (p.get("code"), p.get("state"), p.get("error")),
            None => (None, None, None),
        }
    };
    #[cfg(not(target_arch = "wasm32"))]
    let (code, state_param, error): (Option<String>, Option<String>, Option<String>) =
        (None, None, None);

    // Clone provider before it is consumed by process_datasource_oauth_callback,
    // so the popup postMessage block can use it independently.
    #[cfg(target_arch = "wasm32")]
    let provider_for_msg = provider.clone();

    // Cannot use Action: uses gloo_timers::future::TimeoutFuture (timer-based
    // delay before navigation) and use_navigate() — both !Send browser APIs on
    // wasm32. Signal writes inside the async block use try_set for
    // deferred-write safety.
    leptos::task::spawn_local(async move {
        let outcome =
            process_datasource_oauth_callback(provider, code, state_param, error).await;
        set_status.try_set(outcome.status.clone());
        if !outcome.message.is_empty() {
            set_message.try_set(outcome.message.clone());
        }

        // If opened as a popup, postMessage to the opener window then close.
        // Otherwise fall through to the normal redirect behavior.
        #[cfg(target_arch = "wasm32")]
        {
            use crate::utils::oauth_popup::{send_oauth_error_to_opener, send_oauth_success_to_opener};

            let opener = web_sys::window()
                .and_then(|w| w.opener().ok())
                .and_then(|o| {
                    use wasm_bindgen::JsCast;
                    o.dyn_into::<web_sys::Window>().ok()
                });

            if let Some(opener_win) = opener {
                let is_success = outcome.status == LinkStatus::Success;
                let msg_type = match (is_success, provider_for_msg.as_str()) {
                    (true, "snowflake") => Some("SNOWFLAKE_OAUTH_SUCCESS"),
                    (true, "databricks") => Some("DATABRICKS_OAUTH_SUCCESS"),
                    (true, "bigquery-enterprise") => Some("BIGQUERY_ENTERPRISE_OAUTH_SUCCESS"),
                    (true, "microsoft-enterprise") => Some("MICROSOFT_ENTERPRISE_OAUTH_SUCCESS"),
                    (false, "snowflake") => Some("SNOWFLAKE_OAUTH_ERROR"),
                    (false, "databricks") => Some("DATABRICKS_OAUTH_ERROR"),
                    (false, "bigquery-enterprise") => Some("BIGQUERY_ENTERPRISE_OAUTH_ERROR"),
                    (false, "microsoft-enterprise") => Some("MICROSOFT_ENTERPRISE_OAUTH_ERROR"),
                    // Every provider the backend supports is mapped above — reaching this
                    // arm means the route received an unrecognised provider slug, which
                    // indicates a bug. Log a warning so it shows up in the browser console.
                    _ => {
                        web_sys::console::warn_1(
                            &format!(
                                "[oauth_popup] unrecognised provider '{}' — no postMessage sent",
                                provider_for_msg
                            )
                            .into(),
                        );
                        None
                    }
                };

                if let Some(t) = msg_type {
                    if is_success {
                        // provider_email is not surfaced through this callback page —
                        // the parent window can refetch state after receiving the message.
                        send_oauth_success_to_opener(&opener_win, t, None);
                    } else {
                        send_oauth_error_to_opener(&opener_win, t, &outcome.message);
                    }
                }

                // Close the popup — parent window handles the next step
                if let Some(w) = web_sys::window() {
                    w.close().ok();
                }
                return;
            }
        }

        // Not a popup — use normal redirect behavior.
        // Redirect handling — gloo_timers and use_navigate are browser-only,
        // but reading redirect_url must happen on both targets to consume the field.
        if let Some(_redirect_url) = outcome.redirect_url {
            #[cfg(target_arch = "wasm32")]
            {
                let navigate_clone = navigate.clone();
                // SPA navigation — keeps WASM in memory, short delay for success feedback
                gloo_timers::future::TimeoutFuture::new(1500).await;
                navigate_clone(&_redirect_url, Default::default());
            }
        }
    });

    // ── Reactive title & subtitle ────────────────────────────────────────
    let provider_display_for_subtitle = provider_display.clone();
    let title = Signal::derive(move || match status.get() {
        LinkStatus::Processing => format!("Linking {provider_display} Account"),
        LinkStatus::Success => "Account Linked".to_string(),
        LinkStatus::Error => "Link Failed".to_string(),
    });
    let subtitle = Signal::derive(move || match status.get() {
        LinkStatus::Processing => {
            format!("Completing your {provider_display_for_subtitle} account link...")
        }
        LinkStatus::Success => message.get(),
        LinkStatus::Error => message.get(),
    });

    view! {
        <AuthLayout title=title subtitle=subtitle>
            <div class="text-center space-y-4">
                // Status icon
                <div class="flex justify-center">
                    {move || {
                        if status.get() == LinkStatus::Error {
                            view! {
                                <Icon
                                    icon=phosphor_leptos::X_CIRCLE
                                    size="48px"
                                    attr:class="text-error-foreground"
                                />
                            }
                                .into_any()
                        } else {
                            // Branded moment (auth page) — DESIGN.md Loading State Pattern
                            view! {
                                <img
                                    src="/kyomi_animated_logo.svg"
                                    alt="Processing"
                                    class="w-12 h-12"
                                />
                            }
                                .into_any()
                        }
                    }}
                </div>

                // Error state content
                {move || {
                    if status.get() == LinkStatus::Error {
                        Some(
                            view! {
                                <div class="space-y-3">
                                    <ButtonLink
                                        href="/settings?tab=datasources"
                                        variant=ButtonVariant::Outline
                                        size=ButtonSize::Lg
                                        class="w-full"
                                    >
                                        "Return to Settings"
                                    </ButtonLink>
                                </div>
                            },
                        )
                    } else {
                        None
                    }
                }}

                // Help text — shown after 10 seconds if not redirected
                {move || {
                    if show_help_text.get() && status.get() == LinkStatus::Processing {
                        Some(
                            view! {
                                <div class="mt-6 text-center text-sm text-muted-foreground">
                                    <p>
                                        "If this page doesn't automatically redirect, you can return to settings manually."
                                    </p>
                                </div>
                            },
                        )
                    } else {
                        None
                    }
                }}
            </div>
        </AuthLayout>
    }
}
