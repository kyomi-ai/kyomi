// SPDX-License-Identifier: AGPL-3.0-or-later

//! WatchModal component — matches
//! `apps/frontend/src/components/watches/WatchModal.jsx` exactly.
//!
//! Modal dialog for editing an existing watch. Includes form sections for:
//! name, mode toggle (alert/report), prompt/instructions, reference queries
//! (with inline edit), schedule selector, Slack notifications, and email
//! notifications.

use leptos::prelude::*;
use leptos_icons::Icon;

use crate::components::toast::{toast_error, toast_success};
use crate::components::{
    DynSelect, Label, Modal, ModalSize, Spinner, Switch,
    INPUT_CLASS,
};
use crate::types::WatchListItem;

use super::ScheduleSelector;

#[cfg(feature = "slack")]
use crate::server_fns::slack::SlackChannel;

// ---------------------------------------------------------------------------
// Button CSS constants (from button.rs / schedule_selector.rs) for raw
// <button> elements that need reactive variant switching.
// ---------------------------------------------------------------------------

const BTN_BASE: &str = "inline-flex items-center justify-center gap-2 whitespace-nowrap rounded-md text-sm font-medium transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:pointer-events-none disabled:opacity-50 [&_svg]:pointer-events-none [&_svg]:size-4 [&_svg]:shrink-0";
const BTN_OUTLINE: &str =
    "border border-input bg-background text-foreground shadow-sm hover:bg-accent hover:text-accent-foreground";
const BTN_GHOST: &str = "text-foreground hover:bg-accent hover:text-accent-foreground";
const BTN_SM: &str = "h-8 rounded-md px-3 text-xs";

// ---------------------------------------------------------------------------
// Form types
// ---------------------------------------------------------------------------

/// Internal form state for the watch modal.
#[derive(Clone, Debug)]
struct WatchQueryForm {
    comment: String,
    sql: String,
    datasource: Option<String>,
}

// ---------------------------------------------------------------------------
// Component
// ---------------------------------------------------------------------------

