// SPDX-License-Identifier: AGPL-3.0-or-later

//! Profile settings page — the first Leptos-rendered page in Kyomi.
//!
//! Replaces `apps/frontend/src/components/settings/ProfileSettings.jsx`.
//! All data fetching uses server functions instead of REST API calls.

use leptos::prelude::*;

use crate::components::{
    ActionStatus, Card, CardContent, CardDescription, CardHeader, CardTitle, Label, StyledSelect,
    INPUT_CLASS,
};
use crate::server_fns::profile::*;
use crate::types::{DashboardSummary, ProfileData};

/// Refresh the auth session by calling the REST refresh endpoint.
/// This runs as a JS fetch (non-Send) so it can't go inside a Resource.
/// Call from an Effect or event handler, then trigger resource refetch.
#[cfg(target_arch = "wasm32")]
fn refresh_session_and_retry(profile: Resource<Result<ProfileData, ServerFnError>>) {
    use wasm_bindgen::prelude::*;

    // Use spawn_local since JS futures aren't Send
    leptos::task::spawn_local(async move {
        let promise = js_sys::Function::new_no_args(
            "return fetch('/api/v1/auth/refresh', { method: 'POST', credentials: 'include' }).then(r => r.ok)"
        ).call0(&JsValue::NULL);

        let refreshed = match promise {
            Ok(val) => {
                if let Ok(promise) = val.dyn_into::<js_sys::Promise>() {
                    wasm_bindgen_futures::JsFuture::from(promise)
                        .await
                        .ok()
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false)
                } else {
                    false
                }
            }
            Err(_) => false,
        };

        if refreshed {
            profile.refetch();
        }
    });
}

// ─────────────────────────────────────────────────────────────────────────────
// Palette data — matches apps/frontend/src/config/chartPalettes.js
// ─────────────────────────────────────────────────────────────────────────────

struct PaletteInfo {
    id: &'static str,
    name: &'static str,
    colors: &'static [&'static str],
}

const PALETTES: &[PaletteInfo] = &[
    PaletteInfo {
        id: "balanced",
        name: "Balanced",
        colors: &[
            "#1A75C9", "#B8405A", "#3D8A5A", "#D9952D", "#2D7A8A", "#C9734D",
            "#4D5A8A", "#99C94D", "#8A5A7A", "#D9B370", "#70B8D9", "#6B8A4D",
        ],
    },
    PaletteInfo {
        id: "vibrant",
        name: "Vibrant",
        colors: &[
            "#1E88C7", "#D92849", "#28C75A", "#E8B733", "#28C7A8", "#E87333",
            "#3355D9", "#A8D928", "#C728A8", "#D97328", "#28A8D9", "#73A828",
        ],
    },
    PaletteInfo {
        id: "accessible",
        name: "Accessible",
        colors: &[
            "#2D5F7A", "#A83D52", "#3D7A52", "#C9A642", "#3D8A8A", "#E89970",
            "#5C6D99", "#B8D96B", "#996B8A", "#B87752", "#85B8D9", "#85996B",
        ],
    },
];

// ─────────────────────────────────────────────────────────────────────────────
// Main page
// ─────────────────────────────────────────────────────────────────────────────

