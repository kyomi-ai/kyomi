// SPDX-License-Identifier: AGPL-3.0-or-later

//! Slack Connection card — profile settings section for linking a Slack account.
//!
//! Replaces `apps/frontend/src/components/settings/ProfileSettings.jsx` lines 607-745.
//!
//! Gated behind `#[cfg(feature = "slack")]` at the module level (mod.rs).
//! Hidden in personal mode and when Slack is not available.

use leptos::prelude::*;

use crate::components::{
    Alert, AlertDescription, AlertVariant, Button, ButtonSize, ButtonVariant, Card, CardContent,
    CardDescription, CardHeader, CardTitle, ConfirmDialog, Label,
};
use crate::server_fns::slack::*;

// ─────────────────────────────────────────────────────────────────────────────
// Main card component
// ─────────────────────────────────────────────────────────────────────────────

/// Slack Connection card for the Profile Settings page.
///
/// Displays workspace installation status, user connection status, a channel
/// selector for the default watch channel, and connect/disconnect controls.
#[component]
pub fn SlackConnectionCard() -> impl IntoView {
    let slack_status = Resource::new(|| (), |_| get_slack_status());

    view! {
        <Transition fallback=|| ()>
            {move || Suspend::new(async move {
                match slack_status.await {
                    Ok(status) => view! { <SlackConnectionInner status=status/> }.into_any(),
                    // Tier error or unavailable — hide the card entirely
                    Err(_) => view! { <span class="hidden"></span> }.into_any(),
                }
            })}
        </Transition>
    }
}

