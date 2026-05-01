// SPDX-License-Identifier: AGPL-3.0-or-later

//! WatchesPage — main page with two tabs: Alerts and Watches.
//!
//! Ported from `apps/frontend/src/pages/WatchesPage.jsx` (667 lines).
//!
//! Features:
//! - Header with "Watches" title and "Create Watch" button
//! - Two tabs: Alerts (inbox) and Watches (config)
//! - Alerts tab shows `AlertsHistory` component
//! - Watches tab shows watch cards with toggle, run, edit, delete actions
//! - WatchModal opens for creating/editing watches (Configure + AI sub-tabs)
//! - Execution log modal for viewing watch run history

use leptos::prelude::*;
use phosphor_leptos::{Icon, IconWeight};
use leptos_router::hooks::{use_navigate, use_params_map, use_query_map};

use crate::components::toast::{toast_error, toast_success};
use crate::components::watches::{AlertsHistory, ExecutionLogViewer, WatchModal};
use crate::components::{
    Alert, AlertDescription, AlertVariant, Button, ButtonLink, ButtonSize, ButtonVariant, Card,
    CardContent, CardHeader, CardTitle, ConfirmDialog, EmptyState, ListPageSkeleton, Modal,
    ModalSize, Spinner, StatusBadge, StatusBadgeVariant, Switch,
};
use crate::server_fns::context::UserContext;
use crate::server_fns::watches::{
    delete_watch, get_watch_execution, get_watch_executions, run_watch_now,
    toggle_watch,
};
use crate::types::{WatchExecutionItem, WatchListItem};
use crate::utils::cron::{describe_cron, get_tz_offset_minutes};

// ─── Helpers ─────────────────────────────────────────────────────────────────

