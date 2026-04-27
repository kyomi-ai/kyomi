// SPDX-License-Identifier: AGPL-3.0-or-later

//! Home page — smart redirect based on user's landing page preference.

use leptos::prelude::*;
use leptos_router::hooks::use_navigate;
use leptos_router::NavigateOptions;

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

/// Compute the target URL for the sidebar "Dashboards" nav item.
///
/// Priority: personal default > workspace default > `/dashboards` list.
/// This does NOT consult `landing_page` — the nav link goes to the user's
/// dashboard regardless of whether dashboards is their configured landing
/// page. Landing-page priority lives in [`resolve_redirect_target`].
///
/// Matches the `Some(_)` semantics used by [`resolve_redirect_target`] on
/// the dashboard ID fields — both helpers treat *any* `Some` value as a
/// valid ID (including `Some("")`), so their behaviour stays consistent.
pub fn resolve_dashboards_nav_href(config: &LandingConfig) -> String {
    if let Some(ref id) = config.user_default_dashboard_id {
        format!("/dashboard/{id}")
    } else if let Some(ref id) = config.workspace_default_dashboard_id {
        format!("/dashboard/{id}")
    } else {
        "/dashboards".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_config(
        user_default: Option<&str>,
        workspace_default: Option<&str>,
    ) -> LandingConfig {
        LandingConfig {
            landing_page: String::new(),
            user_default_dashboard_id: user_default.map(String::from),
            workspace_default_dashboard_id: workspace_default.map(String::from),
            is_personal_mode: false,
            llm_configured: true,
        }
    }

    #[test]
    fn nav_href_personal_only() {
        let cfg = make_config(Some("user-abc"), None);
        assert_eq!(resolve_dashboards_nav_href(&cfg), "/dashboard/user-abc");
    }

    #[test]
    fn nav_href_workspace_only() {
        let cfg = make_config(None, Some("ws-abc"));
        assert_eq!(resolve_dashboards_nav_href(&cfg), "/dashboard/ws-abc");
    }

    #[test]
    fn nav_href_both_set_personal_wins() {
        let cfg = make_config(Some("user-abc"), Some("ws-def"));
        assert_eq!(resolve_dashboards_nav_href(&cfg), "/dashboard/user-abc");
    }

    #[test]
    fn nav_href_neither_falls_back_to_list() {
        let cfg = make_config(None, None);
        assert_eq!(resolve_dashboards_nav_href(&cfg), "/dashboards");
    }

    #[test]
    fn nav_href_empty_string_personal_id_is_treated_as_some() {
        // `resolve_redirect_target` uses plain `if let Some(ref id)` on the
        // dashboard ID fields, so `Some("")` is accepted as an ID and produces
        // a `/dashboard/` URL. This test pins that behaviour for the nav
        // helper so the two resolvers stay in sync. If the server ever starts
        // normalising empty IDs to `None`, both helpers should be updated
        // together (and this test updated with them).
        let cfg = make_config(Some(""), Some("ws-abc"));
        assert_eq!(resolve_dashboards_nav_href(&cfg), "/dashboard/");
    }
}

/// Home page component — fetches landing config and redirects.
///
/// Redirect hub — fetches landing config, navigates to the target route.
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

    view! {
        <Transition fallback=move || view! {
            <div class="flex items-center justify-center min-h-[60vh]">
                <img src="/kyomi_animated_logo.svg" alt="Processing" class="w-12 h-12" />
            </div>
        }>
            {move || Suspend::new(async move {
                match config_resource.await {
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
                        view! {
                            <div class="flex items-center justify-center min-h-[60vh]">
                                <img src="/kyomi_animated_logo.svg" alt="Processing" class="w-12 h-12" />
                            </div>
                        }.into_any()
                    }
                }
            })}
        </Transition>
    }
}
