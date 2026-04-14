// SPDX-License-Identifier: AGPL-3.0-or-later

//! Workspace settings page — admin-only workspace configuration.
//!
//! Replaces `apps/frontend/src/components/settings/WorkspaceSettings.jsx`.
//! All data fetching uses server functions instead of REST API calls.

use leptos::prelude::*;

use crate::components::{
    ActionStatus, Alert, AlertDescription, AlertVariant, Button, ButtonVariant, Card, CardContent,
    CardDescription, CardHeader, CardTitle, INPUT_CLASS,
};
use crate::server_fns::workspace::*;
use crate::types::WorkspaceSettingsData;

// ─────────────────────────────────────────────────────────────────────────────
// Main page
// ─────────────────────────────────────────────────────────────────────────────

#[component]
pub fn WorkspacePage() -> impl IntoView {
    let settings = Resource::new(|| (), |_| get_workspace_settings());

    view! {
        <div class="p-4 sm:p-6">
            <h2 class="text-xl font-display text-foreground mb-4">"Workspace Settings"</h2>
            <p class="text-muted-foreground mb-6">
                "Configure workspace-wide preferences (admin only)."
            </p>

            <Transition fallback=move || view! {
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
                            view! {
                                <div class="space-y-6">
                                    <WorkspaceNameCard data=data/>
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
            </Transition>
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

                <Transition fallback=move || view! {
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
                                            on:click=handle_uninstall
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
                                        <Button on:click=handle_install>
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
                </Transition>
            </CardContent>
        </Card>
    }
}
