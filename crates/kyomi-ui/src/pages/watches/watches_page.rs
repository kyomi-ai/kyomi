// SPDX-License-Identifier: AGPL-3.0-or-later

//! WatchesPage — main page with two tabs: Alerts and Watches.
//!
//! Ported from `apps/frontend/src/pages/WatchesPage.jsx` (667 lines).
//!
//! Features:
//! - Header with "Kyomi Watch" title and "Create Watch" button
//! - Two tabs: Alerts (inbox) and Watches (config)
//! - Alerts tab shows `AlertsHistory` component
//! - Watches tab shows watch cards with toggle, run, edit, delete actions
//! - WatchAgentSidebar slides in for AI-powered watch creation/editing
//! - WatchModal opens for direct (quick) edit of an existing watch
//! - Execution log modal for viewing watch run history

use leptos::prelude::*;
use leptos_router::hooks::{use_navigate, use_params_map, use_query_map};

use crate::components::toast::{toast_error, toast_success};
use crate::components::watches::{AlertsHistory, ExecutionLogViewer, WatchAgentSidebar, WatchModal};
use crate::components::{
    Alert, AlertDescription, AlertVariant, Card, CardContent, CardHeader, CardTitle, ConfirmDialog,
    Modal, ModalSize, Spinner, StatusBadge, StatusBadgeVariant, Switch,
};
use crate::server_fns::context::get_user_context;
use crate::server_fns::watches::{
    delete_watch, get_watch_execution, get_watch_executions, list_watches, run_watch_now,
    toggle_watch,
};
use crate::types::WatchListItem;
use crate::utils::cron::{describe_cron, get_tz_offset_minutes};

// ─── Button CSS constants ───────────────────────────────────────────────────
// From button.rs — used for raw <button> elements that need click handlers.

const BTN_BASE: &str = "inline-flex items-center justify-center gap-2 whitespace-nowrap rounded-md text-sm font-medium transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:pointer-events-none disabled:opacity-50 [&_svg]:pointer-events-none [&_svg]:size-4 [&_svg]:shrink-0";
const BTN_DEFAULT: &str = "bg-primary text-primary-foreground shadow hover:bg-primary/90 transition-colors";
const BTN_GHOST: &str = "text-foreground hover:bg-accent hover:text-accent-foreground transition-colors";
const BTN_DEFAULT_SIZE: &str = "h-9 px-4 py-2";
const BTN_SM: &str = "h-8 rounded-md px-3 text-xs";

// ─── SVG Icons ──────────────────────────────────────────────────────────────

/// Eye icon (Heroicons outline).
#[component]
fn EyeIcon(#[prop(into, optional)] class: String) -> impl IntoView {
    view! {
        <svg class=class xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 24 24" stroke-width="1.5" stroke="currentColor">
            <path stroke-linecap="round" stroke-linejoin="round" d="M2.036 12.322a1.012 1.012 0 0 1 0-.639C3.423 7.51 7.36 4.5 12 4.5c4.638 0 8.573 3.007 9.963 7.178.07.207.07.431 0 .639C20.577 16.49 16.64 19.5 12 19.5c-4.638 0-8.573-3.007-9.963-7.178Z" />
            <path stroke-linecap="round" stroke-linejoin="round" d="M15 12a3 3 0 1 1-6 0 3 3 0 0 1 6 0Z" />
        </svg>
    }
}

/// Plus icon (Lucide).
#[component]
fn PlusIcon(#[prop(into, optional)] class: String) -> impl IntoView {
    view! {
        <svg class=class xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <path d="M5 12h14" />
            <path d="M12 5v14" />
        </svg>
    }
}

/// Bell icon (Lucide).
#[component]
fn BellIcon(#[prop(into, optional)] class: String) -> impl IntoView {
    view! {
        <svg class=class xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <path d="M6 8a6 6 0 0 1 12 0c0 7 3 9 3 9H3s3-2 3-9" />
            <path d="M10.3 21a1.94 1.94 0 0 0 3.4 0" />
        </svg>
    }
}

/// Clock icon (Lucide).
#[component]
fn ClockIcon(#[prop(into, optional)] class: String) -> impl IntoView {
    view! {
        <svg class=class xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <circle cx="12" cy="12" r="10" />
            <polyline points="12 6 12 12 16 14" />
        </svg>
    }
}

/// Play icon (Lucide).
#[component]
fn PlayIcon(#[prop(into, optional)] class: String) -> impl IntoView {
    view! {
        <svg class=class xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <polygon points="6 3 20 12 6 21 6 3" />
        </svg>
    }
}

