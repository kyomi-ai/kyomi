// SPDX-License-Identifier: AGPL-3.0-or-later

//! WatchPreviewCard component — matches
//! `apps/frontend/src/components/watches/WatchPreviewCard.jsx` exactly.
//!
//! Preview card shown in chat before creating/updating a watch. Supports two
//! modes:
//! 1. Self-contained (`on_approve`): calls `create_watch`/`update_watch`
//!    server functions directly, manages its own loading and success state.
//! 2. Controlled (`on_confirm`): parent handles the API call; this component
//!    just renders the preview and fires the callback on click.

use leptos::prelude::*;
use leptos_icons::Icon;
use serde::{Deserialize, Serialize};

use crate::components::{
    Badge, BadgeVariant, Button, ButtonSize, ButtonVariant, Card, CardContent, CardHeader,
    CardTitle, Spinner,
};
use crate::utils::cron::{describe_cron, get_tz_offset_minutes};

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Watch preview configuration from the AI agent response.
///
/// Used by `WatchPreviewCard` to render the preview and submit create/update.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WatchPreviewConfig {
    /// `None` for new watches, `Some(id)` for updates.
    pub watch_id: Option<String>,
    pub name: String,
    pub prompt: String,
    pub schedule: String,
    /// `"alert"` or `"report"`. Defaults to `"alert"` if `None`.
    pub mode: Option<String>,
    pub queries: Option<Vec<WatchQuery>>,
}

/// A reference query attached to a watch preview.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WatchQuery {
    pub comment: Option<String>,
    pub sql: String,
    pub datasource: Option<String>,
}

// ---------------------------------------------------------------------------
// Component
// ---------------------------------------------------------------------------

