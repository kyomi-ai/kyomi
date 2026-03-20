// SPDX-License-Identifier: AGPL-3.0-or-later

//! Workspace settings page — admin-only workspace configuration.
//!
//! Replaces `apps/frontend/src/components/settings/WorkspaceSettings.jsx`.
//! All data fetching uses server functions instead of REST API calls.

use leptos::prelude::*;

use crate::components::{
    ActionStatus, Alert, AlertDescription, AlertVariant, Button, ButtonVariant, Card, CardContent,
    CardDescription, CardHeader, CardTitle, Label, INPUT_CLASS,
};
use crate::server_fns::workspace::*;
use crate::types::WorkspaceSettingsData;

// ─────────────────────────────────────────────────────────────────────────────
// Palette data — matches apps/frontend/src/config/chartPalettes.js
// (shared with profile.rs — same static data)
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
// Model options
// ─────────────────────────────────────────────────────────────────────────────

const MODEL_OPTIONS: &[(&str, &str)] = &[
    ("claude-sonnet-4-5-20250929", "Claude Sonnet 4.5"),
    ("claude-sonnet-4-20250514", "Claude Sonnet 4"),
    ("gpt-4o", "GPT-4o"),
    ("gpt-4o-mini", "GPT-4o Mini"),
];

// ─────────────────────────────────────────────────────────────────────────────
// Main page
// ─────────────────────────────────────────────────────────────────────────────