#[component]
pub fn ProfilePage() -> impl IntoView {
    let profile = Resource::new(|| (), |_| get_profile());

    // If the profile load fails with an auth error, try refreshing the session
    // and refetch. This handles the case where cookies have expired.
    #[cfg(target_arch = "wasm32")]
    {
        let profile_for_effect = profile;
        Effect::new(move || {
            if let Some(Err(e)) = profile_for_effect.get() {
                let msg = e.to_string();
                if msg.contains("Authentication required") || msg.contains("Unauthorized") {
                    refresh_session_and_retry(profile_for_effect);
                }
            }
        });
    }
    let dashboards = Resource::new(|| (), |_| get_dashboards());
    let invitations = Resource::new(|| (), |_| get_pending_invitations());

    view! {
        <div class="p-6">
            <h2 class="text-xl font-semibold text-foreground mb-6">"Profile Settings"</h2>

            <Suspense fallback=move || view! {
                <div class="flex items-center justify-center py-12">
                    <p class="text-muted-foreground">"Loading settings..."</p>
                </div>
            }>
                {move || {
                    let profile_result = profile.get();
                    let dashboards_result = dashboards.get();
                    let invitations_result = invitations.get();

                    profile_result.map(|result| match result {
                        Ok(data) => {
                            let dash_list = dashboards_result
                                .and_then(|r| r.ok())
                                .unwrap_or_default();
                            let inv_list = invitations_result
                                .and_then(|r| r.ok())
                                .unwrap_or_default();

                            let is_personal = data.is_personal_mode;
                            let has_invitations = !inv_list.is_empty();
                            let data_profile = data.clone();
                            let data_appearance = data.clone();
                            let data_prefs = data.clone();
                            let data_palette = data.clone();
                            let data_retention = data;

                            view! {
                                <div class="space-y-6">
                                    <Show when=move || !is_personal>
                                        <ProfileInfoCard data=data_profile.clone()/>
                                    </Show>
                                    <AppearanceCard data=data_appearance/>
                                    <PreferencesCard data=data_prefs dashboards=dash_list/>
                                    <ChartPaletteCard data=data_palette/>
                                    <QueryRetentionCard data=data_retention/>
                                    <Show when=move || !is_personal && has_invitations>
                                        <InvitationsCard invitations=inv_list.clone()/>
                                    </Show>
                                </div>
                            }.into_any()
                        },
                        Err(e) => {
                            let msg = e.to_string();
                            view! {
                                <Card>
                                    <div class="p-6">
                                        <p class="text-error-foreground">"Failed to load profile: " {msg}</p>
                                    </div>
                                </Card>
                            }.into_any()
                        },
                    })
                }}
            </Suspense>
        </div>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Profile Info Card
// ─────────────────────────────────────────────────────────────────────────────

#[component]
fn ProfileInfoCard(data: ProfileData) -> impl IntoView {
    let (name, set_name) = signal(data.name.clone().unwrap_or_default());
    let save_action = Action::new(|name: &String| {
        let name = name.clone();
        async move { update_profile_name(name).await }
    });

    let on_blur = move |_| {
        let current = name.get();
        if !current.trim().is_empty() {
            save_action.dispatch(current);
        }
    };

    view! {
        <Card>
            <CardHeader>
                <div class="flex items-center justify-between">
                    <div>
                        <CardTitle>"Profile Information"</CardTitle>
                        <CardDescription>"Your name and email address."</CardDescription>
                    </div>
                    <ActionStatus action=save_action/>
                </div>
            </CardHeader>
            <CardContent>
                <div class="space-y-4 max-w-md">
                    <div class="space-y-2">
                        <Label>"Name"</Label>
                        <input
                            type="text"
                            class=INPUT_CLASS
                            placeholder="Your name"
                            prop:value=name
                            on:input=move |ev| set_name.set(event_target_value(&ev))
                            on:blur=on_blur
                        />
                    </div>
                    <div class="space-y-2">
                        <Label>"Email"</Label>
                        <input
                            type="email"
                            class=format!("{INPUT_CLASS} bg-muted text-muted-foreground")
                            disabled=true
                            prop:value=data.email.clone()
                        />
                        <p class="text-xs text-muted-foreground">"Email cannot be changed"</p>
                    </div>
                </div>
            </CardContent>
        </Card>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Appearance Card
// ─────────────────────────────────────────────────────────────────────────────

#[component]
fn AppearanceCard(data: ProfileData) -> impl IntoView {
    // Get the theme state from context (provided by ThemeProvider at app root)
    let theme_state = crate::components::theme::use_theme();

    // Initialize with the user's saved preference
    if let Some(state) = theme_state {
        state.preference.set(data.theme.clone());
    }

    let (current_theme, set_current_theme) = signal(data.theme.clone());
    let save_action = Action::new(|theme: &String| {
        let theme = theme.clone();
        async move { update_theme(theme).await }
    });

    let theme_options = [("light", "Light"), ("dark", "Dark"), ("system", "System")];

    view! {
        <Card>
            <CardHeader>
                <CardTitle>"Appearance"</CardTitle>
                <CardDescription>"Choose how Kyomi looks to you."</CardDescription>
            </CardHeader>
            <CardContent>
                <div class="flex flex-wrap gap-3">
                    {theme_options.into_iter().map(|(value, label)| {
                        let value_str = value.to_string();
                        let value_for_click = value.to_string();
                        view! {
                            <button
                                class=move || {
                                    let base = "flex items-center gap-2 px-4 py-2 rounded-lg border-2 text-sm font-medium transition-all";
                                    if current_theme.get() == value_str {
                                        format!("{base} border-primary bg-primary/10 text-primary")
                                    } else {
                                        format!("{base} border-border text-muted-foreground hover:border-border/80 hover:text-foreground")
                                    }
                                }
                                on:click={
                                    let set_local = set_current_theme;
                                    let action = save_action;
                                    move |_| {
                                        // Update local UI state
                                        set_local.set(value_for_click.clone());
                                        // Apply theme to DOM immediately
                                        crate::components::theme::set_theme(&value_for_click);
                                        // Persist to server
                                        action.dispatch(value_for_click.clone());
                                    }
                                }
                            >
                                <span>{label}</span>
                            </button>
                        }
                    }).collect_view()}
                </div>
            </CardContent>
        </Card>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Preferences Card (Landing Page + Default Dashboard)
// ─────────────────────────────────────────────────────────────────────────────

#[component]
fn PreferencesCard(data: ProfileData, dashboards: Vec<DashboardSummary>) -> impl IntoView {
    let (landing, _set_landing) = signal(data.landing_page.clone());
    let (default_dash, _set_default_dash) = signal(data.default_dashboard_id.clone().unwrap_or_default());

    let landing_action = Action::new(|page: &String| {
        let page = page.clone();
        async move { update_landing_page(page).await }
    });
    let dashboard_action = Action::new(|id: &String| {
        let id = id.clone();
        async move {
            let opt = if id.is_empty() { None } else { Some(id) };
            update_default_dashboard(opt).await
        }
    });

    let landing_options = vec![
        ("chat", "Chat"),
        ("dashboards", "Dashboards"),
        ("watches", "Watches"),
        ("sql_editor", "SQL Editor"),
    ];

    let mut dashboard_options: Vec<(&str, &str)> = vec![("", "None")];
    // We need to leak these strings since the component owns them
    let dash_leaked: Vec<(String, String)> = dashboards.iter()
        .map(|d| (d.dashboard_id.clone(), d.title.clone()))
        .collect();

    view! {
        <Card>
            <CardHeader>
                <div class="flex items-center justify-between">
                    <div>
                        <CardTitle>"Preferences"</CardTitle>
                        <CardDescription>"Customize your Kyomi experience."</CardDescription>
                    </div>
                    <ActionStatus action=landing_action/>
                </div>
            </CardHeader>
            <CardContent>
                <div class="space-y-6 max-w-md">
                    // Landing Page
                    <div class="space-y-2">
                        <Label>"Landing Page"</Label>
                        <StyledSelect
                            value=landing.get_untracked()
                            options=landing_options
                            on_change=move |val| {
                                landing_action.dispatch(val);
                            }
                        />
                        <p class="text-xs text-muted-foreground">"Choose which page opens when you launch Kyomi."</p>
                    </div>

                    // Default Dashboard
                    <div class="space-y-2">
                        <Label>"My Default Dashboard"</Label>
                        <select
                            class=crate::components::select::SELECT_CLASS
                            style=crate::components::select::CHEVRON_STYLE
                            on:change=move |ev| {
                                dashboard_action.dispatch(event_target_value(&ev));
                            }
                        >
                            <option value="" selected=default_dash.get_untracked().is_empty()>"None"</option>
                            {dash_leaked.iter().map(|(id, title)| {
                                let selected = default_dash.get_untracked() == *id;
                                view! {
                                    <option value=id.clone() selected=selected>
                                        {title.clone()}
                                    </option>
                                }
                            }).collect_view()}
                        </select>
                        <p class="text-xs text-muted-foreground">"Opens this dashboard when landing page is set to Dashboards."</p>
                    </div>
                </div>
            </CardContent>
        </Card>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Chart Palette Card — color swatches matching React UI
// ─────────────────────────────────────────────────────────────────────────────

#[component]
fn ChartPaletteCard(data: ProfileData) -> impl IntoView {
    let (palette, set_palette) = signal(data.chart_palette.clone());
    let save_action = Action::new(|palette: &String| {
        let palette = palette.clone();
        async move { update_chart_palette(palette).await }
    });

    view! {
        <Card>
            <CardHeader>
                <div class="flex items-center justify-between">
                    <div>
                        <CardTitle>"Default Chart Palette"</CardTitle>
                        <CardDescription>"Choose the default color palette for your charts. This overrides workspace defaults."</CardDescription>
                    </div>
                    <ActionStatus action=save_action/>
                </div>
            </CardHeader>
            <CardContent>
                <div class="space-y-3">
                    {PALETTES.iter().map(|p| {
                        let id = p.id.to_string();
                        let id_for_click = p.id.to_string();
                        let name = p.name;
                        let colors = p.colors;
                        view! {
                            <button
                                class=move || {
                                    let base = "w-full text-left p-4 rounded-lg border-2 transition-all";
                                    if palette.get() == id {
                                        format!("{base} border-primary bg-primary/10")
                                    } else {
                                        format!("{base} border-border hover:border-border/80")
                                    }
                                }
                                on:click={
                                    let set_pal = set_palette;
                                    let action = save_action;
                                    move |_| {
                                        set_pal.set(id_for_click.clone());
                                        action.dispatch(id_for_click.clone());
                                    }
                                }
                            >
                                <div class="flex items-start justify-between mb-3">
                                    <div class="font-medium text-foreground">{name}</div>
                                    {move || {
                                        if palette.get() == p.id {
                                            view! {
                                                <svg xmlns="http://www.w3.org/2000/svg" width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" class="text-primary">
                                                    <path d="M22 11.08V12a10 10 0 1 1-5.93-9.14"/>
                                                    <polyline points="22 4 12 14.01 9 11.01"/>
                                                </svg>
                                            }.into_any()
                                        } else {
                                            view! { <span></span> }.into_any()
                                        }
                                    }}
                                </div>
                                <div class="flex flex-wrap gap-1">
                                    {colors.iter().map(|color| {
                                        view! {
                                            <div
                                                class="w-8 h-8 rounded border border-border"
                                                style=format!("background-color: {color}")
                                            />
                                        }
                                    }).collect_view()}
                                </div>
                            </button>
                        }
                    }).collect_view()}
                </div>
            </CardContent>
        </Card>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Query Retention Card
// ─────────────────────────────────────────────────────────────────────────────

#[component]
fn QueryRetentionCard(data: ProfileData) -> impl IntoView {
    let save_action = Action::new(|days: &i32| {
        let days = *days;
        async move { update_query_retention(days).await }
    });

    let options = vec![
        ("7", "7 days"),
        ("14", "14 days"),
        ("30", "30 days"),
        ("90", "90 days"),
        ("365", "1 year"),
    ];

    let initial_value = data.query_history_retention_days.to_string();

    view! {
        <Card>
            <CardHeader>
                <div class="flex items-center justify-between">
                    <div>
                        <CardTitle>"SQL Query History"</CardTitle>
                        <CardDescription>"Starred queries are never deleted. Unstarred queries are removed after the selected period."</CardDescription>
                    </div>
                    <ActionStatus action=save_action/>
                </div>
            </CardHeader>
            <CardContent>
                <div class="max-w-md space-y-2">
                    <Label>"Retention Period"</Label>
                    <StyledSelect
                        value=initial_value
                        options=options
                        on_change=move |val| {
                            if let Ok(days) = val.parse::<i32>() {
                                save_action.dispatch(days);
                            }
                        }
                    />
                </div>
            </CardContent>
        </Card>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Invitations Card
// ─────────────────────────────────────────────────────────────────────────────

#[component]
fn InvitationsCard(invitations: Vec<crate::types::InvitationData>) -> impl IntoView {
    let (inv_list, set_inv_list) = signal(invitations);

    let accept_action = Action::new(|id: &String| {
        let id = id.clone();
        async move { accept_invitation(id).await }
    });
    let decline_action = Action::new(|id: &String| {
        let id = id.clone();
        async move { decline_invitation(id).await }
    });

    view! {
        <Card>
            <CardHeader>
                <CardTitle>"Pending Invitations"</CardTitle>
                <CardDescription>"Workspace invitations waiting for your response."</CardDescription>
            </CardHeader>
            <CardContent>
                <div class="space-y-3">
                    <For
                        each=move || inv_list.get()
                        key=|inv| inv.invitation_id.clone()
                        let:inv
                    >
                        {
                            let inv_id_accept = inv.invitation_id.clone();
                            let inv_id_decline = inv.invitation_id.clone();
                            let inv_id_remove = inv.invitation_id.clone();
                            view! {
                                <div class="flex items-center justify-between p-4 border border-border rounded-lg">
                                    <div>
                                        <p class="text-sm font-medium text-foreground">
                                            "Workspace: " {inv.workspace_id.clone()}
                                        </p>
                                        <p class="text-xs text-muted-foreground">
                                            "Role: " {inv.role.clone()}
                                        </p>
                                    </div>
                                    <div class="flex gap-2">
                                        <button
                                            class="px-3 py-1.5 rounded-md text-xs font-medium bg-primary text-primary-foreground hover:bg-primary/90 transition-colors"
                                            on:click={
                                                let set_list = set_inv_list;
                                                let id = inv_id_accept.clone();
                                                let remove_id = inv_id_remove.clone();
                                                move |_| {
                                                    accept_action.dispatch(id.clone());
                                                    set_list.update(|list| list.retain(|i| i.invitation_id != remove_id));
                                                }
                                            }
                                        >
                                            "Accept"
                                        </button>
                                        <button
                                            class="px-3 py-1.5 rounded-md text-xs font-medium border border-border text-muted-foreground hover:text-foreground transition-colors"
                                            on:click={
                                                let set_list = set_inv_list;
                                                let id = inv_id_decline.clone();
                                                let remove_id = inv_id_remove;
                                                move |_| {
                                                    decline_action.dispatch(id.clone());
                                                    set_list.update(|list| list.retain(|i| i.invitation_id != remove_id));
                                                }
                                            }
                                        >
                                            "Decline"
                                        </button>
                                    </div>
                                </div>
                            }
                        }
                    </For>
                </div>
            </CardContent>
        </Card>
    }
}

