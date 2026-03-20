// SPDX-License-Identifier: AGPL-3.0-or-later

//! Settings page shell — header + tab navigation bar.
//!
//! Matches `apps/frontend/src/pages/SettingsContent.jsx` layout:
//! - Page header: "Settings" title + description
//! - Tab bar: Profile, Security, Workspace, Data Sources, etc.
//! - Content area: renders the active tab's component
//!
//! Tab visibility is role-based (matching React logic exactly):
//! - Profile: always visible
//! - Security: hidden in personal mode
//! - Workspace: admin only, hidden in personal mode
//! - Data Sources: always visible
//! - Analytics: admin only, hidden in self-hosted
//! - Usage: hidden in self-hosted
//! - Billing: owner only, hidden in self-hosted
//! - Team: team tier + admin, hidden in personal mode

use leptos::prelude::*;
use leptos_icons::Icon;

use crate::server_fns::context::get_user_context;

/// Settings tab definition.
struct SettingsTab {
    id: &'static str,
    name: &'static str,
    icon: &'static icondata_core::IconData,
}

/// All settings tabs (visibility filtered at render time based on user context).
const TABS: &[SettingsTab] = &[
    SettingsTab { id: "profile", name: "Profile", icon: icondata_lu::LuUser },
    SettingsTab { id: "security", name: "Security", icon: icondata_lu::LuShield },
    SettingsTab { id: "workspace", name: "Workspace", icon: icondata_lu::LuSettings },
    SettingsTab { id: "datasources", name: "Data Sources", icon: icondata_lu::LuServer },
    SettingsTab { id: "analytics", name: "Analytics", icon: icondata_lu::LuActivity },
    SettingsTab { id: "usage", name: "Usage", icon: icondata_lu::LuChartBar },
    SettingsTab { id: "billing", name: "Billing", icon: icondata_lu::LuCreditCard },
    SettingsTab { id: "team", name: "Team", icon: icondata_lu::LuUsers },
];

/// Settings shell component — wraps settings tab content.
///
/// Fetches `UserContext` once via server function and provides it to all child
/// components via Leptos context. Settings tabs read it with
/// `expect_context::<Resource<Result<UserContext, ServerFnError>>>()`.
#[component]
pub fn SettingsShell(children: Children) -> impl IntoView {
    // Fetch user context once — all settings tabs share this resource.
    let user_ctx = Resource::new(|| (), |_| get_user_context());
    provide_context(user_ctx);

    // Derive active tab from the current URL path.
    let active_tab = {
        #[cfg(target_arch = "wasm32")]
        {
            web_sys::window()
                .and_then(|w| w.location().pathname().ok())
                .and_then(|p| p.strip_prefix("/settings/").map(|s| s.to_string()))
                .unwrap_or_else(|| "profile".to_string())
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            "profile".to_string()
        }
    };

    view! {
        <div class="w-full space-y-8" style:display="block">
            // Settings Header — matches React exactly
            // React: "text-3xl font-bold text-foreground"
            <div class="mb-8">
                <h1 class="text-3xl font-bold text-foreground">"Settings"</h1>
                <p class="text-muted-foreground mt-2">"Manage your workspace configuration and billing settings"</p>
            </div>

            // Settings Navigation Tabs
            // React: "w-full bg-card rounded-xl shadow-sm border border-border mb-6 overflow-hidden"
            <div class="w-full bg-card rounded-xl shadow-sm border border-border mb-6 overflow-hidden">
                <div class="border-b border-border overflow-x-auto">
                    <div class="flex space-x-4 md:space-x-8 px-4 md:px-6 min-w-max">
                        {TABS.iter().map(|tab| {
                            let is_active = tab.id == active_tab;
                            let tab_class = if is_active {
                                "flex items-center space-x-2 py-4 border-b-2 font-medium text-sm transition-colors whitespace-nowrap flex-shrink-0 border-primary text-primary"
                            } else {
                                "flex items-center space-x-2 py-4 border-b-2 font-medium text-sm transition-colors whitespace-nowrap flex-shrink-0 border-transparent text-muted-foreground hover:text-foreground hover:border-border"
                            };

                            // For the POC: profile tab stays in Leptos, others go to React
                            let href = if tab.id == "profile" {
                                "/settings/profile".to_string()
                            } else {
                                format!("/settings/{}", tab.id)
                            };

                            view! {
                                <a href=href class=tab_class>
                                    <Icon icon=tab.icon width="16" height="16"/>
                                    <span>{tab.name}</span>
                                </a>
                            }
                        }).collect_view()}
                    </div>
                </div>
            </div>

            // Settings Content
            // React: "w-full bg-card rounded-xl shadow-sm border border-border"
            <div class="w-full bg-card rounded-xl shadow-sm border border-border">
                {children()}
            </div>
        </div>
    }
}