/// Watch preview card shown in the chat before creating/updating a watch.
///
/// React reference: `apps/frontend/src/components/watches/WatchPreviewCard.jsx`
///
/// # Modes
/// - **Self-contained** (`on_approve`): calls `create_watch`/`update_watch`
///   server functions when the user clicks Accept.
/// - **Controlled** (`on_confirm`): fires the callback with the config and
///   lets the parent handle the API call.
#[component]
pub fn WatchPreviewCard(
    /// Watch configuration from the AI agent response.
    watch_config: WatchPreviewConfig,
    /// Self-contained mode: called after successful creation with the watch_id.
    #[prop(optional)]
    on_approve: Option<Callback<String>>,
    /// Controlled mode: called when the user clicks approve; parent handles API.
    #[prop(optional)]
    on_confirm: Option<Callback<()>>,
    /// Whether this card has already been accepted (e.g. from parent state).
    #[prop(optional, default = false)]
    accepted: bool,
) -> impl IntoView {
    // Internal state for self-contained mode
    let (is_creating, set_is_creating) = signal(false);
    let (created, set_created) = signal(accepted);
    let (error, set_error) = signal::<Option<String>>(None);

    let is_controlled = on_confirm.is_some();

    // Clone config for closures
    let config = StoredValue::new(watch_config.clone());

    let watch_mode = watch_config.mode.clone().unwrap_or_else(|| "alert".to_string());
    let is_update = watch_config.watch_id.is_some();
    let name = watch_config.name.clone();
    let prompt = watch_config.prompt.clone();
    let schedule = watch_config.schedule.clone();
    let queries = watch_config.queries.clone();

    // Schedule description
    let schedule_for_display = schedule.clone();
    let schedule_desc = move || {
        let tz = get_tz_offset_minutes();
        describe_cron(&schedule_for_display, tz).description
    };

    // Handle approve click
    let handle_approve = move |_| {
        if is_controlled {
            // Controlled mode — let parent handle it
            if let Some(ref cb) = on_confirm {
                cb.run(());
            }
            return;
        }

        // Self-contained mode — call server functions directly
        set_is_creating.set(true);
        set_error.set(None);

        let cfg = config.get_value();
        let on_approve = on_approve.clone();

        leptos::task::spawn_local(async move {
            let queries_json = cfg.queries.as_ref().map(|q| {
                serde_json::to_string(q).unwrap_or_default()
            });

            let result = if let Some(ref watch_id) = cfg.watch_id {
                // Update existing watch
                crate::server_fns::watches::update_watch(
                    watch_id.clone(),
                    Some(cfg.name.clone()),
                    Some(cfg.prompt.clone()),
                    Some(cfg.schedule.clone()),
                    cfg.mode.clone(),
                    queries_json,
                    None, // slack_channel_id
                    None, // slack_channel_name
                    None, // alert_emails
                    None, // alert_emails_enabled
                )
                .await
                .map(|_| watch_id.clone())
            } else {
                // Create new watch
                crate::server_fns::watches::create_watch(
                    cfg.name.clone(),
                    cfg.prompt.clone(),
                    cfg.schedule.clone(),
                    cfg.mode.clone(),
                    queries_json,
                    None, // slack_channel_id
                    None, // slack_channel_name
                    None, // alert_emails
                    None, // alert_emails_enabled
                )
                .await
                .map(|item| item.watch_id)
            };

            match result {
                Ok(watch_id) => {
                    set_is_creating.set(false);
                    set_created.set(true);
                    if let Some(cb) = on_approve {
                        cb.run(watch_id);
                    }
                }
                Err(e) => {
                    let action = if cfg.watch_id.is_some() {
                        "update"
                    } else {
                        "create"
                    };
                    set_error.set(Some(format!(
                        "Failed to {action} watch: {}",
                        e.to_string()
                            .strip_prefix("error running server function: ")
                            .unwrap_or(&e.to_string())
                    )));
                    set_is_creating.set(false);
                }
            }
        });
    };

    // Mode badge
    let mode_badge = {
        let wm = watch_mode.clone();
        move || {
            if wm == "report" {
                view! {
                    <Badge variant=BadgeVariant::Default class="text-xs gap-1">
                        // ChartBarIcon SVG (Heroicons chart-bar outline)
                        <svg class="h-3 w-3" xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 24 24" stroke-width="1.5" stroke="currentColor">
                            <path stroke-linecap="round" stroke-linejoin="round" d="M3 13.125C3 12.504 3.504 12 4.125 12h2.25c.621 0 1.125.504 1.125 1.125v6.75C7.5 20.496 6.996 21 6.375 21h-2.25A1.125 1.125 0 0 1 3 19.875v-6.75ZM9.75 8.625c0-.621.504-1.125 1.125-1.125h2.25c.621 0 1.125.504 1.125 1.125v11.25c0 .621-.504 1.125-1.125 1.125h-2.25a1.125 1.125 0 0 1-1.125-1.125V8.625ZM16.5 4.125c0-.621.504-1.125 1.125-1.125h2.25C20.496 3 21 3.504 21 4.125v15.75c0 .621-.504 1.125-1.125 1.125h-2.25a1.125 1.125 0 0 1-1.125-1.125V4.125Z" />
                        </svg>
                        "Report"
                    </Badge>
                }.into_any()
            } else {
                view! {
                    <Badge variant=BadgeVariant::Warning class="text-xs gap-1">
                        // Bell icon (Lucide)
                        <Icon icon=icondata_lu::LuBell attr:class="h-3 w-3"/>
                        "Alert"
                    </Badge>
                }.into_any()
            }
        }
    };

    // Status badge
    let status_text = if is_update {
        "Update"
    } else {
        "New Watch"
    };

    // Queries section
    let queries_view = {
        let queries = queries.clone();
        move || {
            let qs = queries.clone();
            qs.filter(|q| !q.is_empty()).map(|qs| {
                let count = qs.len();
                let items = qs.into_iter().enumerate().map(|(_idx, q)| {
                    let comment = q.comment.clone().unwrap_or_default();
                    let sql = q.sql.clone();
                    let datasource = q.datasource.clone();
                    view! {
                        <div class="flex items-start gap-2 p-2 rounded bg-muted border border-border">
                            <span class="text-muted-foreground mt-0.5 shrink-0 text-[10px] w-4">{"\u{2699}\u{FE0F}"}</span>
                            <div class="flex-1 min-w-0">
                                <p class="text-xs font-medium text-foreground break-words">{comment}</p>
                                <p class="text-[10px] text-muted-foreground font-mono mt-1 truncate">{sql}</p>
                                {datasource.map(|ds| view! {
                                    <div class="mt-1">
                                        <span class="inline-block px-1.5 py-0.5 rounded text-[9px] bg-accent text-foreground">
                                            {ds}
                                        </span>
                                    </div>
                                })}
                            </div>
                        </div>
                    }
                }).collect_view();

                view! {
                    <div>
                        <p class="text-xs font-medium text-muted-foreground uppercase tracking-wider mb-2">
                            "Reference Queries (" {count} ")"
                        </p>
                        <div class="space-y-2 max-h-40 overflow-y-auto">
                            {items}
                        </div>
                    </div>
                }
            })
        }
    };

    view! {
        // React: Card className="border-primary/30 bg-primary/5 my-3"
        <Card class="border-primary/30 bg-primary/5 my-3">
            <CardHeader>
                // React: CardHeader className="pb-2" — override default padding
                // We set the inner flex layout here since CardHeader doesn't support custom class.
                <div class="flex items-center justify-between pb-2">
                    <div class="flex items-center gap-2">
                        // Eye icon (Lucide)
                        <Icon icon=icondata_lu::LuEye attr:class="h-4 w-4 text-primary"/>
                        <CardTitle class="text-base">"Watch Preview"</CardTitle>
                    </div>
                    <div class="flex items-center gap-2">
                        {mode_badge}
                        <Badge variant=BadgeVariant::Secondary class="text-xs">
                            {status_text}
                        </Badge>
                    </div>
                </div>
            </CardHeader>
            <CardContent class="space-y-3">
                // Name
                <div>
                    <p class="text-xs font-medium text-muted-foreground uppercase tracking-wider">"Name"</p>
                    <p class="font-medium">{name}</p>
                </div>

                // Monitoring Instruction
                <div>
                    <p class="text-xs font-medium text-muted-foreground uppercase tracking-wider">"Monitoring"</p>
                    <p class="text-sm text-foreground whitespace-pre-wrap">{prompt}</p>
                </div>

                // Queries
                {queries_view}

                // Schedule
                <div class="flex items-center gap-2 text-sm">
                    <Icon icon=icondata_lu::LuClock attr:class="h-4 w-4 text-muted-foreground"/>
                    <span>{schedule_desc}</span>
                </div>

                // Error message
                {move || error.get().map(|msg| view! {
                    <div class="text-sm text-error-foreground bg-error p-2 rounded">
                        {msg}
                    </div>
                })}

                // Approve button
                <div class="pt-2 border-t border-border">
                    <button
                        class=move || {
                            let base = "inline-flex items-center justify-center gap-2 whitespace-nowrap rounded-md text-sm font-medium transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:pointer-events-none disabled:opacity-50 [&_svg]:pointer-events-none [&_svg]:size-4 [&_svg]:shrink-0 h-8 rounded-md px-3 text-xs w-full";
                            if created.get() {
                                // secondary variant
                                format!("{base} bg-secondary text-secondary-foreground shadow-sm hover:bg-secondary/80")
                            } else {
                                // default variant
                                format!("{base} bg-primary text-primary-foreground shadow hover:bg-primary/90")
                            }
                        }
                        disabled=move || is_creating.get() || created.get()
                        on:click=handle_approve
                    >
                        {move || {
                            if is_creating.get() {
                                view! {
                                    <Spinner class="mr-2"/>
                                    "Accepting..."
                                }.into_any()
                            } else if created.get() {
                                view! {
                                    // CheckCircle icon (Lucide)
                                    <Icon icon=icondata_lu::LuCircleCheck attr:class="h-4 w-4 mr-2"/>
                                    "Accepted"
                                }.into_any()
                            } else {
                                view! {
                                    <Icon icon=icondata_lu::LuCircleCheck attr:class="h-4 w-4 mr-2"/>
                                    "Accept"
                                }.into_any()
                            }
                        }}
                    </button>
                    {move || (!created.get()).then(|| view! {
                        <p class="text-xs text-center text-muted-foreground mt-2">
                            "Or continue chatting to refine"
                        </p>
                    })}
                </div>
            </CardContent>
        </Card>
    }
}
