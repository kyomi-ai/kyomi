// SPDX-License-Identifier: AGPL-3.0-or-later

//! `ConnectStatusPanel` — the edit-mode body for a Kyomi Connect datasource.
//!
//! Replaces the connection/auth/credentials form that would normally fill the
//! Connection tab with a live status indicator, deployment command tabs, and
//! rotate/disconnect actions. Ports `apps/frontend/src/components/settings/
//! datasources/shared/components/ConnectStatus.jsx`.
//!
//! The panel polls `connect_status` every 10 seconds while mounted. The
//! interval handle is stored in a `StoredValue<SendWrapper<Interval>>` so
//! `on_cleanup` can drop it — never `.forget()`, never leaked.

use leptos::prelude::*;

use crate::components::{
    Alert, AlertDescription, AlertTitle, AlertVariant, Button, ButtonVariant, ConfirmDialog,
};
use crate::pages::settings::connect_deployment::{
    CopyButton, DeploymentCommands, DeploymentTabStrip, build_deployment_commands,
};
use crate::server_fns::connect::{
    ConnectStatusResponse, connect_status, disconnect_connect_datasource, rotate_connect_token,
};

/// How often to poll the Connect agent presence endpoint (milliseconds).
///
/// Matches the React reference which uses `setInterval(fetchStatus, 10000)`.
/// Only referenced from the WASM-only polling block; the SSR render path
/// never wires up a timer.
#[cfg(target_arch = "wasm32")]
const POLL_INTERVAL_MS: u32 = 10_000;