/// Settings icon (Lucide).
#[component]
fn SettingsIcon(#[prop(into, optional)] class: String) -> impl IntoView {
    view! {
        <svg class=class xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <path d="M12.22 2h-.44a2 2 0 0 0-2 2v.18a2 2 0 0 1-1 1.73l-.43.25a2 2 0 0 1-2 0l-.15-.08a2 2 0 0 0-2.73.73l-.22.38a2 2 0 0 0 .73 2.73l.15.1a2 2 0 0 1 1 1.72v.51a2 2 0 0 1-1 1.74l-.15.09a2 2 0 0 0-.73 2.73l.22.38a2 2 0 0 0 2.73.73l.15-.08a2 2 0 0 1 2 0l.43.25a2 2 0 0 1 1 1.73V20a2 2 0 0 0 2 2h.44a2 2 0 0 0 2-2v-.18a2 2 0 0 1 1-1.73l.43-.25a2 2 0 0 1 2 0l.15.08a2 2 0 0 0 2.73-.73l.22-.39a2 2 0 0 0-.73-2.73l-.15-.08a2 2 0 0 1-1-1.74v-.5a2 2 0 0 1 1-1.74l.15-.09a2 2 0 0 0 .73-2.73l-.22-.38a2 2 0 0 0-2.73-.73l-.15.08a2 2 0 0 1-2 0l-.43-.25a2 2 0 0 1-1-1.73V4a2 2 0 0 0-2-2z" />
            <circle cx="12" cy="12" r="3" />
        </svg>
    }
}

/// Trash2 icon (Lucide).
#[component]
fn TrashIcon(#[prop(into, optional)] class: String) -> impl IntoView {
    view! {
        <svg class=class xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <path d="M3 6h18" />
            <path d="M19 6v14c0 1-1 2-2 2H7c-1 0-2-1-2-2V6" />
            <path d="M8 6V4c0-1 1-2 2-2h4c1 0 2 1 2 2v2" />
            <line x1="10" x2="10" y1="11" y2="17" />
            <line x1="14" x2="14" y1="11" y2="17" />
        </svg>
    }
}

/// Sparkles icon (Lucide).
#[component]
fn SparklesIcon(#[prop(into, optional)] class: String) -> impl IntoView {
    view! {
        <svg class=class xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <path d="M9.937 15.5A2 2 0 0 0 8.5 14.063l-6.135-1.582a.5.5 0 0 1 0-.962L8.5 9.936A2 2 0 0 0 9.937 8.5l1.582-6.135a.5.5 0 0 1 .963 0L14.063 8.5A2 2 0 0 0 15.5 9.937l6.135 1.581a.5.5 0 0 1 0 .964L15.5 14.063a2 2 0 0 0-1.437 1.437l-1.582 6.135a.5.5 0 0 1-.963 0z" />
        </svg>
    }
}

/// FileText icon (Lucide).
#[component]
fn FileTextIcon(#[prop(into, optional)] class: String) -> impl IntoView {
    view! {
        <svg class=class xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <path d="M15 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V7Z" />
            <path d="M14 2v4a2 2 0 0 0 2 2h4" />
            <path d="M10 9H8" />
            <path d="M16 13H8" />
            <path d="M16 17H8" />
        </svg>
    }
}

/// CheckCircle icon (Lucide).
#[component]
fn CheckCircleIcon(#[prop(into, optional)] class: String) -> impl IntoView {
    view! {
        <svg class=class xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <path d="M22 11.08V12a10 10 0 1 1-5.93-9.14" />
            <path d="m9 11 3 3L22 4" />
        </svg>
    }
}

/// XCircle icon (Lucide).
#[component]
fn XCircleIcon(#[prop(into, optional)] class: String) -> impl IntoView {
    view! {
        <svg class=class xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <circle cx="12" cy="12" r="10" />
            <path d="m15 9-6 6" />
            <path d="m9 9 6 6" />
        </svg>
    }
}

/// Loader2 icon (Lucide) — used for spinning animation.
#[component]
fn Loader2Icon(#[prop(into, optional)] class: String) -> impl IntoView {
    view! {
        <svg class=class xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <path d="M21 12a9 9 0 1 1-6.219-8.56" />
        </svg>
    }
}

/// AlertCircle icon (Lucide).
#[component]
fn AlertCircleIcon(#[prop(into, optional)] class: String) -> impl IntoView {
    view! {
        <svg class=class xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <circle cx="12" cy="12" r="10" />
            <line x1="12" x2="12" y1="8" y2="12" />
            <line x1="12" x2="12.01" y1="16" y2="16" />
        </svg>
    }
}