/// Modal for editing an existing watch.
///
/// React reference: `apps/frontend/src/components/watches/WatchModal.jsx`
///
/// Note: Watch creation is done through chat (AI-guided). This modal is only
/// for editing existing watches.
#[component]
pub fn WatchModal(
    /// The watch to edit.
    watch: WatchListItem,
    /// Whether the modal is open (reactive).
    #[prop(into)]
    open: Signal<bool>,
    /// Called when the modal should close.
    on_close: Callback<()>,
    /// Called after a successful save.
    on_saved: Callback<()>,
) -> impl IntoView {
    // ── Form signals ────────────────────────────────────────────────────
    let (name, set_name) = signal(watch.name.clone());
    let (prompt, set_prompt) = signal(watch.prompt.clone());
    let (schedule, set_schedule) = signal(watch.schedule.clone());
    let (mode, set_mode) = signal(watch.mode.clone());
    let (slack_channel_id, set_slack_channel_id) =
        signal(watch.slack_channel_id.clone().unwrap_or_default());
    let (alert_emails, set_alert_emails) =
        signal(watch.alert_emails.clone().unwrap_or_default());
    let (alert_emails_enabled, set_alert_emails_enabled) = signal(watch.alert_emails_enabled);

    // Queries — parse from serde_json::Value
    let initial_queries: Vec<WatchQueryForm> = watch
        .queries
        .as_ref()
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .map(|q| WatchQueryForm {
                    comment: q
                        .get("comment")
                        .and_then(|c| c.as_str())
                        .unwrap_or("")
                        .to_string(),
                    sql: q
                        .get("sql")
                        .and_then(|c| c.as_str())
                        .unwrap_or("")
                        .to_string(),
                    datasource: q
                        .get("datasource")
                        .and_then(|c| c.as_str())
                        .map(|s| s.to_string()),
                })
                .collect()
        })
        .unwrap_or_default();

    let (queries, set_queries) = signal(initial_queries);

    // UI state — which query is being edited (None = none)
    let (editing_query_idx, set_editing_query_idx) = signal::<Option<usize>>(None);

    // Save pending state
    let (is_saving, set_is_saving) = signal(false);

    // Watch ID for the update call
    let watch_id = watch.watch_id.clone();

    // ── Datasources resource ─────────────────────────────────────────────
    let datasources_resource = Resource::new(
        move || open.get(),
        move |is_open| async move {
            if !is_open {
                return Vec::new();
            }
            crate::server_fns::datasources::list_datasources()
                .await
                .unwrap_or_default()
        },
    );

    // ── Schedule change callback ────────────────────────────────────────
    let on_schedule_change = Callback::new(move |new_schedule: String| {
        set_schedule.set(new_schedule);
    });

    // ── Submit handler ──────────────────────────────────────────────────
    let watch_id_for_submit = watch_id.clone();
    let handle_submit = move || {
        // Validation
        let name_val = name.get_untracked();
        let prompt_val = prompt.get_untracked();

        if name_val.trim().len() < 3 {
            toast_error("Name must be at least 3 characters");
            return;
        }
        if prompt_val.trim().len() < 10 {
            toast_error("Monitoring instruction must be at least 10 characters");
            return;
        }

        set_is_saving.set(true);

        let watch_id = watch_id_for_submit.clone();
        let schedule_val = schedule.get_untracked();
        let mode_val = mode.get_untracked();
        let slack_val = slack_channel_id.get_untracked();
        let emails_val = alert_emails.get_untracked();
        let emails_enabled = alert_emails_enabled.get_untracked();
        let queries_val = queries.get_untracked();

        // Serialize queries to JSON string for the server function
        let queries_json = {
            let filtered: Vec<_> = queries_val
                .iter()
                .filter(|q| !q.sql.trim().is_empty())
                .map(|q| {
                    serde_json::json!({
                        "comment": q.comment,
                        "sql": q.sql,
                        "datasource": q.datasource,
                    })
                })
                .collect();
            if filtered.is_empty() {
                None
            } else {
                Some(serde_json::to_string(&filtered).unwrap_or_default())
            }
        };

        let slack_channel_id_param = if slack_val.is_empty() {
            Some(String::new()) // empty string = remove channel
        } else {
            Some(slack_val)
        };

        let on_saved = on_saved;

        leptos::task::spawn_local(async move {
            let result = crate::server_fns::watches::update_watch(
                watch_id,
                crate::server_fns::watches::WatchConfig {
                    name: name_val.trim().to_string(),
                    prompt: prompt_val.trim().to_string(),
                    schedule: schedule_val,
                    mode: Some(mode_val),
                    queries: queries_json,
                    slack_channel_id: slack_channel_id_param,
                    slack_channel_name: None, // resolved server-side
                    alert_emails: Some(emails_val.trim().to_string()),
                    alert_emails_enabled: Some(emails_enabled),
                },
            )
            .await;

            match result {
                Ok(()) => {
                    set_is_saving.set(false);
                    toast_success("Watch updated successfully");
                    on_saved.run(());
                }
                Err(e) => {
                    let msg = e
                        .to_string()
                        .strip_prefix("error running server function: ")
                        .unwrap_or(&e.to_string())
                        .to_string();
                    toast_error(msg);
                    set_is_saving.set(false);
                }
            }
        });
    };

    let handle_submit_form = StoredValue::new(handle_submit.clone());

    // ── Footer ──────────────────────────────────────────────────────────
    let footer_view = {
        let handle_submit = handle_submit.clone();
        ChildrenFn::to_children(move || {
            let handle_submit = handle_submit.clone();
            view! {
                <button
                    type="button"
                    class=format!("{BTN_BASE} {BTN_GHOST} {BTN_SM}")
                    disabled=move || is_saving.get()
                    on:click=move |_| on_close.run(())
                >
                    "Cancel"
                </button>
                <button
                    type="button"
                    class=format!("{BTN_BASE} bg-primary text-primary-foreground shadow hover:bg-primary/90 {BTN_SM}")
                    disabled=move || is_saving.get()
                    on:click=move |_| handle_submit()
                >
                    {move || {
                        if is_saving.get() {
                            view! {
                                <Spinner class="mr-2"/>
                                "Saving..."
                            }.into_any()
                        } else {
                            view! { "Save Changes" }.into_any()
                        }
                    }}
                </button>
            }
            .into_any()
        })
    };

    view! {
        <Modal
            show=open
            on_close=on_close
            title="Edit Watch"
            size=ModalSize::Lg
            footer=footer_view
        >
            <form
                class="space-y-5"
                on:submit=move |ev: web_sys::SubmitEvent| {
                    ev.prevent_default();
                    handle_submit_form.with_value(|f| f());
                }
            >
                // ── Name ────────────────────────────────────────────────
                <div class="space-y-2">
                    <Label html_for="watch-name">"Name"</Label>
                    <input
                        id="watch-name"
                        type="text"
                        class=INPUT_CLASS
                        prop:value=move || name.get()
                        on:input=move |ev| set_name.set(event_target_value(&ev))
                        placeholder="Daily Sales Monitor"
                    />
                    <p class="text-xs text-muted-foreground">
                        "A short, descriptive name for this watch"
                    </p>
                </div>

                // ── Mode Toggle ─────────────────────────────────────────
                <div class="space-y-2">
                    <Label>"Mode"</Label>
                    <div class="flex flex-col sm:flex-row gap-3 sm:gap-4">
                        // Alert button
                        <button
                            type="button"
                            class=move || {
                                let base = "flex-1 p-3 rounded-lg border text-left transition-colors";
                                if mode.get() == "alert" {
                                    format!("{base} border-primary bg-primary/5")
                                } else {
                                    format!("{base} border-border hover:border-muted-foreground/50")
                                }
                            }
                            on:click=move |_| set_mode.set("alert".to_string())
                        >
                            <div class="flex items-center gap-2 mb-1">
                                // BellIcon (Heroicons outline)
                                <svg class="h-5 w-5" xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 24 24" stroke-width="1.5" stroke="currentColor">
                                    <path stroke-linecap="round" stroke-linejoin="round" d="M14.857 17.082a23.848 23.848 0 0 0 5.454-1.31A8.967 8.967 0 0 1 18 9.75V9A6 6 0 0 0 6 9v.75a8.967 8.967 0 0 1-2.312 6.022c1.733.64 3.56 1.085 5.455 1.31m5.714 0a24.255 24.255 0 0 1-5.714 0m5.714 0a3 3 0 1 1-5.714 0" />
                                </svg>
                                <span class=move || {
                                    if mode.get() == "alert" {
                                        "font-medium text-primary"
                                    } else {
                                        "font-medium"
                                    }
                                }>
                                    "Alert"
                                </span>
                            </div>
                            <p class="text-xs text-muted-foreground">
                                "Agent decides when to notify you based on conditions"
                            </p>
                        </button>
                        // Report button
                        <button
                            type="button"
                            class=move || {
                                let base = "flex-1 p-3 rounded-lg border text-left transition-colors";
                                if mode.get() == "report" {
                                    format!("{base} border-primary bg-primary/5")
                                } else {
                                    format!("{base} border-border hover:border-muted-foreground/50")
                                }
                            }
                            on:click=move |_| set_mode.set("report".to_string())
                        >
                            <div class="flex items-center gap-2 mb-1">
                                // ChartBarIcon (Heroicons outline)
                                <svg class="h-5 w-5" xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 24 24" stroke-width="1.5" stroke="currentColor">
                                    <path stroke-linecap="round" stroke-linejoin="round" d="M3 13.125C3 12.504 3.504 12 4.125 12h2.25c.621 0 1.125.504 1.125 1.125v6.75C7.5 20.496 6.996 21 6.375 21h-2.25A1.125 1.125 0 0 1 3 19.875v-6.75ZM9.75 8.625c0-.621.504-1.125 1.125-1.125h2.25c.621 0 1.125.504 1.125 1.125v11.25c0 .621-.504 1.125-1.125 1.125h-2.25a1.125 1.125 0 0 1-1.125-1.125V8.625ZM16.5 4.125c0-.621.504-1.125 1.125-1.125h2.25C20.496 3 21 3.504 21 4.125v15.75c0 .621-.504 1.125-1.125 1.125h-2.25a1.125 1.125 0 0 1-1.125-1.125V4.125Z" />
                                </svg>
                                <span class=move || {
                                    if mode.get() == "report" {
                                        "font-medium text-primary"
                                    } else {
                                        "font-medium"
                                    }
                                }>
                                    "Report"
                                </span>
                            </div>
                            <p class="text-xs text-muted-foreground">
                                "Always sends a summary on schedule, no conditions"
                            </p>
                        </button>
                    </div>
                </div>

                // ── Prompt / Instructions ───────────────────────────────
                <div class="space-y-2">
                    <Label html_for="watch-prompt">
                        {move || {
                            if mode.get() == "report" {
                                "Report Instructions"
                            } else {
                                "Monitoring Instructions"
                            }
                        }}
                    </Label>
                    <textarea
                        id="watch-prompt"
                        prop:value=move || prompt.get()
                        on:input=move |ev| set_prompt.set(event_target_value(&ev))
                        placeholder=move || {
                            if mode.get() == "report" {
                                "Summarize our daily sales revenue. Include key metrics, trends, and any notable observations."
                            } else {
                                "Check our daily sales revenue. Alert me if it drops more than 10% compared to the same day last week, or if there are any unusual patterns."
                            }
                        }
                        rows=6
                        class="flex w-full rounded-md border border-input bg-transparent px-3 py-2 text-sm shadow-sm placeholder:text-muted-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:cursor-not-allowed disabled:opacity-50 resize-y min-h-[120px]"
                    />
                    <p class="text-xs text-muted-foreground">
                        {move || {
                            if mode.get() == "report" {
                                "Describe what data to include in the scheduled report. Be specific about metrics and format."
                            } else {
                                "Describe what to monitor and when to alert you. Be specific about thresholds or conditions."
                            }
                        }}
                    </p>
                </div>

                // ── Reference Queries ───────────────────────────────────
                <div class="space-y-3">
                    <Label>
                        <span class="flex items-center gap-2">
                            <Icon icon=icondata_lu::LuCode attr:class="h-4 w-4"/>
                            "Reference Queries"
                        </span>
                    </Label>
                    <p class="text-xs text-muted-foreground">
                        {move || {
                            if mode.get() == "report" {
                                "These queries serve as reference for the report generation."
                            } else {
                                "These queries serve as reference for the monitoring agent."
                            }
                        }}
                    </p>

                    {move || {
                        let qs = queries.get();
                        if qs.is_empty() {
                            view! {
                                <div class="text-sm text-muted-foreground italic p-3 bg-muted/50 rounded-lg">
                                    "No reference queries configured"
                                </div>
                            }.into_any()
                        } else {
                            let items = qs.into_iter().enumerate().map(|(idx, query)| {
                                let editing_idx = editing_query_idx.get();
                                if editing_idx == Some(idx) {
                                    // Edit mode for this query
                                    view! {
                                        <div class="border border-border rounded-lg p-4 space-y-3 bg-muted/20">
                                            <div class="flex items-start gap-2">
                                                <div class="flex-1 space-y-2">
                                                    <label class="text-sm font-medium leading-none text-xs">"Query Title"</label>
                                                    <input
                                                        type="text"
                                                        class=format!("{INPUT_CLASS} text-sm")
                                                        prop:value=query.comment.clone()
                                                        on:input=move |ev| {
                                                            set_queries.update(|qs| {
                                                                if let Some(q) = qs.get_mut(idx) {
                                                                    q.comment = event_target_value(&ev);
                                                                }
                                                            });
                                                        }
                                                        placeholder="e.g., Daily Revenue Trend"
                                                    />
                                                </div>
                                                // Delete button
                                                <button
                                                    type="button"
                                                    class=format!("{BTN_BASE} {BTN_GHOST} h-9 w-9 mt-6")
                                                    on:click=move |_| {
                                                        set_queries.update(|qs| {
                                                            qs.remove(idx);
                                                        });
                                                        set_editing_query_idx.set(None);
                                                    }
                                                >
                                                    // Trash2 icon (Lucide)
                                                    <Icon icon=icondata_lu::LuTrash2 attr:class="h-4 w-4 text-destructive"/>
                                                </button>
                                            </div>

                                            <div class="space-y-2">
                                                <label class="text-sm font-medium leading-none text-xs">"SQL Query"</label>
                                                <textarea
                                                    prop:value=query.sql.clone()
                                                    on:input=move |ev| {
                                                        set_queries.update(|qs| {
                                                            if let Some(q) = qs.get_mut(idx) {
                                                                q.sql = event_target_value(&ev);
                                                            }
                                                        });
                                                    }
                                                    placeholder="SELECT ..."
                                                    rows=4
                                                    class="w-full font-mono text-xs p-2 border border-input rounded-md bg-background"
                                                />
                                            </div>

                                            // Datasource selector
                                            <div class="space-y-2">
                                                <label class="text-sm font-medium leading-none text-xs">"Datasource (Optional)"</label>
                                                <DynSelect
                                                    value=Signal::derive(move || {
                                                        queries.with(|qs| {
                                                            qs.get(idx)
                                                                .and_then(|q| q.datasource.clone())
                                                                .unwrap_or_else(|| "none".to_string())
                                                        })
                                                    })
                                                    options=Signal::derive(move || {
                                                        let mut opts = vec![("none".to_string(), "None".to_string())];
                                                        if let Some(ds_list) = datasources_resource.get() {
                                                            for ds in ds_list {
                                                                opts.push((ds.slug.clone(), ds.name.clone()));
                                                            }
                                                        }
                                                        opts
                                                    })
                                                    on_change=move |val: String| {
                                                        set_queries.update(|qs| {
                                                            if let Some(q) = qs.get_mut(idx) {
                                                                q.datasource = if val == "none" { None } else { Some(val) };
                                                            }
                                                        });
                                                    }
                                                    placeholder="Select a datasource"
                                                />
                                            </div>

                                            <div class="flex gap-2 pt-2 border-t border-border">
                                                <button
                                                    type="button"
                                                    class=format!("{BTN_BASE} {BTN_OUTLINE} {BTN_SM}")
                                                    on:click=move |_| set_editing_query_idx.set(None)
                                                >
                                                    "Done"
                                                </button>
                                            </div>
                                        </div>
                                    }.into_any()
                                } else {
                                    // Read-only view: compact block
                                    let comment = query.comment.clone();
                                    let sql = query.sql.clone();
                                    let datasource = query.datasource.clone();
                                    view! {
                                        <div class="flex items-start gap-3 p-3 rounded-lg border border-border bg-muted/30 hover:bg-muted/50 transition-colors group">
                                            <Icon icon=icondata_lu::LuCode attr:class="h-4 w-4 text-muted-foreground mt-0.5 shrink-0"/>
                                            <div class="flex-1 min-w-0">
                                                <p class="font-medium text-sm text-foreground break-words">{comment}</p>
                                                <p class="text-xs text-muted-foreground font-mono mt-1 truncate">{sql}</p>
                                                {datasource.map(|ds| view! {
                                                    <div class="mt-2">
                                                        <span class="inline-block px-2 py-1 rounded-md text-xs bg-secondary text-secondary-foreground">
                                                            {ds}
                                                        </span>
                                                    </div>
                                                })}
                                            </div>
                                            // Edit button (pencil)
                                            <button
                                                type="button"
                                                class=format!("{BTN_BASE} {BTN_GHOST} h-8 w-8 shrink-0 sm:opacity-0 sm:group-hover:opacity-100 transition-opacity")
                                                on:click=move |_| set_editing_query_idx.set(Some(idx))
                                            >
                                                // PencilIcon (Heroicons outline)
                                                <svg class="h-4 w-4" xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 24 24" stroke-width="1.5" stroke="currentColor">
                                                    <path stroke-linecap="round" stroke-linejoin="round" d="m16.862 4.487 1.687-1.688a1.875 1.875 0 1 1 2.652 2.652L6.832 19.82a4.5 4.5 0 0 1-1.897 1.13l-2.685.8.8-2.685a4.5 4.5 0 0 1 1.13-1.897L16.863 4.487Zm0 0L19.5 7.125" />
                                                </svg>
                                            </button>
                                        </div>
                                    }.into_any()
                                }
                            }).collect_view();

                            view! {
                                <div class="space-y-2">
                                    {items}
                                </div>
                            }.into_any()
                        }
                    }}

                    // Add Query button
                    <button
                        type="button"
                        class=format!("{BTN_BASE} {BTN_OUTLINE} {BTN_SM}")
                        on:click=move |_| {
                            set_queries.update(|qs| {
                                qs.push(WatchQueryForm {
                                    comment: String::new(),
                                    sql: String::new(),
                                    datasource: None,
                                });
                            });
                        }
                    >
                        <Icon icon=icondata_lu::LuPlus attr:class="h-4 w-4 mr-1"/>
                        "Add Query"
                    </button>
                </div>

                // ── Schedule ────────────────────────────────────────────
                <ScheduleSelector
                    value=Signal::derive(move || schedule.get())
                    on_change=on_schedule_change
                />

                // ── Slack Notifications ─────────────────────────────────
                <SlackNotificationsSection
                    open=open
                    mode=Signal::derive(move || mode.get())
                    slack_channel_id=Signal::derive(move || slack_channel_id.get())
                    set_slack_channel_id=set_slack_channel_id
                />

                // ── Email Notifications ─────────────────────────────────
                <div class="space-y-3">
                    <div class="flex items-center justify-between">
                        <Label html_for="alert-emails-toggle">
                            <span class="flex items-center gap-2">
                                <Icon icon=icondata_lu::LuMail attr:class="h-4 w-4"/>
                                "Email Notifications"
                            </span>
                        </Label>
                        <div class="flex items-center gap-2">
                            <span class="text-xs text-muted-foreground">
                                {move || if alert_emails_enabled.get() { "Enabled" } else { "Disabled" }}
                            </span>
                            <Switch
                                checked=Signal::derive(move || alert_emails_enabled.get())
                                on_change=Callback::new(move |val: bool| set_alert_emails_enabled.set(val))
                            />
                        </div>
                    </div>
                    <input
                        id="alert-emails"
                        type="text"
                        class=move || {
                            if alert_emails_enabled.get() {
                                INPUT_CLASS.to_string()
                            } else {
                                format!("{INPUT_CLASS} opacity-50")
                            }
                        }
                        prop:value=move || alert_emails.get()
                        on:input=move |ev| set_alert_emails.set(event_target_value(&ev))
                        placeholder="your@email.com, colleague@email.com"
                        disabled=move || !alert_emails_enabled.get()
                    />
                    <p class="text-xs text-muted-foreground">
                        {move || {
                            if alert_emails_enabled.get() {
                                if mode.get() == "report" {
                                    "Comma-separated email addresses to receive reports."
                                } else {
                                    "Comma-separated email addresses to receive alerts."
                                }
                            } else {
                                "Enable email notifications to configure recipients."
                            }
                        }}
                    </p>
                </div>

                // ── Help text ───────────────────────────────────────────
                <div class="rounded-lg bg-muted/50 p-4 text-sm text-muted-foreground">
                    <p class="font-medium text-foreground mb-2">"How it works"</p>
                    {move || {
                        if mode.get() == "report" {
                            view! {
                                <ul class="space-y-1 text-xs">
                                    <li>"The AI will analyze your data based on your instructions"</li>
                                    <li>"A report summary will be sent on every scheduled run"</li>
                                    <li>"You can view all reports in the Alerts tab"</li>
                                </ul>
                            }.into_any()
                        } else {
                            view! {
                                <ul class="space-y-1 text-xs">
                                    <li>"The AI will analyze your data based on your instructions"</li>
                                    <li>"If something noteworthy is found, you will receive an alert"</li>
                                    <li>"You can view all alerts in the Alerts tab"</li>
                                </ul>
                            }.into_any()
                        }
                    }}
                </div>
            </form>
        </Modal>
    }
}