/// Format an RFC 3339 date string for display.
///
/// Matches React's `formatDate` helper in WatchesPage.jsx:
/// `date.toLocaleString(undefined, { month: 'short', day: 'numeric', hour: '2-digit', minute: '2-digit' })`.
///
/// Uses `js_sys::Date` on WASM for proper locale/timezone handling. Falls back
/// to `chrono` on the server.
fn format_date(date_str: &str) -> String {
    #[cfg(target_arch = "wasm32")]
    {
        let js_date = js_sys::Date::new(&wasm_bindgen::JsValue::from_str(date_str));
        if js_date.get_time().is_nan() {
            return date_str.to_string();
        }

        let month = match js_date.get_month() {
            0 => "Jan",
            1 => "Feb",
            2 => "Mar",
            3 => "Apr",
            4 => "May",
            5 => "Jun",
            6 => "Jul",
            7 => "Aug",
            8 => "Sep",
            9 => "Oct",
            10 => "Nov",
            11 => "Dec",
            _ => "???",
        };
        let day = js_date.get_date();
        let hours = js_date.get_hours();
        let minutes = js_date.get_minutes();
        let hour_12 = if hours == 0 {
            12
        } else if hours > 12 {
            hours - 12
        } else {
            hours
        };
        let ampm = if hours < 12 { "am" } else { "pm" };

        format!("{day} {month}, {:02}:{:02} {ampm}", hour_12, minutes)
    }

    // Server-side fallback: use chrono (UTC).
    #[cfg(not(target_arch = "wasm32"))]
    {
        let Ok(parsed) = chrono::DateTime::parse_from_rfc3339(date_str) else {
            return date_str.to_string();
        };
        let utc = parsed.with_timezone(&chrono::Utc);
        let formatted = utc.format("%-d %b, %I:%M %p").to_string();
        formatted.replace(" AM", " am").replace(" PM", " pm")
    }
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
    /// Edit watch via modal.
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
                                view! { <Icon icon=phosphor_leptos::CHART_BAR attr:class="h-4 w-4 shrink-0 text-muted-foreground" /> }.into_any()
                            } else {
                                view! { <Icon icon=phosphor_leptos::BELL attr:class="h-4 w-4 shrink-0 text-muted-foreground" /> }.into_any()
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
                    <Icon icon=phosphor_leptos::CLOCK attr:class="h-4 w-4 mr-2" />
                    {cron_desc.description}
                </div>

                // Status & last run
                <div class="flex items-center justify-between">
                    {last_run_status.map(|status| {
                        let (variant, label) = status_badge_config(&status);
                        view! {
                            <StatusBadge variant=variant class="gap-1">
                                {if status == "success" || status == "no_alert" {
                                    view! { <Icon icon=phosphor_leptos::CHECK_CIRCLE attr:class="h-3 w-3" /> }.into_any()
                                } else if status == "error" {
                                    view! { <Icon icon=phosphor_leptos::X_CIRCLE attr:class="h-3 w-3" /> }.into_any()
                                } else if status == "running" {
                                    view! { <Icon icon=phosphor_leptos::CIRCLE_NOTCH attr:class="h-3 w-3 animate-spin" /> }.into_any()
                                } else {
                                    view! { <Icon icon=phosphor_leptos::CHECK_CIRCLE attr:class="h-3 w-3" /> }.into_any()
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
                    <Button
                        variant=ButtonVariant::GhostMuted
                        size=ButtonSize::IconSm
                        on:click=move |_| on_run.run(wid_run.clone())
                        disabled=MaybeProp::derive(move || Some(run_pending.get()))
                        aria_label="Run now"
                    >
                        <Icon icon=phosphor_leptos::PLAY attr:class="h-4 w-4" />
                    </Button>
                    {has_last_run.then(|| {
                        let w = watch_for_log.clone();
                        view! {
                            <Button
                                variant=ButtonVariant::GhostMuted
                                size=ButtonSize::IconSm
                                on:click=move |_| on_view_log.run(w.clone())
                                aria_label="View execution log"
                            >
                                <Icon icon=phosphor_leptos::FILE_TEXT attr:class="h-4 w-4" />
                            </Button>
                        }
                    })}
                    <Button
                        variant=ButtonVariant::GhostMuted
                        size=ButtonSize::IconSm
                        on:click={
                            let w = watch_for_edit.clone();
                            move |_| on_edit.run(w.clone())
                        }
                        aria_label="Edit watch"
                    >
                        <Icon icon=phosphor_leptos::GEAR attr:class="h-4 w-4" />
                    </Button>
                    <Button
                        variant=ButtonVariant::GhostDestructive
                        size=ButtonSize::IconSm
                        on:click={
                            let w = watch_for_delete.clone();
                            move |_| on_delete.run(w.clone())
                        }
                        aria_label="Delete watch"
                    >
                        <Icon icon=phosphor_leptos::TRASH attr:class="h-4 w-4" />
                    </Button>
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
    // Provided by the parent Layout — one fetch per session.
    let user_ctx_resource =
        expect_context::<LocalResource<Result<UserContext, ServerFnError>>>();

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

    // ── Unified Watch Modal state ──────────────────────────────────────
    let (show_watch_modal, set_show_watch_modal) = signal(false);
    let (modal_watch, set_modal_watch) = signal(Option::<WatchListItem>::None);
    let (modal_initial_tab, set_modal_initial_tab) = signal("configure".to_string());

    // ── Execution log modal state ────────────────────────────────────────
    let (show_execution_log, set_show_execution_log) = signal(false);
    let (viewing_watch_id, set_viewing_watch_id) = signal(Option::<String>::None);
    let (selected_execution_id, set_selected_execution_id) = signal(Option::<i32>::None);

    // ── Delete confirmation ──────────────────────────────────────────────
    let (confirm_open, set_confirm_open) = signal(false);
    let (deleting_watch, set_deleting_watch) = signal(Option::<WatchListItem>::None);

    // ── Pending mutation states ──────────────────────────────────────────
    let (toggling, set_toggling) = signal(false);
    let (running, set_running) = signal(false);

    // ── Data fetching ────────────────────────────────────────────────────
    // Watches come from the SyncStore (populated from IndexedDB on startup
    // and kept current by the sync engine). Mutations (toggle/delete/run/create)
    let sync_store = expect_context::<crate::cache::store::SyncStore>();
    let watches_signal = sync_store.watches();
    let store_initialized = sync_store.initialized();

    // Executions list — manual signal + effect (Resource::new keeps stale initial
    // value across refetches in CSR mode, preventing the modal from showing data).
    let executions_data = RwSignal::new(Option::<Result<Vec<WatchExecutionItem>, ServerFnError>>::None);
    Effect::new(move |_| {
        let wid = viewing_watch_id.get();
        let show = show_execution_log.get();
        if let (Some(watch_id), true) = (wid, show) {
            executions_data.set(None);
            leptos::task::spawn_local(async move {
                let result = get_watch_executions(watch_id, None).await;
                if let Ok(ref execs) = result && !execs.is_empty() && selected_execution_id.try_get_untracked().flatten().is_none() {
                    set_selected_execution_id.try_set(Some(execs[0].id));
                }
                executions_data.try_set(Some(result));
            });
        } else {
            executions_data.set(None);
        }
    });

    // Selected execution detail — same pattern.
    let execution_detail_data = RwSignal::new(Option::<Option<WatchExecutionItem>>::None);
    Effect::new(move |_| {
        let wid = viewing_watch_id.get();
        let show = show_execution_log.get();
        let selected = selected_execution_id.get();
        if let (Some(watch_id), true) = (wid, show) {
            if let Some(eid) = selected {
                execution_detail_data.set(None);
                leptos::task::spawn_local(async move {
                    let result = get_watch_execution(watch_id, eid).await.ok();
                    execution_detail_data.try_set(Some(result));
                });
            } else {
                execution_detail_data.set(Some(None));
            }
        } else {
            execution_detail_data.set(None);
        }
    });

    // ── Handlers ─────────────────────────────────────────────────────────

    let handle_toggle_watch = Callback::new(move |watch_id: String| {
        set_toggling.set(true);
        leptos::task::spawn_local(async move {
            match toggle_watch(watch_id).await {
                Ok(()) => {
                    // Sync engine handles the watch list update via WebSocket (KYO-169).
                }
                Err(e) => toast_error(format!("Failed to toggle watch: {e}")),
            }
            set_toggling.try_set(false);
        });
    });

    let handle_run_now = Callback::new(move |watch_id: String| {
        set_running.set(true);
        leptos::task::spawn_local(async move {
            match run_watch_now(watch_id).await {
                Ok(()) => {
                    toast_success("Watch execution started");
                    // Sync engine handles the watch list update via WebSocket (KYO-169).
                }
                Err(e) => toast_error(format!("Failed to run watch: {e}")),
            }
            set_running.try_set(false);
        });
    });

    let handle_edit_watch = Callback::new(move |watch: WatchListItem| {
        set_modal_watch.set(Some(watch));
        set_modal_initial_tab.set("configure".to_string());
        set_show_watch_modal.set(true);
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
                        // Sync engine handles the watch list update via WebSocket (KYO-169).
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
        set_modal_watch.set(None);
    });

    let on_watch_saved = Callback::new(move |()| {
        // Sync engine handles the watch list update via WebSocket (KYO-169).
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
        set_modal_watch.set(None);
        set_modal_initial_tab.set("ai".to_string());
        set_show_watch_modal.set(true);
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
                        <div class="page-header h-16 px-4 md:px-6 flex-shrink-0 flex items-center justify-between">
                            <h1 class="text-3xl font-display text-foreground">"Watches"</h1>
                        </div>
                        <div class="flex-1 flex items-center justify-center p-6">
                            <Card class="max-w-lg".to_string()>
                                <CardHeader class="text-center".to_string()>
                                    <div class="mx-auto mb-4 h-16 w-16 rounded-full bg-primary/10 flex items-center justify-center">
                                        <Icon icon=phosphor_leptos::EYE attr:class="h-8 w-8 text-primary" />
                                    </div>
                                    <CardTitle class="text-xl".to_string()>"Proactive Data Monitoring"</CardTitle>
                                    <p class="text-base text-muted-foreground">
                                        "Let Kyomi watch your data and alert you when something noteworthy happens."
                                    </p>
                                </CardHeader>
                                <CardContent class="space-y-4".to_string()>
                                    <ul class="space-y-3 text-sm text-muted-foreground">
                                        <li class="flex items-start gap-2">
                                            <Icon icon=phosphor_leptos::CHECK_CIRCLE attr:class="h-5 w-5 text-success-foreground mt-0.5 shrink-0" />
                                            <span>"Monitor data with plain English instructions"</span>
                                        </li>
                                        <li class="flex items-start gap-2">
                                            <Icon icon=phosphor_leptos::CHECK_CIRCLE attr:class="h-5 w-5 text-success-foreground mt-0.5 shrink-0" />
                                            <span>"Get alerts when metrics change or anomalies occur"</span>
                                        </li>
                                        <li class="flex items-start gap-2">
                                            <Icon icon=phosphor_leptos::CHECK_CIRCLE attr:class="h-5 w-5 text-success-foreground mt-0.5 shrink-0" />
                                            <span>"Schedule checks hourly, daily, or custom intervals"</span>
                                        </li>
                                    </ul>
                                    <div class="pt-4">
                                        <ButtonLink href="/settings/billing" class="w-full">
                                            "Upgrade to Pro"
                                        </ButtonLink>
                                        <p class="text-xs text-center text-muted-foreground mt-2">
                                            "Watches are available on Pro and Team plans"
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
                <div class="h-full flex bg-background">
                    // Main content area
                    <div class="flex-1 flex flex-col min-w-0 overflow-hidden">
                        // Row 1: Header
                        <div class="page-header h-16 px-4 md:px-6 flex-shrink-0 flex items-center justify-between">
                            <h1 class="text-3xl font-display text-foreground">"Watches"</h1>
                            <Button
                                size=ButtonSize::Sm
                                on:click=handle_create_watch
                                disabled=MaybeProp::derive(move || Some(!is_ai_enabled))
                                aria_label=if !is_ai_enabled {
                                    if is_credits_exhausted { "AI budget exhausted for this billing period".to_string() }
                                    else { "AI features are not available".to_string() }
                                } else { "Create Watch".to_string() }
                            >
                                <Icon icon=phosphor_leptos::PLUS attr:class="h-4 w-4" />
                                <span class="hidden sm:inline">"Create Watch"</span>
                            </Button>
                        </div>

                        // Row 2: Underline tabs
                        <div class="px-4 md:px-6 border-b border-border flex items-center gap-4">
                            <button
                                on:click=move |_| nav_alerts.with_value(|f| f())
                                class=move || {
                                    if active_view.get() == "alerts" {
                                        "flex items-center gap-1.5 px-1 py-3 text-sm font-medium border-b-2 border-primary text-foreground transition-colors"
                                    } else {
                                        "flex items-center gap-1.5 px-1 py-3 text-sm font-medium border-b-2 border-transparent text-muted-foreground hover:text-foreground transition-colors"
                                    }
                                }
                            >
                                <Icon icon=phosphor_leptos::BELL attr:class="h-4 w-4" />
                                "Alerts"
                            </button>
                            <button
                                on:click=move |_| nav_watches.with_value(|f| f())
                                class=move || {
                                    if active_view.get() == "watches" {
                                        "flex items-center gap-1.5 px-1 py-3 text-sm font-medium border-b-2 border-primary text-foreground transition-colors"
                                    } else {
                                        "flex items-center gap-1.5 px-1 py-3 text-sm font-medium border-b-2 border-transparent text-muted-foreground hover:text-foreground transition-colors"
                                    }
                                }
                            >
                                <Icon icon=phosphor_leptos::EYE attr:class="h-4 w-4" />
                                "Watches"
                            </button>
                        </div>

                        // Budget exhausted warning
                        {is_credits_exhausted.then(|| {
                            view! {
                                <div class="px-6 pt-4">
                                    <Alert variant=AlertVariant::Warning>
                                        <Icon icon=phosphor_leptos::WARNING_CIRCLE attr:class="h-4 w-4" />
                                        <AlertDescription>
                                            "Your AI budget is exhausted for this billing period. Existing watches will not run until your budget resets. "
                                            <a href="/settings/billing" class="underline transition-colors hover:text-foreground">"Upgrade your plan"</a>
                                        </AlertDescription>
                                    </Alert>
                                </div>
                            }
                        })}

                // Content
                <div class="flex-1 overflow-auto p-4 md:p-6 @container">
                    {move || {
                        if active_view.get() == "watches" {
                            // Watches list view
                            view! {
                                <Transition fallback=move || view! { <ListPageSkeleton /> }>
                                    {move || {
                                        // Show skeleton until the SyncStore has been hydrated
                                        // from IndexedDB (KYO-169).
                                        if !store_initialized.get() {
                                            return view! { <ListPageSkeleton /> }.into_any();
                                        }
                                        let watches = watches_signal.get();
                                        if watches.is_empty() {
                                            let desc = if is_credits_exhausted {
                                                "Your AI budget is exhausted. Wait for it to reset or upgrade your plan to create watches."
                                            } else {
                                                "Create your first watch to start monitoring your data proactively."
                                            };
                                            view! {
                                                <EmptyState
                                                    icon=std::sync::Arc::new(|| view! { <Icon icon=phosphor_leptos::EYE weight=IconWeight::Duotone size="64px" /> }.into_any())
                                                    title="No watches yet"
                                                    description=desc.to_string()
                                                    action=std::sync::Arc::new(move || view! {
                                                        <Button
                                                            on:click=move |_| {
                                                                set_modal_watch.set(None);
                                                                set_modal_initial_tab.set("ai".to_string());
                                                                set_show_watch_modal.set(true);
                                                            }
                                                            disabled=MaybeProp::derive(move || Some(!is_ai_enabled))
                                                        >
                                                            <Icon icon=phosphor_leptos::PLUS attr:class="h-4 w-4" />
                                                            "Create Watch"
                                                        </Button>
                                                    }.into_any())
                                                />
                                            }.into_any()
                                        } else {
                                            let cards = watches.into_iter().map(|watch| {
                                                view! {
                                                    <WatchCard
                                                        watch=watch
                                                        tz_offset=tz_offset
                                                        on_toggle=handle_toggle_watch
                                                        on_run=handle_run_now
                                                        on_edit=handle_edit_watch
                                                        on_delete=handle_request_delete
                                                        on_view_log=handle_view_execution_log
                                                        toggle_pending=Signal::derive(move || toggling.get())
                                                        run_pending=Signal::derive(move || running.get())
                                                    />
                                                }
                                            }).collect_view();
                                            view! {
                                                <div class="grid gap-4 @2xl:grid-cols-2 @4xl:grid-cols-3">
                                                    {cards}
                                                </div>
                                            }.into_any()
                                        }
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

                // Watch Modal — create or edit
                {move || {
                    show_watch_modal.get().then(|| {
                        let watch = modal_watch.get();
                        let tab = modal_initial_tab.get();
                        view! {
                            <WatchModal
                                watch=watch
                                initial_tab=tab
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
                    show.then(|| wid.clone()).flatten().map(|watch_id| {
                        let watch_id_for_prompt = watch_id.clone();
                        let modal_title = watches_signal.get_untracked()
                            .iter()
                            .find(|w| w.watch_id == watch_id)
                            .map(|w| format!("Execution History - {}", w.name))
                            .unwrap_or_else(|| "Execution History".to_string());
                        let watch_prompt_val = StoredValue::new(
                            watches_signal.get_untracked()
                                .iter()
                                .find(|w| w.watch_id == watch_id_for_prompt)
                                .map(|w| w.prompt.clone())
                                .unwrap_or_default(),
                        );

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
                                {move || {
                                    let raw: Option<Result<Vec<WatchExecutionItem>, ServerFnError>> = executions_data.get();
                                    let is_loading = raw.is_none();
                                    let execs = raw
                                        .and_then(|r| r.ok())
                                        .unwrap_or_default();
                                    let selected: Option<WatchExecutionItem> = execution_detail_data.get().flatten();
                                    let selected_stored = StoredValue::new(selected);
                                    let wp = watch_prompt_val.get_value();
                                    view! {
                                        <ExecutionLogViewer
                                            executions=execs
                                            selected_execution=Signal::derive(move || selected_stored.get_value())
                                            on_select_execution=on_select_execution
                                            is_loading=Signal::derive(move || is_loading)
                                            watch_prompt=wp
                                        />
                                    }
                                }}
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

        </div>
            }.into_any()
        }}
    }
}
