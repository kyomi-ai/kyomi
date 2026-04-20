// SPDX-License-Identifier: AGPL-3.0-or-later

//! Home page — smart redirect based on user's landing page preference.
//!
//! This page acts as a routing hub: it fetches the user's configured
//! landing page preference and redirects to the appropriate route.
//! It never renders persistent content — only a loading spinner while
//! the server function resolves.
//!
//! Mirrors `apps/frontend/src/components/LandingRedirect.jsx`.

use leptos::prelude::*;
use leptos_router::hooks::use_navigate;
use leptos_router::NavigateOptions;

use crate::components::spinner::Spinner;
use crate::server_fns::home::{LandingConfig, get_landing_config};

/// Resolve the redirect target URL from the landing configuration.
///
/// Implements the same priority chain as `LandingRedirect.jsx`:
/// 1. `"chat"` -> `/chat`
/// 2. `"watches"` -> `/watches`
/// 3. `"sql_editor"` -> `/sql-editor`
/// 4. `"dashboards"` -> user default dashboard > workspace default > `/dashboards`
/// 5. Personal mode without LLM -> defaults to dashboards instead of chat
///
/// Public so callers outside this module (e.g. the sidebar logo link in
/// [`crate::components::layout`]) can compute the user's landing href
/// from a cached [`LandingConfig`] without duplicating the priority chain.
pub fn resolve_redirect_target(config: &LandingConfig) -> String {
    // Determine the effective landing page, applying the personal mode special case
    let default_page = if config.is_personal_mode && !config.llm_configured {
        "dashboards"
    } else {
        "chat"
    };

    let landing_page = if config.landing_page.is_empty() {
        default_page
    } else {
        config.landing_page.as_str()
    };

    match landing_page {
        "chat" => "/chat".to_string(),
        "watches" => "/watches".to_string(),
        "sql_editor" => "/sql-editor".to_string(),
        "dashboards" => {
            // Dashboard resolution chain: user default > workspace default > list
            if let Some(ref id) = config.user_default_dashboard_id {
                format!("/dashboard/{id}")
            } else if let Some(ref id) = config.workspace_default_dashboard_id {
                format!("/dashboard/{id}")
            } else {
                "/dashboards".to_string()
            }
        }
        // Unknown preference — fall back to default
        _ => {
            if default_page == "dashboards" {
                "/dashboards".to_string()
            } else {
                "/chat".to_string()
            }
        }
    }
}

/// Home page component — fetches landing config and redirects.
///
/// Shows a centered spinner during the server function call, then
/// navigates with `replace: true` so the home page doesn't appear
/// in browser history.
#[component]
pub fn HomePage() -> impl IntoView {
    let config_resource = Resource::new(|| (), |_| get_landing_config());
    let navigate = use_navigate();

    // Navigate once the config loads. Using an Effect ensures we don't
    // call navigate() during the render phase (which Leptos forbids).
    Effect::new(move || {
        if let Some(Ok(config)) = config_resource.get() {
            let target = resolve_redirect_target(&config);
            navigate(
                &target,
                NavigateOptions {
                    replace: true,
                    ..Default::default()
                },
            );
        }
    });

    // Show a centered spinner while loading.
    // On error, show a minimal message — this should rarely happen since
    // the auth middleware would have already redirected unauthenticated users.
    view! {
        <Suspense fallback=move || view! {
            <div class="flex items-center justify-center min-h-[60vh]">
                <Spinner class="h-8 w-8 text-muted-foreground" />
            </div>
        }>
            {move || {
                config_resource.get().map(|result| match result {
                    Err(e) => {
                        // Server error — show message rather than blank screen
                        let msg = format!("Failed to load landing configuration: {e}");
                        view! {
                            <div class="flex items-center justify-center min-h-[60vh]">
                                <p class="text-sm text-destructive">{msg}</p>
                            </div>
                        }.into_any()
                    }
                    Ok(_) => {
                        // Config loaded — Effect above handles navigation.
                        // Show spinner briefly while the navigation takes effect.
                        view! {
                            <div class="flex items-center justify-center min-h-[60vh]">
                                <Spinner class="h-8 w-8 text-muted-foreground" />
                            </div>
                        }.into_any()
                    }
                })
            }}
        </Suspense>
    }
}