// ---------------------------------------------------------------------------
// Slack Notifications sub-component — feature-gated
// ---------------------------------------------------------------------------

/// Slack notifications section when the `slack` feature is enabled.
///
/// Fetches Slack status and channel list, then renders the appropriate UI
/// state: not installed, user not connected, loading, no channels, or the
/// channel selector dropdown. Matches the React WatchModal.jsx Slack section.
#[cfg(feature = "slack")]
#[component]
fn SlackNotificationsSection(
    /// Whether the parent modal is open (drives resource fetches).
    #[prop(into)]
    open: Signal<bool>,
    /// Current watch mode ("alert" or "report").
    #[prop(into)]
    mode: Signal<String>,
    /// Currently selected Slack channel ID.
    #[prop(into)]
    slack_channel_id: Signal<String>,
    /// Setter for the selected Slack channel ID.
    set_slack_channel_id: WriteSignal<String>,
) -> impl IntoView {
    let slack_status_resource = Resource::new(
        move || open.get(),
        move |is_open| async move {
            if !is_open {
                return None;
            }
            crate::server_fns::slack::get_slack_status().await.ok()
        },
    );

    let slack_channels_resource = Resource::new(
        move || {
            let status = slack_status_resource.get().flatten();
            matches!(status, Some(ref s) if s.workspace_connected && s.user_connected)
        },
        move |should_fetch| async move {
            if !should_fetch {
                return Vec::<SlackChannel>::new();
            }
            crate::server_fns::slack::get_slack_channels()
                .await
                .unwrap_or_default()
        },
    );

    view! {
        <div class="space-y-2">
            <Label>
                <span class="flex items-center gap-2">
                    <Icon icon=icondata_lu::LuMessageSquare attr:class="h-4 w-4"/>
                    "Slack Notifications"
                </span>
            </Label>

            {move || {
                let status = slack_status_resource.get().flatten();
                match status {
                    // Still loading status
                    None => view! {
                        <div class="flex items-center gap-2 text-sm text-muted-foreground">
                            <Spinner />
                            <span>"Checking Slack status..."</span>
                        </div>
                    }.into_any(),

                    Some(ref status) if !status.workspace_connected => view! {
                        // Slack app not installed in workspace
                        <div class="flex items-start gap-2 p-3 bg-muted/50 rounded-lg text-sm text-muted-foreground">
                            <Icon icon=icondata_lu::LuTriangleAlert attr:class="h-4 w-4 text-warning-foreground mt-0.5 shrink-0"/>
                            <span>
                                "Slack is not installed. Ask your workspace admin to connect Slack in Settings."
                            </span>
                        </div>
                    }.into_any(),

                    Some(ref status) if !status.user_connected => {
                        let label = if mode.get() == "report" { "reports" } else { "alerts" };
                        view! {
                            // User not connected to Slack
                            <div class="flex items-start gap-2 p-3 bg-muted/50 rounded-lg text-sm text-muted-foreground">
                                <Icon icon=icondata_lu::LuTriangleAlert attr:class="h-4 w-4 text-warning-foreground mt-0.5 shrink-0"/>
                                <span>
                                    {format!("Connect your Slack account in Profile Settings to send {label} to Slack.")}
                                </span>
                            </div>
                        }.into_any()
                    },

                    Some(_) => {
                        // User is connected — show channel selector or loading/empty state
                        let channels = slack_channels_resource.get();
                        match channels {
                            None => view! {
                                <div class="flex items-center gap-2 text-sm text-muted-foreground">
                                    <Spinner />
                                    <span>"Loading channels..."</span>
                                </div>
                            }.into_any(),

                            Some(ref ch) if ch.is_empty() => view! {
                                <div class="flex items-start gap-2 p-3 bg-muted/50 rounded-lg text-sm text-muted-foreground">
                                    <Icon icon=icondata_lu::LuTriangleAlert attr:class="h-4 w-4 text-warning-foreground mt-0.5 shrink-0"/>
                                    <span>
                                        "Invite the Kyomi app to a Slack channel first. Then refresh this page to see available channels."
                                    </span>
                                </div>
                            }.into_any(),

                            Some(ref channels) => {
                                let channel_options: Vec<(String, String)> = {
                                    let mut opts = vec![("none".to_string(), "None (no Slack notifications)".to_string())];
                                    for ch in channels {
                                        opts.push((
                                            ch.channel_id.clone(),
                                            format!("#{}", ch.channel_name),
                                        ));
                                    }
                                    opts
                                };
                                let current_val = slack_channel_id.get();
                                let select_val = if current_val.is_empty() {
                                    "none".to_string()
                                } else {
                                    current_val
                                };
                                let (channel_opts_sig, _) = signal(channel_options);
                                let (select_val_sig, _) = signal(select_val);
                                let mode_label = if mode.get() == "report" { "Reports" } else { "Alerts" };
                                view! {
                                    <DynSelect
                                        value=Signal::derive(move || select_val_sig.get())
                                        options=Signal::derive(move || channel_opts_sig.get())
                                        on_change=move |val: String| {
                                            if val == "none" {
                                                set_slack_channel_id.set(String::new());
                                            } else {
                                                set_slack_channel_id.set(val);
                                            }
                                        }
                                        placeholder="Select a channel"
                                    />
                                    <p class="text-xs text-muted-foreground">
                                        {format!("{mode_label} will be posted to this channel as Kyomi.")}
                                    </p>
                                }.into_any()
                            }
                        }
                    }
                }
            }}
        </div>
    }
}

/// Slack notifications section when the `slack` feature is NOT enabled.
///
/// Shows the static "not installed" message since Slack functions are
/// unavailable without the feature flag.
#[cfg(not(feature = "slack"))]
#[component]
fn SlackNotificationsSection(
    #[prop(into)]
    open: Signal<bool>,
    #[prop(into)]
    mode: Signal<String>,
    #[prop(into)]
    slack_channel_id: Signal<String>,
    set_slack_channel_id: WriteSignal<String>,
) -> impl IntoView {
    let _ = (open, mode, slack_channel_id, set_slack_channel_id);
    view! {
        <div class="space-y-2">
            <Label>
                <span class="flex items-center gap-2">
                    <Icon icon=icondata_lu::LuMessageSquare attr:class="h-4 w-4"/>
                    "Slack Notifications"
                </span>
            </Label>
            <div class="flex items-start gap-2 p-3 bg-muted/50 rounded-lg text-sm text-muted-foreground">
                <Icon icon=icondata_lu::LuTriangleAlert attr:class="h-4 w-4 text-warning-foreground mt-0.5 shrink-0"/>
                <span>
                    "Slack is not installed. Ask your workspace admin to connect Slack in Settings."
                </span>
            </div>
        </div>
    }
}