/// Edit-mode status panel for a Kyomi Connect datasource.
///
/// * `datasource_id` — database UUID; used for status/rotate/disconnect server_fns.
/// * `datasource_type` — provider type (e.g. `"postgres"`); drives the default
///   TCP port rendered into deployment commands.
#[component]
pub fn ConnectStatusPanel(
    #[prop(into)] datasource_id: String,
    #[prop(into)] datasource_type: String,
) -> impl IntoView {
    // ── Reactive state ───────────────────────────────────────────────────
    let (status, set_status) = signal::<Option<ConnectStatusResponse>>(None);
    let (loading, set_loading) = signal(true);

    // Rotate / disconnect in-flight flags — disable the action buttons while
    // a server_fn is pending.
    let (rotating, set_rotating) = signal(false);
    let (disconnecting, set_disconnecting) = signal(false);

    // Confirm dialog open state (one per action).
    let (show_rotate_confirm, set_show_rotate_confirm) = signal(false);
    let (show_disconnect_confirm, set_show_disconnect_confirm) = signal(false);

    // The token returned from a successful rotation, shown once with a
    // warning alert. Cleared on disconnect so the deployment commands
    // revert to `<YOUR_TOKEN>`. Matches React: the token is never persisted
    // across remounts.
    let (new_token, set_new_token) = signal::<Option<String>>(None);

    // Action error surfaces (rotate/disconnect failures).
    let (action_error, set_action_error) = signal::<Option<String>>(None);

    // Currently selected deployment tab. Default: Linux (matches React).
    let (active_deploy_tab, set_active_deploy_tab) = signal::<String>("linux".to_string());

    // ── Helpers ──────────────────────────────────────────────────────────
    // Clone for moves into closures.
    let ds_id = datasource_id.clone();
    let ds_type_for_cmds = datasource_type.clone();

    let fetch_status = {
        let ds_id = ds_id.clone();
        move || {
            let ds_id = ds_id.clone();
            leptos::task::spawn_local(async move {
                match connect_status(ds_id).await {
                    Ok(resp) => {
                        set_status.set(Some(resp));
                    }
                    Err(err) => {
                        // Don't surface polling failures in the UI — they'd
                        // flash every 10s on a broken server. Log only.
                        leptos::logging::warn!("Failed to fetch Connect status: {err}");
                    }
                }
                set_loading.set(false);
            });
        }
    };

    // ── Initial fetch + polling interval ─────────────────────────────────
    // The interval handle lives in a StoredValue so `on_cleanup` can drop it.
    // We deliberately do NOT use `.forget()` — that would leak the closure
    // and the timer would keep firing after the modal closed.
    #[cfg(target_arch = "wasm32")]
    {
        use send_wrapper::SendWrapper;

        // Kick off an immediate fetch so the user doesn't wait 10s for the
        // first status.
        fetch_status();

        let interval_handle: StoredValue<Option<SendWrapper<gloo_timers::callback::Interval>>> =
            StoredValue::new(None);

        let fetch_for_interval = fetch_status.clone();
        let interval = gloo_timers::callback::Interval::new(POLL_INTERVAL_MS, move || {
            fetch_for_interval();
        });
        interval_handle.set_value(Some(SendWrapper::new(interval)));

        on_cleanup(move || {
            interval_handle.set_value(None);
        });
    }

    // On SSR the server_fn can't be invoked from the render pass — leave the
    // loading state and let the WASM hydrate take over.
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = fetch_status;
    }

    // ── Rotate token ─────────────────────────────────────────────────────
    let on_rotate_click = move |_| {
        set_action_error.set(None);
        set_show_rotate_confirm.set(true);
    };

    let on_rotate_cancel = Callback::new(move |()| {
        set_show_rotate_confirm.set(false);
    });

    let on_rotate_confirm = {
        let ds_id = ds_id.clone();
        let fetch_status = fetch_status.clone();
        Callback::new(move |()| {
            set_show_rotate_confirm.set(false);
            set_rotating.set(true);
            set_action_error.set(None);

            let ds_id = ds_id.clone();
            let fetch_status = fetch_status.clone();
            leptos::task::spawn_local(async move {
                match rotate_connect_token(ds_id).await {
                    Ok(token) => {
                        set_new_token.set(Some(token));
                        fetch_status();
                    }
                    Err(err) => {
                        set_action_error.set(Some(format!("Failed to rotate token: {err}")));
                    }
                }
                set_rotating.set(false);
            });
        })
    };

    // ── Disconnect ───────────────────────────────────────────────────────
    let on_disconnect_click = move |_| {
        set_action_error.set(None);
        set_show_disconnect_confirm.set(true);
    };

    let on_disconnect_cancel = Callback::new(move |()| {
        set_show_disconnect_confirm.set(false);
    });

    let on_disconnect_confirm = {
        let ds_id = ds_id.clone();
        let fetch_status = fetch_status.clone();
        Callback::new(move |()| {
            set_show_disconnect_confirm.set(false);
            set_disconnecting.set(true);
            set_action_error.set(None);

            let ds_id = ds_id.clone();
            let fetch_status = fetch_status.clone();
            leptos::task::spawn_local(async move {
                match disconnect_connect_datasource(ds_id).await {
                    Ok(()) => {
                        // Drop any just-rotated token — after disconnect the
                        // agent must be redeployed with a brand new one.
                        set_new_token.set(None);
                        fetch_status();
                    }
                    Err(err) => {
                        set_action_error.set(Some(format!("Failed to disconnect: {err}")));
                    }
                }
                set_disconnecting.set(false);
            });
        })
    };

    // ── Derived: deployment commands for the current (token, tab) ────────
    let deployment_commands: Memo<DeploymentCommands> = Memo::new(move |_| {
        let token = new_token.get();
        build_deployment_commands(&ds_type_for_cmds, token.as_deref(), None)
    });

    let active_command = move || -> String {
        let tab = active_deploy_tab.get();
        deployment_commands.with(|cmds| cmds.for_tab(&tab).to_string())
    };

    view! {
        <div class="space-y-6">
            // ── Connection status indicator ────────────────────────────
            // Green dot = agent online; muted dot = waiting. The inner text
            // copy matches the React reference verbatim.
            {move || {
                if loading.get() {
                    view! {
                        <div class="flex items-center gap-3 p-4 rounded-md border border-border bg-muted/30">
                            <div class="w-3 h-3 rounded-full bg-muted-foreground/40"></div>
                            <div class="text-sm text-muted-foreground">"Loading status..."</div>
                        </div>
                    }.into_any()
                } else {
                    let connected = status.get().map(|s| s.connected).unwrap_or(false);
                    let (dot_class, title, subtitle) = if connected {
                        (
                            "w-3 h-3 rounded-full bg-success-foreground animate-pulse",
                            "Connected",
                            "Kyomi Connect agent is online",
                        )
                    } else {
                        (
                            "w-3 h-3 rounded-full bg-muted-foreground/60",
                            "Disconnected",
                            "Waiting for agent to connect",
                        )
                    };
                    view! {
                        <div class="flex items-center gap-3 p-4 rounded-md border border-border">
                            <div class=dot_class></div>
                            <div>
                                <div class="text-sm font-medium text-foreground">{title}</div>
                                <div class="text-xs text-muted-foreground">{subtitle}</div>
                            </div>
                        </div>
                    }.into_any()
                }
            }}

            // ── Action error ─────────────────────────────────────────
            {move || action_error.get().map(|msg| view! {
                <Alert variant=AlertVariant::Error>
                    <AlertDescription>{msg}</AlertDescription>
                </Alert>
            })}

            // ── New token display (after rotation) ───────────────────
            // Shown once, with a warning alert. The deployment commands
            // below automatically pick up the new token via the memo.
            <Show when=move || new_token.get().is_some()>
                <div class="space-y-3">
                    <Alert variant=AlertVariant::Warning>
                        <AlertTitle>"New token generated"</AlertTitle>
                        <AlertDescription>
                            "Save this token now — it will not be shown again."
                        </AlertDescription>
                    </Alert>
                    <div class="space-y-1.5">
                        <label class="block text-sm font-medium text-foreground">
                            "New Connect Token"
                        </label>
                        <div class="flex items-center gap-2 rounded-md border border-border bg-muted/30 px-3 py-2">
                            <code class="flex-1 text-xs font-mono text-foreground break-all select-all">
                                {move || new_token.get().unwrap_or_default()}
                            </code>
                            <CopyButton text=Signal::derive(move || {
                                new_token.get().unwrap_or_default()
                            })/>
                        </div>
                    </div>
                </div>
            </Show>

            // ── Deployment instructions ──────────────────────────────
            <div class="space-y-2">
                <h4 class="text-sm font-medium text-foreground">"Deployment Instructions"</h4>
                <Show when=move || new_token.get().is_none()>
                    <p class="text-xs text-muted-foreground">
                        "Rotate the token above to generate a new one, then copy the commands below."
                    </p>
                </Show>

                // Tab strip — underline-active pattern per DESIGN.md.
                <DeploymentTabStrip
                    active_tab=active_deploy_tab.into()
                    set_active_tab=set_active_deploy_tab
                />

                // Code block + copy button.
                <div class="relative rounded-md border border-border bg-muted/30">
                    <pre class="p-4 pr-12 text-xs font-mono text-foreground overflow-x-auto whitespace-pre">
                        {active_command}
                    </pre>
                    <div class="absolute top-2 right-2">
                        <CopyButton text=Signal::derive(move || {
                            let tab = active_deploy_tab.get();
                            deployment_commands.with(|cmds| cmds.for_tab(&tab).to_string())
                        })/>
                    </div>
                </div>
            </div>

            // ── Management buttons ───────────────────────────────────
            <div class="flex gap-3">
                <Button
                    variant=ButtonVariant::Outline
                    disabled=rotating
                    on:click=on_rotate_click
                >
                    {move || if rotating.get() { "Rotating..." } else { "Rotate Token" }}
                </Button>
                <Button
                    variant=ButtonVariant::Destructive
                    disabled=disconnecting
                    on:click=on_disconnect_click
                >
                    {move || if disconnecting.get() { "Disconnecting..." } else { "Disconnect" }}
                </Button>
            </div>
            <p class="text-xs text-muted-foreground">
                "Rotating the token generates a new token and immediately disconnects the current agent. \
                 The agent must be restarted with the new token."
            </p>

            // ── Confirm dialogs ──────────────────────────────────────
            <ConfirmDialog
                open=show_rotate_confirm
                title="Rotate Token?"
                message="Rotating the token will disconnect the current Connect agent. You will need to restart it with the new token. Continue?"
                confirm_text="Rotate"
                destructive=true
                on_confirm=on_rotate_confirm
                on_cancel=on_rotate_cancel
            />
            <ConfirmDialog
                open=show_disconnect_confirm
                title="Disconnect Agent?"
                message="This will revoke the token and disconnect the Connect agent. You will need to redeploy it with a new token. Continue?"
                confirm_text="Disconnect"
                destructive=true
                on_confirm=on_disconnect_confirm
                on_cancel=on_disconnect_cancel
            />
        </div>
    }
}