#[component]
pub fn WorkspacePage() -> impl IntoView {
    let settings = Resource::new(|| (), |_| get_workspace_settings());

    view! {
        <div class="p-6">
            <h2 class="text-xl font-semibold text-foreground mb-4">"Workspace Settings"</h2>
            <p class="text-muted-foreground mb-6">
                "Configure workspace-wide preferences (admin only)."
            </p>

            <Suspense fallback=move || view! {
                <Card>
                    <CardContent>
                        <p class="text-sm text-muted-foreground text-center py-8">
                            "Loading workspace settings..."
                        </p>
                    </CardContent>
                </Card>
            }>
                {move || {
                    settings.get().map(|result| match result {
                        Ok(data) => {
                            let data_name = data.clone();
                            let data_model = data.clone();
                            let data_palette = data.clone();

                            view! {
                                <div class="space-y-6">
                                    <WorkspaceNameCard data=data_name/>
                                    <DefaultModelCard data=data_model/>
                                    <WorkspaceChartPaletteCard data=data_palette/>
                                    <KnowledgeGraphCard/>
                                    <WorkspaceSlackSection/>
                                </div>
                            }.into_any()
                        },
                        Err(e) => {
                            let msg = e.to_string();
                            view! {
                                <Card>
                                    <div class="p-6">
                                        <p class="text-error-foreground">"Failed to load workspace settings: " {msg}</p>
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
// Workspace Name Card
// ─────────────────────────────────────────────────────────────────────────────

#[component]
fn WorkspaceNameCard(data: WorkspaceSettingsData) -> impl IntoView {
    let (name, set_name) = signal(data.workspace_name.clone());
    let save_action = Action::new(|name: &String| {
        let name = name.clone();
        async move { update_workspace_name(name).await }
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
                        <CardTitle>"Workspace Name"</CardTitle>
                        <CardDescription>
                            "Give your workspace a meaningful name to help identify it."
                        </CardDescription>
                    </div>
                    <ActionStatus action=save_action/>
                </div>
            </CardHeader>
            <CardContent>
                <input
                    type="text"
                    class=INPUT_CLASS
                    placeholder="My Workspace"
                    prop:value=name
                    on:input=move |ev| set_name.set(event_target_value(&ev))
                    on:blur=on_blur
                />
            </CardContent>
        </Card>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Default AI Model Card
// ─────────────────────────────────────────────────────────────────────────────

#[component]
fn DefaultModelCard(data: WorkspaceSettingsData) -> impl IntoView {
    let save_action = Action::new(|model: &String| {
        let model = model.clone();
        async move { update_workspace_model(model).await }
    });

    view! {
        <Card>
            <CardHeader>
                <div class="flex items-center justify-between">
                    <div>
                        <CardTitle>"Default AI Model"</CardTitle>
                        <CardDescription>
                            "Choose the default AI model for workspace conversations."
                        </CardDescription>
                    </div>
                    <ActionStatus action=save_action/>
                </div>
            </CardHeader>
            <CardContent>
                <div class="max-w-md space-y-2">
                    <Label>"Model"</Label>
                    {
                        let options: Vec<(&'static str, &'static str)> = MODEL_OPTIONS.to_vec();
                        view! {
                            <crate::components::StyledSelect
                                value=data.default_model.clone()
                                options=options
                                on_change=move |val| {
                                    save_action.dispatch(val);
                                }
                            />
                        }
                    }
                </div>
            </CardContent>
        </Card>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Workspace Chart Palette Card
// ─────────────────────────────────────────────────────────────────────────────

#[component]
fn WorkspaceChartPaletteCard(data: WorkspaceSettingsData) -> impl IntoView {
    let (palette, set_palette) = signal(data.chart_palette.clone());
    let save_action = Action::new(|palette: &String| {
        let palette = palette.clone();
        async move { update_workspace_chartml_config(palette).await }
    });

    view! {
        <Card>
            <CardHeader>
                <div class="flex items-center justify-between">
                    <div>
                        <CardTitle>"Workspace Chart Palette"</CardTitle>
                        <CardDescription>"Choose the default color palette for workspace charts. Individual users can override this."</CardDescription>
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
// Knowledge Graph Card
// ─────────────────────────────────────────────────────────────────────────────

#[component]
fn KnowledgeGraphCard() -> impl IntoView {
    let rebuild_action = Action::new(|_: &()| async move {
        populate_knowledge_graph().await
    });

    view! {
        <Card>
            <CardHeader>
                <CardTitle>"Knowledge Graph"</CardTitle>
                <CardDescription>
                    "Rebuild the knowledge graph from your catalog and learnings. This fixes stale or missing graph data."
                </CardDescription>
            </CardHeader>
            <CardContent>
                {move || {
                    let value = rebuild_action.value().get();
                    match value {
                        Some(Ok(result)) => {
                            view! {
                                <Alert variant=AlertVariant::Success attr:class="mb-4">
                                    <AlertDescription>
                                        {format!(
                                            "Graph rebuilt: {} learnings with references.",
                                            result.learnings_with_references
                                        )}
                                    </AlertDescription>
                                </Alert>
                            }.into_any()
                        },
                        Some(Err(e)) => {
                            view! {
                                <Alert variant=AlertVariant::Error attr:class="mb-4">
                                    <AlertDescription>{e.to_string()}</AlertDescription>
                                </Alert>
                            }.into_any()
                        },
                        None => view! { <span></span> }.into_any(),
                    }
                }}
                <Button
                    variant=ButtonVariant::Outline
                    on:click=move |_| { rebuild_action.dispatch(()); }
                    disabled=rebuild_action.pending().get()
                >
                    {move || {
                        if rebuild_action.pending().get() {
                            view! {
                                <span class="inline-flex items-center gap-2">
                                    <span class="h-4 w-4 animate-spin rounded-full border-2 border-current border-t-transparent"></span>
                                    "Rebuilding..."
                                </span>
                            }.into_any()
                        } else {
                            view! {
                                <span class="inline-flex items-center gap-2">
                                    <span class="inline-flex">
                                        <leptos_icons::Icon icon=icondata_lu::LuRefreshCw width="16" height="16"/>
                                    </span>
                                    "Rebuild Graph"
                                </span>
                            }.into_any()
                        }
                    }}
                </Button>
            </CardContent>
        </Card>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Slack section — feature-gated wrapper to avoid cfg inside view! macro
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(feature = "slack")]
#[component]
fn WorkspaceSlackSection() -> impl IntoView {
    view! { <WorkspaceSlackCard/> }
}

#[cfg(not(feature = "slack"))]
#[component]
fn WorkspaceSlackSection() -> impl IntoView {
    view! { <span></span> }
}

#[cfg(feature = "slack")]
#[component]
fn WorkspaceSlackCard() -> impl IntoView {
    let slack_status = Resource::new(|| (), |_| get_workspace_slack_status());
    let (slack_error, set_slack_error) = signal::<Option<String>>(None);
    let (slack_success, set_slack_success) = signal::<Option<String>>(None);
    let (uninstalling, set_uninstalling) = signal(false);

    // Check URL params for OAuth callback result
    #[cfg(target_arch = "wasm32")]
    {
        if let Some(window) = web_sys::window() {
            if let Ok(search) = window.location().search() {
                if search.contains("slack=installed") {
                    set_slack_success.set(Some("Kyomi has been added to your Slack workspace!".to_string()));
                    // Clear URL param
                    let _ = window.history().and_then(|h| {
                        let pathname = window.location().pathname().unwrap_or_default();
                        h.replace_state_with_url(&wasm_bindgen::JsValue::NULL, "", Some(&pathname))
                    });
                } else if search.contains("slack=error") {
                    set_slack_error.set(Some("Failed to install Slack integration. Please try again.".to_string()));
                    let _ = window.history().and_then(|h| {
                        let pathname = window.location().pathname().unwrap_or_default();
                        h.replace_state_with_url(&wasm_bindgen::JsValue::NULL, "", Some(&pathname))
                    });
                }
            }
        }
    }

    let handle_install = move |_| {
        set_slack_error.set(None);
        leptos::task::spawn_local(async move {
            match get_slack_install_url().await {
                Ok(url) => {
                    #[cfg(target_arch = "wasm32")]
                    {
                        if let Some(window) = web_sys::window() {
                            let _ = window.location().set_href(&url);
                        }
                    }
                    #[cfg(not(target_arch = "wasm32"))]
                    {
                        let _ = url;
                    }
                }
                Err(e) => {
                    set_slack_error.set(Some(e.to_string()));
                }
            }
        });
    };

    let handle_uninstall = {
        let slack_status = slack_status;
        move |_| {
            // Get the team_id from current status for the uninstall call
            let team_id = slack_status
                .get()
                .and_then(|r| r.ok())
                .and_then(|s| s.team_id.clone())
                .unwrap_or_default();

            set_uninstalling.set(true);
            set_slack_error.set(None);
            set_slack_success.set(None);

            leptos::task::spawn_local(async move {
                match uninstall_workspace_slack(team_id).await {
                    Ok(()) => {
                        set_slack_success.set(Some("Slack integration removed successfully.".to_string()));
                        slack_status.refetch();
                    }
                    Err(e) => {
                        set_slack_error.set(Some(e.to_string()));
                    }
                }
                set_uninstalling.set(false);
            });
        }
    };

    view! {
        <Card>
            <CardHeader>
                <div class="flex items-center justify-between">
                    <div>
                        <CardTitle>"Slack Integration"</CardTitle>
                        <CardDescription>
                            "Enable watch alerts to post to Slack channels as Kyomi."
                        </CardDescription>
                    </div>
                </div>
            </CardHeader>
            <CardContent>
                // Success alert
                {move || slack_success.get().map(|msg| view! {
                    <Alert variant=AlertVariant::Success attr:class="mb-4">
                        <AlertDescription>{msg}</AlertDescription>
                    </Alert>
                })}

                // Error alert
                {move || slack_error.get().map(|msg| view! {
                    <Alert variant=AlertVariant::Error attr:class="mb-4">
                        <AlertDescription>{msg}</AlertDescription>
                    </Alert>
                })}

                <Suspense fallback=move || view! {
                    <div class="flex items-center gap-2 text-muted-foreground">
                        <span class="h-4 w-4 animate-spin rounded-full border-2 border-current border-t-transparent"></span>
                        <span>"Loading Slack status..."</span>
                    </div>
                }>
                    {move || {
                        slack_status.get().map(|result| match result {
                            Ok(status) if status.installed => {
                                let team_name = status.team_name.clone().unwrap_or_default();
                                view! {
                                    <div class="space-y-4">
                                        <div class="flex items-center gap-2">
                                            <div class="h-2 w-2 rounded-full bg-success"></div>
                                            <span class="text-sm text-foreground">
                                                "Connected to "
                                                <strong>{team_name}</strong>
                                            </span>
                                        </div>
                                        <Button
                                            variant=ButtonVariant::Outline
                                            on:click=handle_uninstall.clone()
                                            disabled=uninstalling.get()
                                        >
                                            {move || {
                                                if uninstalling.get() {
                                                    view! {
                                                        <span class="inline-flex items-center gap-2">
                                                            <span class="h-4 w-4 animate-spin rounded-full border-2 border-current border-t-transparent"></span>
                                                            "Removing..."
                                                        </span>
                                                    }.into_any()
                                                } else {
                                                    view! {
                                                        <span class="inline-flex items-center gap-2">
                                                            <span class="inline-flex">
                                                                <leptos_icons::Icon icon=icondata_lu::LuUnplug width="16" height="16"/>
                                                            </span>
                                                            "Remove Integration"
                                                        </span>
                                                    }.into_any()
                                                }
                                            }}
                                        </Button>
                                    </div>
                                }.into_any()
                            },
                            Ok(_) => {
                                // Not installed
                                view! {
                                    <div class="space-y-4">
                                        <p class="text-sm text-muted-foreground">
                                            "Connect Kyomi to your Slack workspace to receive watch alerts in channels."
                                        </p>
                                        <Button on:click=handle_install.clone()>
                                            <span class="inline-flex items-center gap-2">
                                                <span class="inline-flex">
                                                    <leptos_icons::Icon icon=icondata_lu::LuExternalLink width="16" height="16"/>
                                                </span>
                                                "Add Kyomi to Slack"
                                            </span>
                                        </Button>
                                    </div>
                                }.into_any()
                            },
                            Err(e) => {
                                let msg = e.to_string();
                                // If the error is about tier, show upgrade prompt
                                if msg.contains("Team and Enterprise") {
                                    view! {
                                        <Alert variant=AlertVariant::Info>
                                            <AlertDescription>
                                                "Slack integration is available on Team and Enterprise plans. "
                                                <a href="/settings/billing" class="text-primary font-medium hover:underline">
                                                    "Upgrade to enable"
                                                </a>
                                            </AlertDescription>
                                        </Alert>
                                    }.into_any()
                                } else {
                                    view! {
                                        <Alert variant=AlertVariant::Error>
                                            <AlertDescription>{msg}</AlertDescription>
                                        </Alert>
                                    }.into_any()
                                }
                            },
                        })
                    }}
                </Suspense>
            </CardContent>
        </Card>
    }
}