/// ChartBar icon (Heroicons outline) — used for report mode badge.
#[component]
fn ChartBarIcon(#[prop(into, optional)] class: String) -> impl IntoView {
    view! {
        <svg class=class xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 24 24" stroke-width="1.5" stroke="currentColor">
            <path stroke-linecap="round" stroke-linejoin="round" d="M3 13.125C3 12.504 3.504 12 4.125 12h2.25c.621 0 1.125.504 1.125 1.125v6.75C7.5 20.496 6.996 21 6.375 21h-2.25A1.125 1.125 0 0 1 3 19.875v-6.75ZM9.75 8.625c0-.621.504-1.125 1.125-1.125h2.25c.621 0 1.125.504 1.125 1.125v11.25c0 .621-.504 1.125-1.125 1.125h-2.25a1.125 1.125 0 0 1-1.125-1.125V8.625ZM16.5 4.125c0-.621.504-1.125 1.125-1.125h2.25C20.496 3 21 3.504 21 4.125v15.75c0 .621-.504 1.125-1.125 1.125h-2.25a1.125 1.125 0 0 1-1.125-1.125V4.125Z" />
        </svg>
    }
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

/// Format an RFC 3339 date string for display.
///
/// Matches React's `formatDate` helper in WatchesPage.jsx.
fn format_date(date_str: &str) -> String {
    let Ok(parsed) = chrono::DateTime::parse_from_rfc3339(date_str) else {
        return date_str.to_string();
    };
    // NOTE: Displays in UTC. Browser local time would require js_sys::Date
    // integration. The React version uses toLocaleString() for local display.
    let utc = parsed.with_timezone(&chrono::Utc);
    utc.format("%b %-d, %H:%M").to_string()
}

/// Map a watch status string to StatusBadge configuration.
fn status_badge_config(status: &str) -> (StatusBadgeVariant, &'static str) {
    match status {
        "success" => (StatusBadgeVariant::Success, "Success"),
        "error" => (StatusBadgeVariant::Error, "Error"),
        "running" => (StatusBadgeVariant::Info, "Running"),
        // "no_alert" and default
        _ => (StatusBadgeVariant::Default, "No Alert"),
    }
}

// ─── Watch Card Component ───────────────────────────────────────────────────

/// Individual watch card — extracted to reduce nesting in the main view.
#[component]
fn WatchCard(
    watch: WatchListItem,
    tz_offset: i32,
    /// Toggle enabled/disabled.
    on_toggle: Callback<String>,
    /// Run immediately.
    on_run: Callback<String>,
    /// Edit with AI sidebar.
    on_edit_ai: Callback<WatchListItem>,
    /// Quick edit via modal.
    on_edit: Callback<WatchListItem>,
    /// Request delete (opens confirm dialog).
    on_delete: Callback<WatchListItem>,
    /// View execution log.
    on_view_log: Callback<WatchListItem>,
    /// Whether toggle mutation is pending.
    #[prop(into)]
    toggle_pending: Signal<bool>,
    /// Whether run mutation is pending.
    #[prop(into)]
    run_pending: Signal<bool>,
    /// Whether AI features are enabled.
    #[prop(default = true)]
    ai_enabled: bool,
) -> impl IntoView {
    let watch_id = watch.watch_id.clone();
    let watch_name = watch.name.clone();
    let watch_prompt = watch.prompt.clone();
    let watch_mode = watch.mode.clone();
    let watch_schedule = watch.schedule.clone();
    let watch_enabled = watch.enabled;
    let last_run_status = watch.last_run_status.clone();
    let last_run_at = watch.last_run_at.clone();
    let next_run_at = watch.next_run_at.clone();
    let has_last_run = watch.last_run_at.is_some();

    let cron_desc = describe_cron(&watch_schedule, tz_offset);

    // Clones for each handler closure.
    let wid_toggle = watch_id.clone();
    let wid_run = watch_id.clone();
    let watch_for_ai = watch.clone();
    let watch_for_edit = watch.clone();
    let watch_for_delete = watch.clone();
    let watch_for_log = watch.clone();

    // Switch needs Signal<bool>, not plain bool.
    let enabled_signal = Signal::derive(move || watch_enabled);

    view! {
        <Card>
            <CardHeader class="pb-2".to_string()>
                <div class="flex items-start justify-between">
                    <div class="flex-1 min-w-0">
                        <CardTitle class="text-base truncate flex items-center gap-2".to_string()>
                            {if watch_mode == "report" {
                                view! { <ChartBarIcon class="h-4 w-4 shrink-0 text-muted-foreground".to_string() /> }.into_any()
                            } else {
                                view! { <BellIcon class="h-4 w-4 shrink-0 text-muted-foreground".to_string() /> }.into_any()
                            }}
                            {watch_name}
                        </CardTitle>
                        <p class="text-sm text-muted-foreground mt-1 line-clamp-2">
                            {watch_prompt}
                        </p>
                    </div>
                    <Switch
                        checked=enabled_signal
                        on_change=Callback::new(move |_: bool| {
                            on_toggle.run(wid_toggle.clone());
                        })
                        disabled=toggle_pending.get()
                    />
                </div>
            </CardHeader>
            <CardContent class="space-y-3".to_string()>
                // Schedule
                <div class="flex items-center text-sm text-muted-foreground">
                    <ClockIcon class="h-4 w-4 mr-2".to_string() />
                    {cron_desc.description}
                </div>

                // Status & last run
                <div class="flex items-center justify-between">
                    {last_run_status.map(|status| {
                        let (variant, label) = status_badge_config(&status);
                        view! {
                            <StatusBadge variant=variant class="gap-1">
                                {if status == "success" || status == "no_alert" {
                                    view! { <CheckCircleIcon class="h-3 w-3".to_string() /> }.into_any()
                                } else if status == "error" {
                                    view! { <XCircleIcon class="h-3 w-3".to_string() /> }.into_any()
                                } else if status == "running" {
                                    view! { <Loader2Icon class="h-3 w-3 animate-spin".to_string() /> }.into_any()
                                } else {
                                    view! { <CheckCircleIcon class="h-3 w-3".to_string() /> }.into_any()
                                }}
                                {label}
                            </StatusBadge>
                        }
                    })}
                    <span class="text-xs text-muted-foreground">
                        {last_run_at
                            .as_deref()
                            .map(|d| format!("Last: {}", format_date(d)))
                            .unwrap_or_else(|| "Not run yet".to_string())}
                    </span>
                </div>

                // Next run
                {(watch_enabled && next_run_at.is_some()).then(|| {
                    let next = next_run_at.as_deref().unwrap_or_default();
                    view! {
                        <div class="text-xs text-muted-foreground">
                            {format!("Next run: {}", format_date(next))}
                        </div>
                    }
                })}

                // Actions
                <div class="flex items-center gap-2 pt-2 border-t border-border">
                    <button
                        class=format!("{BTN_BASE} {BTN_GHOST} {BTN_SM}")
                        on:click=move |_| on_run.run(wid_run.clone())
                        disabled=move || run_pending.get()
                    >
                        <PlayIcon class="h-4 w-4".to_string() />
                    </button>
                    {has_last_run.then(|| {
                        let w = watch_for_log.clone();
                        view! {
                            <button
                                class=format!("{BTN_BASE} {BTN_GHOST} {BTN_SM}")
                                on:click=move |_| on_view_log.run(w.clone())
                            >
                                <FileTextIcon class="h-4 w-4".to_string() />
                            </button>
                        }
                    })}
                    <button
                        class=format!("{BTN_BASE} {BTN_GHOST} {BTN_SM}")
                        on:click={
                            let w = watch_for_ai.clone();
                            move |_| on_edit_ai.run(w.clone())
                        }
                        disabled=!ai_enabled
                        title=if !ai_enabled { "AI features not available" } else { "Edit with AI" }
                    >
                        <SparklesIcon class="h-4 w-4".to_string() />
                    </button>
                    <button
                        class=format!("{BTN_BASE} {BTN_GHOST} {BTN_SM}")
                        on:click={
                            let w = watch_for_edit.clone();
                            move |_| on_edit.run(w.clone())
                        }
                    >
                        <SettingsIcon class="h-4 w-4".to_string() />
                    </button>
                    <button
                        class=format!("{BTN_BASE} {BTN_GHOST} {BTN_SM} text-destructive hover:text-destructive")
                        on:click={
                            let w = watch_for_delete.clone();
                            move |_| on_delete.run(w.clone())
                        }
                    >
                        <TrashIcon class="h-4 w-4".to_string() />
                    </button>
                </div>
            </CardContent>
        </Card>
    }
}

// ─── Main Component ─────────────────────────────────────────────────────────

/// WatchesPage — main page with Alerts and Watches tabs.
///
/// Matches `apps/frontend/src/pages/WatchesPage.jsx` exactly.
#[component]
pub fn WatchesPage() -> impl IntoView {
    let navigate = use_navigate();
    let params = use_params_map();
    let query = use_query_map();

    // Deep-link alert ID from ?alert=N query parameter.
    let expanded_alert_id = Memo::new(move |_| {
        query.get().get("alert").and_then(|v| v.parse::<i32>().ok())
    });

    // ── User context & capabilities ─────────────────────────────────────
    let user_ctx_resource = Resource::new(|| (), |_| get_user_context());

    let has_watch_capability = Memo::new(move |_| {
        user_ctx_resource
            .get()
            .and_then(|r| r.ok())
            .and_then(|ctx| ctx.capabilities.get("kyomi_watch_enabled").copied())
            .unwrap_or(false)
    });

    let ai_enabled = Memo::new(move |_| {
        user_ctx_resource
            .get()
            .and_then(|r| r.ok())
            .and_then(|ctx| ctx.capabilities.get("ai_chat_enabled").copied())
            .unwrap_or(false)
    });

    let credits_exhausted = Memo::new(move |_| {
        user_ctx_resource
            .get()
            .and_then(|r| r.ok())
            .and_then(|ctx| ctx.capabilities.get("credits_exhausted").copied())
            .unwrap_or(false)
    });

    let ctx_loading = Memo::new(move |_| user_ctx_resource.get().is_none());

    // View from URL: "config" maps to "watches", anything else maps to "alerts"
    let active_view = Memo::new(move |_| {
        let view_param = params.get().get("view").unwrap_or_default();
        if view_param == "config" {
            "watches"
        } else {
            "alerts"
        }
    });

    // ── Watch Modal state (for quick editing) ────────────────────────────
    let (show_watch_modal, set_show_watch_modal) = signal(false);
    let (editing_watch, set_editing_watch) = signal(Option::<WatchListItem>::None);

    // ── Execution log modal state ────────────────────────────────────────
    let (show_execution_log, set_show_execution_log) = signal(false);
    let (viewing_watch_id, set_viewing_watch_id) = signal(Option::<String>::None);
    let (selected_execution_id, set_selected_execution_id) = signal(Option::<i32>::None);

    // ── AI Sidebar state ─────────────────────────────────────────────────
    let (show_agent_sidebar, set_show_agent_sidebar) = signal(false);
    let (agent_editing_watch, set_agent_editing_watch) = signal(Option::<WatchListItem>::None);

    // ── Delete confirmation ──────────────────────────────────────────────
    let (confirm_open, set_confirm_open) = signal(false);
    let (deleting_watch, set_deleting_watch) = signal(Option::<WatchListItem>::None);

    // ── Pending mutation states ──────────────────────────────────────────
    let (toggling, set_toggling) = signal(false);
    let (running, set_running) = signal(false);

    // ── Data fetching ────────────────────────────────────────────────────
    let (refetch_counter, set_refetch_counter) = signal(0u32);

    let watches_resource = Resource::new(
        move || refetch_counter.get(),
        |_| list_watches(),
    );

    // Executions list resource (for execution log modal).
    let executions_resource = Resource::new(
        move || {
            let wid = viewing_watch_id.get();
            let show = show_execution_log.get();
            (wid, show)
        },
        move |(wid, show)| async move {
            if let (Some(watch_id), true) = (wid, show) {
                get_watch_executions(watch_id, None).await
            } else {
                Ok(vec![])
            }
        },
    );

    // Selected execution with full trace.
    let execution_detail_resource = Resource::new(
        move || {
            let wid = viewing_watch_id.get();
            let show = show_execution_log.get();
            let selected = selected_execution_id.get();
            (wid, show, selected)
        },
        move |(wid, show, selected)| async move {
            if let (Some(watch_id), true) = (wid, show) {
                if let Some(eid) = selected {
                    get_watch_execution(watch_id, eid).await.ok()
                } else {
                    None
                }
            } else {
                None
            }
        },
    );

    // ── Handlers ─────────────────────────────────────────────────────────

    let handle_toggle_watch = Callback::new(move |watch_id: String| {
        set_toggling.set(true);
        leptos::task::spawn_local(async move {
            match toggle_watch(watch_id).await {
                Ok(()) => set_refetch_counter.update(|c| *c += 1),
                Err(e) => toast_error(format!("Failed to toggle watch: {e}")),
            }
            set_toggling.set(false);
        });
    });

    let handle_run_now = Callback::new(move |watch_id: String| {
        set_running.set(true);
        leptos::task::spawn_local(async move {
            match run_watch_now(watch_id).await {
                Ok(()) => {
                    toast_success("Watch execution started");
                    set_refetch_counter.update(|c| *c += 1);
                }
                Err(e) => toast_error(format!("Failed to run watch: {e}")),
            }
            set_running.set(false);
        });
    });

    let handle_edit_watch = Callback::new(move |watch: WatchListItem| {
        set_editing_watch.set(Some(watch));
        set_show_watch_modal.set(true);
    });

    let handle_edit_with_ai = Callback::new(move |watch: WatchListItem| {
        set_agent_editing_watch.set(Some(watch));
        set_show_agent_sidebar.set(true);
    });

    let handle_request_delete = Callback::new(move |watch: WatchListItem| {
        set_deleting_watch.set(Some(watch));
        set_confirm_open.set(true);
    });

    let handle_view_execution_log = Callback::new(move |watch: WatchListItem| {
        set_viewing_watch_id.set(Some(watch.watch_id.clone()));
        set_selected_execution_id.set(None);
        set_show_execution_log.set(true);
    });

    // Delete confirm/cancel callbacks.
    let handle_delete_confirm = Callback::new(move |()| {
        set_confirm_open.set(false);
        if let Some(watch) = deleting_watch.get_untracked() {
            let watch_id = watch.watch_id.clone();
            leptos::task::spawn_local(async move {
                match delete_watch(watch_id).await {
                    Ok(()) => {
                        toast_success("Watch deleted");
                        set_refetch_counter.update(|c| *c += 1);
                    }
                    Err(e) => toast_error(format!("Failed to delete watch: {e}")),
                }
            });
        }
        set_deleting_watch.set(None);
    });

    let handle_delete_cancel = Callback::new(move |()| {
        set_confirm_open.set(false);
        set_deleting_watch.set(None);
    });

    // Watch modal callbacks.
    let on_watch_modal_close = Callback::new(move |()| {
        set_show_watch_modal.set(false);
        set_editing_watch.set(None);
    });

    let on_watch_saved = Callback::new(move |()| {
        set_show_watch_modal.set(false);
        set_editing_watch.set(None);
        set_refetch_counter.update(|c| *c += 1);
    });

    // Agent sidebar callbacks.
    let on_agent_close = Callback::new(move |()| {
        set_show_agent_sidebar.set(false);
        set_agent_editing_watch.set(None);
    });

    let on_watch_changed_by_agent = Callback::new(move |()| {
        // Read editing state before it might be cleared by on_agent_close.
        let was_editing = agent_editing_watch.get_untracked().is_some();
        set_refetch_counter.update(|c| *c += 1);
        toast_success(if was_editing { "Watch updated" } else { "Watch created" });
    });

    // Execution log callbacks.
    let on_select_execution = Callback::new(move |exec_id: i32| {
        set_selected_execution_id.set(Some(exec_id));
    });

    // AlertsHistory "continue in chat" handler.
    let on_continue_chat = {
        let navigate = navigate.clone();
        Callback::new(move |session_id: String| {
            navigate(&format!("/chat/{session_id}"), Default::default());
        })
    };

    // Confirm dialog derived values.
    let confirm_message = move || {
        deleting_watch
            .get()
            .map(|w| {
                format!(
                    "Are you sure you want to delete \"{}\"? This action cannot be undone.",
                    w.name
                )
            })
            .unwrap_or_default()
    };

    // Timezone offset for cron descriptions.
    let tz_offset = get_tz_offset_minutes();

    // Navigation helpers — stored as Callbacks so they are Copy and can be used
    // inside `move` closures without consuming them.
    let nav_alerts = StoredValue::new({
        let navigate = navigate.clone();
        move || {
            navigate("/watches/alerts", Default::default());
        }
    });
    let nav_watches = StoredValue::new({
        let navigate = navigate.clone();
        move || {
            navigate("/watches/config", Default::default());
        }
    });
    let handle_create_watch = move |_: leptos::ev::MouseEvent| {
        set_agent_editing_watch.set(None);
        set_show_agent_sidebar.set(true);
    };

    view! {
        {move || {
            // Loading state for capabilities
            if ctx_loading.get() {
                return view! {
                    <div class="flex items-center justify-center h-full">
                        <Spinner />
                    </div>
                }.into_any();
            }

            // Capability gate — upgrade prompt for non-Pro users
            if !has_watch_capability.get() {
                return view! {
                    <div class="h-full flex flex-col bg-background">
                        <div class="flex items-center justify-between px-6 py-4 border-b border-border">
                            <div class="flex items-center gap-3">
                                <EyeIcon class="h-6 w-6 text-primary".to_string() />
                                <h1 class="text-xl font-semibold text-foreground">"Kyomi Watch"</h1>
                            </div>
                        </div>
                        <div class="flex-1 flex items-center justify-center p-6">
                            <Card class="max-w-lg".to_string()>
                                <CardHeader class="text-center".to_string()>
                                    <div class="mx-auto mb-4 h-16 w-16 rounded-full bg-primary/10 flex items-center justify-center">
                                        <EyeIcon class="h-8 w-8 text-primary".to_string() />
                                    </div>
                                    <CardTitle class="text-xl".to_string()>"Proactive Data Monitoring"</CardTitle>
                                    <p class="text-base text-muted-foreground">
                                        "Let Kyomi watch your data and alert you when something noteworthy happens."
                                    </p>
                                </CardHeader>
                                <CardContent class="space-y-4".to_string()>
                                    <ul class="space-y-3 text-sm text-muted-foreground">
                                        <li class="flex items-start gap-2">
                                            <CheckCircleIcon class="h-5 w-5 text-success-foreground mt-0.5 shrink-0".to_string() />
                                            <span>"Monitor data with plain English instructions"</span>
                                        </li>
                                        <li class="flex items-start gap-2">
                                            <CheckCircleIcon class="h-5 w-5 text-success-foreground mt-0.5 shrink-0".to_string() />
                                            <span>"Get alerts when metrics change or anomalies occur"</span>
                                        </li>
                                        <li class="flex items-start gap-2">
                                            <CheckCircleIcon class="h-5 w-5 text-success-foreground mt-0.5 shrink-0".to_string() />
                                            <span>"Schedule checks hourly, daily, or custom intervals"</span>
                                        </li>
                                    </ul>
                                    <div class="pt-4">
                                        <a
                                            href="/settings/billing"
                                            class=format!("{BTN_BASE} {BTN_DEFAULT} {BTN_DEFAULT_SIZE} w-full")
                                        >
                                            "Upgrade to Pro"
                                        </a>
                                        <p class="text-xs text-center text-muted-foreground mt-2">
                                            "Kyomi Watch is available on Pro and Team plans"
                                        </p>
                                    </div>
                                </CardContent>
                            </Card>
                        </div>
                    </div>
                }.into_any();
            }

            let is_ai_enabled = ai_enabled.get();
            let is_credits_exhausted = credits_exhausted.get();

            view! {
                <div class="h-full flex bg-muted">
                    // Main content area
                    <div class="flex-1 flex flex-col min-w-0 overflow-hidden">
                        // Header
                        <div class="h-16 bg-card border-b border-border px-6 flex-shrink-0 flex items-center justify-between">
                            <div class="flex items-center gap-3">
                                <EyeIcon class="h-6 w-6 text-primary hidden sm:block".to_string() />
                                <h1 class="text-lg sm:text-xl font-semibold text-foreground">"Kyomi Watch"</h1>
                            </div>
                            <div class="flex items-center gap-2">
                                // View toggle — Alerts first since it's the inbox
                                <div class="flex items-center rounded-lg bg-muted p-1">
                                    <button
                                        on:click=move |_| nav_alerts.with_value(|f| f())
                                        class=move || {
                                            if active_view.get() == "alerts" {
                                                "flex items-center gap-1.5 px-2 sm:px-3 py-1.5 text-sm rounded-md transition-colors bg-background text-foreground shadow-sm"
                                            } else {
                                                "flex items-center gap-1.5 px-2 sm:px-3 py-1.5 text-sm rounded-md transition-colors text-muted-foreground hover:text-foreground"
                                            }
                                        }
                                    >
                                        <BellIcon class="h-4 w-4".to_string() />
                                        <span class="hidden sm:inline">"Alerts"</span>
                                    </button>
                                    <button
                                        on:click=move |_| nav_watches.with_value(|f| f())
                                        class=move || {
                                            if active_view.get() == "watches" {
                                                "flex items-center gap-1.5 px-2 sm:px-3 py-1.5 text-sm rounded-md transition-colors bg-background text-foreground shadow-sm"
                                            } else {
                                                "flex items-center gap-1.5 px-2 sm:px-3 py-1.5 text-sm rounded-md transition-colors text-muted-foreground hover:text-foreground"
                                            }
                                        }
                                    >
                                        <EyeIcon class="h-4 w-4".to_string() />
                                        <span class="hidden sm:inline">"Watches"</span>
                                    </button>
                                </div>
                                <button
                                    class=format!("{BTN_BASE} {BTN_DEFAULT} {BTN_DEFAULT_SIZE}")
                                    on:click=handle_create_watch
                                    disabled=!is_ai_enabled
                                    title=if !is_ai_enabled {
                                        if is_credits_exhausted { "AI budget exhausted for this billing period" }
                                        else { "AI features are not available" }
                                    } else { "" }
                                >
                                    <PlusIcon class="h-4 w-4".to_string() />
                                    <span class="hidden sm:inline">"Create Watch"</span>
                                </button>
                            </div>
                        </div>

                        // Budget exhausted warning
                        {is_credits_exhausted.then(|| {
                            view! {
                                <div class="px-6 pt-4">
                                    <Alert variant=AlertVariant::Warning>
                                        <AlertCircleIcon class="h-4 w-4".to_string() />
                                        <AlertDescription>
                                            "Your AI budget is exhausted for this billing period. Existing watches will not run until your budget resets. "
                                            <a href="/settings/billing" class="underline hover:text-foreground">"Upgrade your plan"</a>
                                        </AlertDescription>
                                    </Alert>
                                </div>
                            }
                        })}

                // Content
                <div class="flex-1 overflow-auto p-3 sm:p-6 @container">
                    {move || {
                        if active_view.get() == "watches" {
                            // Watches list view
                            view! {
                                <Transition fallback=move || view! {
                                    <div class="flex items-center justify-center py-12">
                                        <Spinner />
                                    </div>
                                }>
                                    {move || {
                                        watches_resource.get().map(|result| {
                                            match result {
                                                Err(e) => view! {
                                                    <Alert variant=AlertVariant::Error>
                                                        <AlertCircleIcon class="h-4 w-4".to_string() />
                                                        <AlertDescription>
                                                            {format!("Failed to load watches: {e}")}
                                                        </AlertDescription>
                                                    </Alert>
                                                }.into_any(),
                                                Ok(watches) if watches.is_empty() => view! {
                                                    <div class="flex flex-col items-center justify-center py-12 text-center">
                                                        <div class="h-16 w-16 rounded-full bg-muted flex items-center justify-center mb-4">
                                                            <EyeIcon class="h-8 w-8 text-muted-foreground".to_string() />
                                                        </div>
                                                        <h3 class="text-lg font-medium text-foreground mb-2">"No watches yet"</h3>
                                                        <p class="text-muted-foreground mb-4 max-w-md">
                                                            {if is_credits_exhausted {
                                                                "Your AI budget is exhausted. Wait for it to reset or upgrade your plan to create watches."
                                                            } else {
                                                                "Create your first watch to start monitoring your data proactively."
                                                            }}
                                                        </p>
                                                        <button
                                                            class=format!("{BTN_BASE} {BTN_DEFAULT} {BTN_DEFAULT_SIZE}")
                                                            on:click=move |_| {
                                                                set_agent_editing_watch.set(None);
                                                                set_show_agent_sidebar.set(true);
                                                            }
                                                            disabled=!is_ai_enabled
                                                        >
                                                            <PlusIcon class="h-4 w-4 mr-2".to_string() />
                                                            "Create Watch"
                                                        </button>
                                                    </div>
                                                }.into_any(),
                                                Ok(watches) => {
                                                    let cards = watches.into_iter().map(|watch| {
                                                        view! {
                                                            <WatchCard
                                                                watch=watch
                                                                tz_offset=tz_offset
                                                                on_toggle=handle_toggle_watch
                                                                on_run=handle_run_now
                                                                on_edit_ai=handle_edit_with_ai
                                                                on_edit=handle_edit_watch
                                                                on_delete=handle_request_delete
                                                                on_view_log=handle_view_execution_log
                                                                toggle_pending=Signal::derive(move || toggling.get())
                                                                run_pending=Signal::derive(move || running.get())
                                                                ai_enabled=is_ai_enabled
                                                            />
                                                        }
                                                    }).collect_view();
                                                    view! {
                                                        <div class="grid gap-4 @2xl:grid-cols-2 @4xl:grid-cols-3">
                                                            {cards}
                                                        </div>
                                                    }.into_any()
                                                }
                                            }
                                        })
                                    }}
                                </Transition>
                            }.into_any()
                        } else {
                            // Alerts history view
                            view! {
                                <AlertsHistory
                                    on_continue_chat=on_continue_chat
                                    expanded_alert_id=expanded_alert_id.get()
                                />
                            }.into_any()
                        }
                    }}
                </div>

                // Watch Modal — only for editing existing watches
                {move || {
                    editing_watch.get().map(|watch| {
                        view! {
                            <WatchModal
                                watch=watch
                                open=Signal::derive(move || show_watch_modal.get())
                                on_close=on_watch_modal_close
                                on_saved=on_watch_saved
                            />
                        }
                    })
                }}

                // Execution Log Modal
                {move || {
                    let show = show_execution_log.get();
                    let wid = viewing_watch_id.get();
                    (show && wid.is_some()).then(|| {
                        let watch_id = wid.unwrap();
                        let execs = executions_resource.get()
                            .and_then(|r| r.ok())
                            .unwrap_or_default();
                        let selected_exec = StoredValue::new(execution_detail_resource.get().flatten());
                        let is_loading = Signal::derive(move || {
                            executions_resource.get().is_none()
                        });
                        // Find watch name and prompt from watches resource (single lookup).
                        let (watch_name, watch_prompt) = watches_resource.get()
                            .and_then(|r| r.ok())
                            .and_then(|watches| {
                                watches.iter().find(|w| w.watch_id == watch_id)
                                    .map(|w| (w.name.clone(), w.prompt.clone()))
                            })
                            .unwrap_or_default();

                        let modal_title = format!("Execution History - {watch_name}");
                        let execs = StoredValue::new(execs);
                        let watch_prompt = StoredValue::new(watch_prompt);
                        view! {
                            <Modal
                                show=Signal::derive(move || show_execution_log.get())
                                on_close=Callback::new(move |()| {
                                    set_show_execution_log.set(false);
                                    set_viewing_watch_id.set(None);
                                    set_selected_execution_id.set(None);
                                })
                                title=modal_title
                                size=ModalSize::Xl
                            >
                                <ExecutionLogViewer
                                    executions=execs.get_value()
                                    selected_execution=Signal::derive(move || selected_exec.get_value())
                                    on_select_execution=on_select_execution
                                    is_loading=is_loading
                                    watch_prompt=watch_prompt.get_value()
                                />
                            </Modal>
                        }
                    })
                }}

                // Confirm Dialog — wrapped in reactive closure so message updates
                {move || {
                    let msg = confirm_message();
                    view! {
                        <ConfirmDialog
                            open=Signal::derive(move || confirm_open.get())
                            title="Delete Watch"
                            message=msg
                            confirm_text="Delete"
                            destructive=true
                            on_confirm=handle_delete_confirm
                            on_cancel=handle_delete_cancel
                        />
                    }
                }}
            </div>

            // AI Agent Sidebar — reactive so editing_watch prop updates
            {move || {
                let ew = agent_editing_watch.get();
                view! {
                    <WatchAgentSidebar
                        open=Signal::derive(move || show_agent_sidebar.get())
                        on_close=on_agent_close
                        on_watch_changed=on_watch_changed_by_agent
                        editing_watch=ew
                    />
                }
            }}
        </div>
            }.into_any()
        }}
    }
}