/// Inner component rendered once the Slack status has loaded.
#[component]
fn SlackConnectionInner(status: SlackStatus) -> impl IntoView {
    let (error, set_error) = signal(Option::<String>::None);
    let (disconnecting, set_disconnecting) = signal(false);
    let (show_disconnect_dialog, set_show_disconnect_dialog) = signal(false);
    let (user_connected, set_user_connected) = signal(status.user_connected);
    let (slack_username, _set_slack_username) = signal(status.slack_username.clone());
    let team_name = status.slack_team_name.clone().unwrap_or_default();
    let workspace_connected = status.workspace_connected;

    // Channel resources — only fetched when user is connected
    let channels = Resource::new(
        move || user_connected.get(),
        |connected| async move {
            if connected {
                get_slack_channels().await.ok().unwrap_or_default()
            } else {
                Vec::new()
            }
        },
    );
    let default_channel = Resource::new(
        move || user_connected.get(),
        |connected| async move {
            if connected {
                get_default_watch_channel().await.ok()
            } else {
                None
            }
        },
    );

    // -- Connect handler --
    let connect_action = Action::new(move |_: &()| async move {
        match slack_connect().await {
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
                set_error.set(Some(e.to_string()));
            }
        }
    });

    // -- Disconnect handler --
    let disconnect_action = Action::new(move |_: &()| async move {
        set_disconnecting.set(true);
        match slack_disconnect().await {
            Ok(()) => {
                set_user_connected.set(false);
                set_error.set(None);
            }
            Err(e) => {
                set_error.set(Some(e.to_string()));
            }
        }
        set_disconnecting.set(false);
    });

    // -- Default channel save handler --
    let (saving_channel, set_saving_channel) = signal(false);
    let save_channel = move |id: Option<String>, name: Option<String>| {
        set_saving_channel.set(true);
        let set_saving = set_saving_channel;
        let set_err = set_error;
        leptos::task::spawn_local(async move {
            match set_default_watch_channel(id, name).await {
                Ok(_) => {}
                Err(e) => { set_err.try_set(Some(e.to_string())); }
            }
            set_saving.try_set(false);
        });
    };

    let on_confirm_disconnect = Callback::new(move |()| {
        set_show_disconnect_dialog.set(false);
        let _ = disconnect_action.dispatch(());
    });
    let on_cancel_disconnect = Callback::new(move |()| {
        set_show_disconnect_dialog.set(false);
    });

    let team_name_connected = team_name.clone();
    let team_name_not_connected = team_name.clone();

    view! {
        <Card>
            <CardHeader>
                <CardTitle>"Slack Connection"</CardTitle>
                <CardDescription>
                    "Link your Slack account to receive watch alerts in Slack channels."
                </CardDescription>
            </CardHeader>
            <CardContent>
                // Error alert
                {move || {
                    error.get().map(|msg| view! {
                        <Alert variant=AlertVariant::Error class="mb-4">
                            <AlertDescription>{msg}</AlertDescription>
                        </Alert>
                    })
                }}

                {if !workspace_connected {
                    // Workspace doesn't have Slack installed
                    view! {
                        <div class="flex items-start gap-3 p-4 bg-muted/50 rounded-lg">
                            <span class="h-5 w-5 text-warning-foreground mt-0.5">
                                <phosphor_leptos::Icon icon=phosphor_leptos::WARNING size="20px"/>
                            </span>
                            <div>
                                <p class="text-sm text-foreground font-medium">"Slack not installed"</p>
                                <p class="text-sm text-muted-foreground mt-1">
                                    "Ask your workspace admin to install the Kyomi Slack app in Workspace Settings."
                                </p>
                            </div>
                        </div>
                    }.into_any()
                } else {
                    // Workspace has Slack installed
                    view! {
                        <Show
                            when=move || user_connected.get()
                            fallback={
                                let team = team_name_not_connected.clone();
                                move || {
                                    let team = team.clone();
                                    view! {
                                        <div class="space-y-4">
                                            <p class="text-sm text-muted-foreground">
                                                "Connect your Slack account to send watch alerts to "
                                                <strong>{team.clone()}</strong>
                                                "."
                                            </p>
                                            <div class="space-y-2">
                                                <Button on:click=move |_| { let _ = connect_action.dispatch(()); }>
                                                    <phosphor_leptos::Icon icon=phosphor_leptos::ARROW_SQUARE_OUT size="16px"/>
                                                    "Connect with Slack"
                                                </Button>
                                                <p class="text-xs text-muted-foreground">
                                                    "Or type "
                                                    <code class="px-1 py-0.5 bg-muted rounded-md text-xs">"/kyomi connect"</code>
                                                    " in Slack"
                                                </p>
                                            </div>
                                        </div>
                                    }
                                }
                            }
                        >
                            // User is connected
                            {
                                let team = team_name_connected.clone();
                                view! {
                                    <div class="space-y-4">
                                        <div class="flex items-center gap-2">
                                            <div class="h-2 w-2 rounded-full bg-success"/>
                                            <span class="text-sm text-foreground">
                                                "Connected as "
                                                <strong>"@" {move || slack_username.get().unwrap_or_else(|| "unknown".to_string())}</strong>
                                                " in "
                                                <strong>{team.clone()}</strong>
                                            </span>
                                        </div>
                                        <p class="text-xs text-muted-foreground">
                                            "Your watches can now post alerts to Slack channels."
                                        </p>

                                        // Default Watch Channel Selector
                                        <div class="space-y-2">
                                            <Label>
                                                <span class="flex items-center gap-2">
                                                    <phosphor_leptos::Icon icon=phosphor_leptos::CHAT size="16px"/>
                                                    "Default Watch Channel"
                                                </span>
                                            </Label>

                                            <Transition fallback=move || view! {
                                                <div class="flex items-center gap-2 text-sm text-muted-foreground">
                                                    <span class="animate-spin h-4 w-4 border-2 border-current border-t-transparent rounded-full"/>
                                                    <span>"Loading channels..."</span>
                                                </div>
                                            }>
                                                {move || {
                                                    let channel_list = channels.get()
                                                        .unwrap_or_default();
                                                    let current_channel_id = default_channel.get()
                                                        .flatten()
                                                        .and_then(|wc| wc.channel_id)
                                                        .unwrap_or_else(|| "none".to_string());

                                                    if channel_list.is_empty() {
                                                        view! {
                                                            <div class="flex items-start gap-2 p-3 bg-muted/50 rounded-lg text-sm text-muted-foreground">
                                                                <span class="text-warning-foreground mt-0.5 shrink-0">
                                                                    <phosphor_leptos::Icon icon=phosphor_leptos::WARNING size="16px"/>
                                                                </span>
                                                                <span>
                                                                    "Invite the Kyomi app to a Slack channel first. Then refresh this page to see available channels."
                                                                </span>
                                                            </div>
                                                        }.into_any()
                                                    } else {
                                                        let save = save_channel;
                                                        // Build options: "none" sentinel + each channel.
                                                        let mut channel_opts: Vec<(String, String)> =
                                                            vec![("none".to_string(), "No default channel".to_string())];
                                                        channel_opts.extend(channel_list.iter().map(|ch| {
                                                            (ch.channel_id.clone(), format!("#{}", ch.channel_name))
                                                        }));
                                                        let channel_list_for_cb = channel_list.clone();
                                                        view! {
                                                            <div class="space-y-2">
                                                                <crate::components::select::Select
                                                                    value=Signal::derive(move || current_channel_id.clone())
                                                                    options=Signal::derive(move || channel_opts.clone())
                                                                    disabled=Signal::derive(move || saving_channel.get())
                                                                    on_change=move |value| {
                                                                        if value == "none" {
                                                                            save(None, None);
                                                                        } else {
                                                                            let name = channel_list_for_cb.iter()
                                                                                .find(|ch| ch.channel_id == value)
                                                                                .map(|ch| ch.channel_name.clone());
                                                                            save(Some(value), name);
                                                                        }
                                                                    }
                                                                />
                                                                <p class="text-xs text-muted-foreground">
                                                                    "New watches will post alerts to this channel by default. You can override this for individual watches. If you don\u{2019}t see a channel, add the Kyomi app to it in Slack and refresh this page."
                                                                </p>
                                                            </div>
                                                        }.into_any()
                                                    }
                                                }}
                                            </Transition>
                                        </div>

                                        // Disconnect button
                                        {move || {
                                            view! {
                                                <Button
                                                    variant=ButtonVariant::Outline
                                                    size=ButtonSize::Sm
                                                    disabled=disconnecting.get()
                                                    on:click=move |_| set_show_disconnect_dialog.set(true)
                                                >
                                                    {if disconnecting.get() {
                                                        view! {
                                                            <span class="flex items-center gap-2">
                                                                <span class="animate-spin h-4 w-4 border-2 border-current border-t-transparent rounded-full"/>
                                                                "Disconnecting..."
                                                            </span>
                                                        }.into_any()
                                                    } else {
                                                        view! {
                                                            <span class="flex items-center gap-2">
                                                                <phosphor_leptos::Icon icon=phosphor_leptos::PLUGS size="16px"/>
                                                                "Disconnect"
                                                            </span>
                                                        }.into_any()
                                                    }}
                                                </Button>
                                            }
                                        }}

                                        // Confirm disconnect dialog
                                        <ConfirmDialog
                                            open=Signal::from(show_disconnect_dialog)
                                            title="Disconnect Slack?"
                                            message="This will unlink your Slack account. Watch alerts will no longer be sent to Slack channels."
                                            confirm_text="Disconnect"
                                            on_confirm=on_confirm_disconnect
                                            on_cancel=on_cancel_disconnect
                                        />
                                    </div>
                                }
                            }
                        </Show>
                    }.into_any()
                }}
            </CardContent>
        </Card>
    }
}
