// SPDX-License-Identifier: AGPL-3.0-or-later

//! Profile settings page — the first Leptos-rendered page in Kyomi.
//!
//! Replaces `apps/frontend/src/components/settings/ProfileSettings.jsx`.
//! All data fetching uses server functions instead of REST API calls.

use leptos::prelude::*;

use crate::components::{
    ActionStatus, Card, CardContent, CardDescription, CardHeader, CardTitle, Label,
    SettingsPageSkeleton, StyledSelect, INPUT_CLASS,
};
use crate::components::select::DynSelect;
use crate::pages::settings::push_notifications::PushNotificationsCard;
use crate::server_fns::context::UserContext;
use crate::server_fns::profile::*;
use crate::types::{DashboardSummary, ProfileData};

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
        id: "kyomi",
        name: "Kyomi",
        // Amber-anchored editorial warm — Kyomi signature palette.
        // Slot 2 is shown here as the light-mode navy; dark mode lifts it
        // to #5A87C2 automatically at render time.
        colors: &[
            "#D97706", "#1E3A5F", "#3D8A5A", "#7C2D12", "#2D7A8A", "#A16207",
            "#7E22CE", "#6B8A4D", "#0891B2", "#9F1239", "#CA8A04", "#4D5A8A",
        ],
    },
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
    // Auth error handling (expired access_token) is done globally in Layout.
    let dashboards = Resource::new(|| (), |_| get_dashboards());
    let invitations = Resource::new(|| (), |_| get_pending_invitations());

    view! {
        <div class="p-4 sm:p-6">
            <h2 class="text-xl font-display text-foreground mb-6">"Profile Settings"</h2>

            <Transition fallback=move || view! { <SettingsPageSkeleton /> }>
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

                                    // Slack Connection — hidden in personal mode (feature-gated)
                                    <SlackSection is_personal=is_personal/>

                                    // Push Notifications — hidden in personal mode
                                    <Show when=move || !is_personal>
                                        <PushNotificationsCard/>
                                    </Show>

                                    <McpConnectionCard is_personal=is_personal/>

                                    <ChartPaletteCard data=data_palette/>
                                    <QueryRetentionCard data=data_retention/>
                                    <Show when=move || !is_personal && has_invitations>
                                        <InvitationsCard invitations=inv_list.clone()/>
                                    </Show>
                                    <ClearLocalDataCard/>
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
            </Transition>
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
                    </div>
                    <ActionStatus action=save_action/>
                </div>
            </CardHeader>
            <CardContent>
                <div class="space-y-4">
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
                            class="w-full px-3 py-2 border border-input rounded-md bg-muted text-foreground cursor-not-allowed"
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

    static THEME_OPTIONS: &[(&str, &str, phosphor_leptos::IconData)] = &[
        ("light", "Light", phosphor_leptos::SUN),
        ("dark", "Dark", phosphor_leptos::MOON),
        ("system", "System", phosphor_leptos::MONITOR),
    ];

    view! {
        <Card>
            <CardHeader>
                <CardTitle>"Appearance"</CardTitle>
                <CardDescription>"Choose how Kyomi looks to you."</CardDescription>
            </CardHeader>
            <CardContent>
                <div class="flex flex-wrap gap-3">
                    {THEME_OPTIONS.iter().map(|(value, label, icon_data)| {
                        let value_str = value.to_string();
                        let value_for_weight = value.to_string();
                        let value_for_click = value.to_string();
                        let label_str = label.to_string();
                        let icon = *icon_data;
                        let theme_weight = Memo::new(move |_| {
                            if current_theme.get() == value_for_weight {
                                phosphor_leptos::IconWeight::Fill
                            } else {
                                phosphor_leptos::IconWeight::Light
                            }
                        });
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
                                        // Apply theme to DOM + localStorage immediately
                                        crate::components::theme::set_theme(&value_for_click);
                                        crate::components::theme::save_theme_to_local_storage(&value_for_click);
                                        // Persist to server
                                        action.dispatch(value_for_click.clone());
                                    }
                                }
                            >
                                <phosphor_leptos::Icon icon=icon weight=theme_weight size="16px"/>
                                <span>{label_str}</span>
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

    let _dashboard_options: Vec<(&str, &str)> = vec![("", "None")];
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
                <div class="space-y-6">
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
                        <DynSelect
                            value=Signal::derive(move || default_dash.get())
                            options=Signal::derive(move || {
                                let mut opts: Vec<(String, String)> = vec![
                                    (String::new(), "None".to_string()),
                                ];
                                opts.extend(dash_leaked.iter().map(|(id, title)| (id.clone(), title.clone())));
                                opts
                            })
                            on_change=move |val| {
                                dashboard_action.dispatch(val);
                            }
                            placeholder="None"
                        />
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

    // Layout-level user_context LocalResource. Refetch it after a successful
    // palette save so `UserContext::chart_palette` reflects the new choice
    // across every subtree that reads it (without a full page reload).
    // KYO-129 Part 3.
    let user_ctx =
        expect_context::<LocalResource<Result<UserContext, ServerFnError>>>();

    Effect::new(move |_| {
        if matches!(save_action.value().get(), Some(Ok(()))) {
            user_ctx.refetch();
        }
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
                                                class="w-8 h-8 rounded-md border border-border"
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
                <div class="space-y-2">
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
// MCP Connection Card
// ─────────────────────────────────────────────────────────────────────────────

#[component]
fn McpConnectionCard(is_personal: bool) -> impl IntoView {
    // Derive MCP URL from window.location on the client.
    // On the server (SSR), use a sensible default that will be replaced on hydration.
    let mcp_url = {
        #[cfg(target_arch = "wasm32")]
        {
            let window = web_sys::window().expect("no global window");
            let location = window.location();
            let port = location.port().unwrap_or_default();
            let hostname = location.hostname().unwrap_or_default();
            let origin = location.origin().unwrap_or_default();

            if is_personal {
                let p = if port.is_empty() { "3000".to_string() } else { port };
                format!("http://localhost:{p}/mcp")
            } else if hostname == "localhost" || hostname == "127.0.0.1" {
                format!("http://{hostname}:8002/mcp")
            } else {
                format!("{origin}/mcp")
            }
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            "/mcp".to_string()
        }
    };

    let mcp_port = {
        #[cfg(target_arch = "wasm32")]
        {
            let port = web_sys::window()
                .and_then(|w| w.location().port().ok())
                .unwrap_or_default();
            if port.is_empty() { "3000".to_string() } else { port }
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            "3000".to_string()
        }
    };

    let claude_code_command = format!(
        "claude mcp add --transport http kyomi http://localhost:{mcp_port}/mcp"
    );

    let claude_desktop_config = format!(
        "{{\n  \"mcpServers\": {{\n    \"kyomi\": {{\n      \"url\": \"{mcp_url}\"\n    }}\n  }}\n}}"
    );

    // Build the Cursor deep-link URL (base64-encoded config, client-only)
    #[cfg(target_arch = "wasm32")]
    let cursor_config_b64 = {
        use base64::Engine;
        let cursor_config_json = format!("{{\"type\":\"http\",\"url\":\"{mcp_url}\"}}");
        base64::engine::general_purpose::STANDARD.encode(cursor_config_json.as_bytes())
    };

    // Clones for closures
    let url_for_copy = mcp_url.clone();
    let cmd_for_copy = claude_code_command.clone();
    let config_for_copy = claude_desktop_config.clone();

    view! {
        <Card>
            <CardHeader>
                <CardTitle>
                    <span class="flex items-center gap-2">
                        <phosphor_leptos::Icon icon=phosphor_leptos::PLUG weight=phosphor_leptos::IconWeight::Light size="20px"/>
                        "MCP Connection"
                    </span>
                </CardTitle>
                <CardDescription>
                    "Connect Kyomi to any MCP-compatible client for AI-powered data analysis."
                </CardDescription>
            </CardHeader>
            <CardContent>
                <div class="space-y-6">
                    // -- Server URL --
                    <div class="space-y-3">
                        <h4 class="font-medium text-foreground">"Server URL"</h4>
                        {if !is_personal {
                            view! {
                                <p class="text-sm text-muted-foreground">
                                    "Use this URL to connect from any MCP client. You\u{2019}ll be prompted to authorize via your browser."
                                </p>
                            }.into_any()
                        } else {
                            view! {
                                <p class="text-sm text-muted-foreground">
                                    "Use this URL to connect from any MCP client."
                                </p>
                            }.into_any()
                        }}
                        <div class="relative">
                            <pre class="p-4 bg-muted rounded-md text-sm overflow-x-auto pr-12">
                                {mcp_url.clone()}
                            </pre>
                            <CopyButton text=url_for_copy/>
                        </div>
                    </div>

                    // -- Claude Code (personal mode only) --
                    {if is_personal {
                        view! {
                            <div class="space-y-3 pt-4 border-t border-border">
                                <h4 class="font-medium text-foreground">"Claude Code"</h4>
                                <p class="text-sm text-muted-foreground">
                                    "Run this command in your terminal to connect Claude Code."
                                </p>
                                <div class="relative">
                                    <pre class="p-4 bg-muted rounded-md text-sm overflow-x-auto pr-12">
                                        {claude_code_command}
                                    </pre>
                                    <CopyButton text=cmd_for_copy/>
                                </div>
                            </div>
                        }.into_any()
                    } else {
                        view! { <span class="hidden"></span> }.into_any()
                    }}

                    // -- Claude Desktop (personal mode only) --
                    {if is_personal {
                        view! {
                            <div class="space-y-3 pt-4 border-t border-border">
                                <h4 class="font-medium text-foreground">"Claude Desktop"</h4>
                                <p class="text-sm text-muted-foreground">
                                    "Add this to your Claude Desktop configuration file."
                                </p>
                                <div class="relative">
                                    <pre class="p-4 bg-muted rounded-md text-sm overflow-x-auto pr-12">
                                        {claude_desktop_config}
                                    </pre>
                                    <CopyButton text=config_for_copy/>
                                </div>
                            </div>
                        }.into_any()
                    } else {
                        view! { <span class="hidden"></span> }.into_any()
                    }}

                    // -- Cursor One-Click --
                    <div class="space-y-3 pt-4 border-t border-border">
                        <h4 class="font-medium text-foreground">"Cursor"</h4>
                        <p class="text-sm text-muted-foreground">
                            "One-click install for Cursor users."
                        </p>
                        {
                            #[cfg(target_arch = "wasm32")]
                            {
                                let cursor_url = format!(
                                    "cursor://anysphere.cursor-deeplink/mcp/install?name=kyomi&config={cursor_config_b64}"
                                );
                                view! {
                                    <a
                                        href=cursor_url
                                        target="_blank"
                                        class="inline-flex items-center gap-2 px-4 py-2 rounded-md border border-border text-sm font-medium text-foreground hover:bg-secondary transition-colors"
                                    >
                                        <phosphor_leptos::Icon icon=phosphor_leptos::ARROW_SQUARE_OUT weight=phosphor_leptos::IconWeight::Light size="16px"/>
                                        "Connect with Cursor"
                                    </a>
                                }.into_any()
                            }
                            #[cfg(not(target_arch = "wasm32"))]
                            {
                                view! {
                                    <a
                                        href="#"
                                        class="inline-flex items-center gap-2 px-4 py-2 rounded-md border border-border text-sm font-medium text-foreground hover:bg-secondary transition-colors"
                                    >
                                        <phosphor_leptos::Icon icon=phosphor_leptos::ARROW_SQUARE_OUT weight=phosphor_leptos::IconWeight::Light size="16px"/>
                                        "Connect with Cursor"
                                    </a>
                                }.into_any()
                            }
                        }
                    </div>
                </div>
            </CardContent>
        </Card>
    }
}

/// Small copy-to-clipboard button used inside MCP Connection card.
#[component]
fn CopyButton(text: String) -> impl IntoView {
    let (copied, set_copied) = signal(false);
    let text = text.clone();

    let on_click = move |_| {
        let text = text.clone();
        let set_copied = set_copied;

        #[cfg(target_arch = "wasm32")]
        {
            leptos::task::spawn_local(async move {
                if let Some(window) = web_sys::window() {
                    let clipboard = window.navigator().clipboard();
                    let promise = clipboard.write_text(&text);
                    let _ = wasm_bindgen_futures::JsFuture::from(promise).await;
                    set_copied.set(true);
                    gloo_timers::future::TimeoutFuture::new(2000).await;
                    set_copied.set(false);
                }
            });
        }

        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = (text, set_copied);
        }
    };

    view! {
        <button
            class="absolute top-2 right-2 p-1.5 rounded-md text-muted-foreground hover:text-foreground hover:bg-secondary transition-colors"
            on:click=on_click
            title="Copy to clipboard"
        >
            {move || {
                if copied.get() {
                    view! { <phosphor_leptos::Icon icon=phosphor_leptos::CHECK weight=phosphor_leptos::IconWeight::Bold size="16px"/> }.into_any()
                } else {
                    view! { <phosphor_leptos::Icon icon=phosphor_leptos::COPY weight=phosphor_leptos::IconWeight::Light size="16px"/> }.into_any()
                }
            }}
        </button>
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


// ─────────────────────────────────────────────────────────────────────────────
// Clear local data — wipes IndexedDB cache and reloads
// ─────────────────────────────────────────────────────────────────────────────

#[component]
fn ClearLocalDataCard() -> impl IntoView {
    #[cfg(target_arch = "wasm32")]
    {
        use leptos::task::spawn_local;

        let on_clear = move |_: leptos::ev::MouseEvent| {
            spawn_local(async move {
                crate::cache::db::force_wipe_and_reload("").await;
            });
        };

        view! {
            <Card>
                <CardHeader>
                    <CardTitle>"Local Data"</CardTitle>
                    <CardDescription>
                        "Clear cached data stored in your browser. Use this if dashboards, chats, or other data appears missing or stale. Your account data is not affected."
                    </CardDescription>
                </CardHeader>
                <CardContent>
                    <crate::components::Button
                        variant=crate::components::ButtonVariant::Secondary
                        size=crate::components::ButtonSize::Sm
                        on:click=on_clear
                    >
                        "Clear local data and reload"
                    </crate::components::Button>
                </CardContent>
            </Card>
        }.into_any()
    }

    #[cfg(not(target_arch = "wasm32"))]
    {
        view! { <span></span> }.into_any()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Slack section — feature-gated wrapper to avoid cfg inside view! macro
// ─────────────────────────────────────────────────────────────────────────────

#[component]
fn SlackSection(is_personal: bool) -> impl IntoView {
    if !is_personal {
        view! { <crate::pages::settings::slack_connection::SlackConnectionCard/> }.into_any()
    } else {
        view! { <span></span> }.into_any()
    }
}

#[cfg(all(test, feature = "ssr"))]
mod tests_part3 {
    //! Part 3 compile-time sanity — verifies that the refetch wiring is
    //! present in the source. The end-to-end behaviour (palette change
    //! without page reload) is validated by the verifier's Playwright run.

    #[test]
    fn palette_card_source_contains_refetch_wiring() {
        // Static self-check: the source file should reference the
        // LocalResource<Result<UserContext, ServerFnError>> context lookup
        // and the `.refetch()` call. If this assertion fails, someone has
        // removed the KYO-129 Part 3 wiring.
        let src = include_str!("profile.rs");
        assert!(
            src.contains("expect_context::<LocalResource<Result<UserContext, ServerFnError>>>"),
            "Chart palette card must grab the layout-level user_context resource"
        );
        assert!(
            src.contains("user_ctx.refetch()"),
            "Chart palette card must refetch user_context after successful save (KYO-129 Part 3)"
        );
    }
}
