// SPDX-License-Identifier: AGPL-3.0-or-later

//! AlertsHistory — Gmail-style alerts inbox with selection and bulk actions.
//!
//! Ported from `apps/frontend/src/components/watches/AlertsHistory.jsx` (694 lines).
//!
//! Features:
//! - Gmail-style: checkboxes always visible, toolbar swaps between filters and
//!   bulk actions based on selection state
//! - Per-alert actions: mark read/unread, delete/restore, continue in chat
//! - Filter by watch, toggle deleted alerts
//! - Pagination (20 per page)
//! - Auto-mark as read on expand

use std::collections::HashSet;

use leptos::prelude::*;
use leptos::server_fn::ServerFnError;
use phosphor_leptos::{Icon, IconWeight};
use crate::components::dashboard::{ChartInfoModal, MarkdownRenderer};
use crate::components::popover::{Placement, Popover};
use crate::components::{
    AlertsListSkeleton, Badge, BadgeVariant, Button, ButtonSize, ButtonVariant, Checkbox,
    Select, EmptyState, Label, Spinner, Switch,
};
use crate::query_cache::{use_query, QueryCache};
use crate::server_fns::context::UserContext;
use crate::server_fns::watches::{
    bulk_delete_alerts, bulk_mark_alerts_read, bulk_mark_alerts_unread, continue_alert_in_chat,
    delete_alert, get_alerts, list_watches, mark_alert_read, mark_alert_unread, restore_alert,
};
use crate::types::{AlertItem, WatchListItem};

// ─── Constants ──────────────────────────────────────────────────────────────

const ALERTS_PER_PAGE: i64 = 20;


// ─── Dropdown Menu ──────────────────────────────────────────────────────────

/// Per-alert action menu rendered via portal-based `Popover`.
///
/// Uses `Popover` to escape `overflow: hidden` on alert card containers.
/// Opens on click, closes on click-outside (handled by Popover) or item
/// selection.
#[component]
fn AlertDropdownMenu(
    /// Whether this alert is deleted.
    is_deleted: bool,
    /// Whether this alert is unread (non-deleted, no read_at).
    is_unread: bool,
    /// Alert execution ID.
    alert_id: i32,
    /// Shared per-mutation-kind `Action`s, owned by `AlertsHistory`'s stable
    /// outer scope — NOT declared per-row here. A per-row `Action` would live
    /// inside the `<Transition>` children closure, which is disposed and
    /// rebuilt every time `alerts_resource` refetches — and every one of
    /// these mutations triggers exactly that refetch via
    /// `query_cache.invalidate("alerts")`. A per-row `Action` completing
    /// after a *different* row's mutation lands would be silently dropped:
    /// its `Effect` no longer exists to run. See code review discussion on
    /// KYO-226. These four are `Copy` and passed down as props instead.
    continue_chat_action: Action<i32, Result<String, ServerFnError>>,
    mark_unread_action: Action<i32, Result<(), ServerFnError>>,
    delete_action: Action<i32, Result<(), ServerFnError>>,
    restore_action: Action<i32, Result<(), ServerFnError>>,
) -> impl IntoView {
    let (is_open, set_is_open) = signal(false);
    let trigger_ref = NodeRef::<leptos::html::Div>::new();

    // `continue_chat_action` is shared across every row, so dispatching from
    // two rows concurrently would let the second dispatch's write to
    // `.value()` land before the first row's `Effect` (owned by
    // `AlertsHistory`) has run — see the top-level `Effect` for this action.
    // The `pending()`-gated `disabled` below (mirrored on the standalone
    // button and the mobile menu item) makes double-dispatch structurally
    // impossible rather than merely unlikely, so this guard is a second,
    // defence-in-depth check.
    let handle_continue_chat = move |_: web_sys::MouseEvent| {
        set_is_open.set(false);
        if !continue_chat_action.pending().get_untracked() {
            continue_chat_action.dispatch(alert_id);
        }
    };

    let handle_mark_unread = move |_: web_sys::MouseEvent| {
        set_is_open.set(false);
        mark_unread_action.dispatch(alert_id);
    };

    let handle_delete = move |_: web_sys::MouseEvent| {
        set_is_open.set(false);
        delete_action.dispatch(alert_id);
    };

    let handle_restore = move |_: web_sys::MouseEvent| {
        set_is_open.set(false);
        restore_action.dispatch(alert_id);
    };

    view! {
        <div node_ref=trigger_ref>
            <button
                class="inline-flex items-center justify-center gap-2 whitespace-nowrap rounded-md text-sm font-medium transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:pointer-events-none disabled:opacity-50 [&_svg]:pointer-events-none [&_svg]:size-4 [&_svg]:shrink-0 text-foreground hover:bg-secondary hover:text-accent-foreground h-8 rounded-md px-3 text-xs"
                on:click=move |_| set_is_open.update(|v| *v = !*v)
                disabled=Signal::derive(move || continue_chat_action.pending().get())
            >
                <Icon icon=phosphor_leptos::DOTS_THREE_VERTICAL attr:class="h-4 w-4" />
            </button>
        </div>
        <Popover
            trigger_ref=trigger_ref
            open=Signal::from(is_open)
            on_close=Callback::new(move |_| set_is_open.set(false))
            placement=Placement::BOTTOM_END
            class="min-w-[10rem] rounded-md border border-border bg-popover text-popover-foreground shadow-md p-1"
        >
            // Continue in Chat — visible in dropdown on mobile
            <button
                class="relative flex w-full cursor-default select-none items-center rounded-sm py-1.5 px-2 text-sm outline-none transition-colors hover:bg-secondary hover:text-accent-foreground sm:hidden"
                disabled=Signal::derive(move || continue_chat_action.pending().get())
                on:click=handle_continue_chat
            >
                <Icon icon=phosphor_leptos::CHATS attr:class="h-4 w-4 mr-2" />
                "Continue in Chat"
            </button>

            // Mark as unread (only for read, non-deleted alerts)
            {(!is_unread && !is_deleted).then(|| view! {
                <button
                    class="relative flex w-full cursor-default select-none items-center rounded-sm py-1.5 px-2 text-sm outline-none transition-colors hover:bg-secondary hover:text-accent-foreground"
                    on:click=handle_mark_unread
                >
                    <Icon icon=phosphor_leptos::ENVELOPE_OPEN attr:class="h-4 w-4 mr-2" />
                    "Mark as unread"
                </button>
            })}

            // Delete/Restore
            {if is_deleted {
                view! {
                    <button
                        class="relative flex w-full cursor-default select-none items-center rounded-sm py-1.5 px-2 text-sm outline-none transition-colors hover:bg-secondary hover:text-accent-foreground"
                        on:click=handle_restore
                    >
                        <Icon icon=phosphor_leptos::ARROW_U_UP_LEFT attr:class="h-4 w-4 mr-2" />
                        "Restore"
                    </button>
                }.into_any()
            } else {
                view! {
                    <button
                        class="relative flex w-full cursor-default select-none items-center rounded-sm py-1.5 px-2 text-sm outline-none transition-colors hover:bg-destructive/10 [color:var(--color-destructive)]"
                        on:click=handle_delete
                    >
                        <Icon icon=phosphor_leptos::TRASH attr:class="h-4 w-4 mr-2" />
                        "Delete"
                    </button>
                }.into_any()
            }}
        </Popover>
    }
}

// ─── Helper functions ───────────────────────────────────────────────────────

/// Format a date string for display.
/// Matches React: `date.toLocaleString(undefined, { month: 'short', day: 'numeric', year: 'numeric', hour: '2-digit', minute: '2-digit' })`.
///
/// Uses `js_sys::Date` on WASM for proper locale/timezone handling (same as
/// React's `toLocaleString`). Falls back to `chrono` on the server.
fn format_date(date_str: &str) -> String {
    if date_str.is_empty() {
        return String::new();
    }

    // Use JS Date in WASM for proper locale formatting and timezone conversion,
    // matching React's `new Date(str).toLocaleString(...)` exactly.
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
        let year = js_date.get_full_year();
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

        format!("{day} {month} {year}, {:02}:{:02} {ampm}", hour_12, minutes)
    }

    // Server-side fallback: use chrono for best-effort formatting (UTC).
    #[cfg(not(target_arch = "wasm32"))]
    {
        let Ok(parsed) = chrono::DateTime::parse_from_rfc3339(date_str) else {
            return date_str.to_string();
        };
        let utc = parsed.with_timezone(&chrono::Utc);
        let formatted = utc.format("%-d %b %Y, %I:%M %p").to_string();
        // Lowercase AM/PM to match React's locale output (e.g. "06:14 pm")
        formatted.replace(" AM", " am").replace(" PM", " pm")
    }
}

/// Get the display name for a watch from an alert, looking up in the watches list if needed.
fn get_watch_name(alert: &AlertItem, watches: &[WatchListItem]) -> String {
    if let Some(ref name) = alert.watch_name
        && !name.is_empty()
    {
        return name.clone();
    }
    if let Some(ref watch_id) = alert.watch_id
        && let Some(watch) = watches.iter().find(|w| &w.watch_id == watch_id)
    {
        return watch.name.clone();
    }
    "Deleted Watch".to_string()
}

// ─── Main component ─────────────────────────────────────────────────────────

/// AlertsHistory — View all watch alerts with Gmail-style selection and bulk actions.
///
/// Shows execution history for watches that triggered alerts.
/// Filterable by watch and paginated.
#[component]
pub fn AlertsHistory(
    /// Optional pre-expanded alert ID (from URL params), as a reactive signal.
    /// Using `Signal<Option<i32>>` rather than a plain `Option<i32>` snapshot
    /// prevents disposed-scope panics when the user navigates away while an
    /// async operation is still in flight (KYO-274).
    expanded_alert_id: Signal<Option<i32>>,
    /// Called when user clicks "Continue in Chat" on an alert.
    on_continue_chat: Callback<String>,
) -> impl IntoView {
    // ── Workspace id for chart provider wiring (KYO-119) ─────────────────
    // Alerts can contain chartml blocks that resolve `data: { datasource,
    // query }` via `KyomiDatasourceProvider`. The alerts inbox lives outside
    // `DashboardChartProviders`, so we pass `workspace_id` into
    // `MarkdownRenderer` which locally registers the provider the same way
    // the dashboard root does. `None` skips registration — matches what the
    // renderer did before this ticket for deployments without a workspace id.
    //
    // Derived the same way `dashboard_viewer.rs` does (minus the "default"
    // fallback — per KYO-119, we skip provider registration when the user
    // context lacks a workspace id rather than fabricating one).
    let user_ctx_resource =
        expect_context::<LocalResource<Result<UserContext, ServerFnError>>>();
    let workspace_id: Memo<Option<String>> = Memo::new(move |_| {
        user_ctx_resource
            .get()
            .and_then(|r| r.ok())
            .and_then(|ctx| ctx.workspace_id)
    });

    // ── Chart header action state (KYO-120) ──────────────────────────────
    // Each expanded alert renders its chartml blocks through `MarkdownRenderer`,
    // and the renderer only shows the `info` + `ask-about-chart` header
    // buttons when the caller wires the matching callbacks (see
    // `markdown_renderer.rs::has_info / has_ask`). We keep the historical-
    // snapshot semantics of an alert intact by NOT wiring `edit`, `delete`,
    // `refresh`, or `save_to_dashboard` — alerts are point-in-time records.
    let (chart_info_yaml, set_chart_info_yaml) = signal(String::new());
    let (chart_info_open, set_chart_info_open) = signal(false);

    let on_chart_info = Callback::new(move |yaml: String| {
        set_chart_info_yaml.set(yaml);
        set_chart_info_open.set(true);
    });

    let on_chart_info_close = Callback::new(move |()| {
        set_chart_info_open.set(false);
    });

    // Store the chart YAML in KV and navigate to chat with the returned UUID.
    // Using `store_chart_context_for_ask` instead of passing raw YAML in the URL
    // so the chat page's `get_chart_context` lookup succeeds (it expects a UUID key).
    let on_ask_about_chart = Callback::new(move |chart_md: String| {
        let nav = leptos_router::hooks::use_navigate();
        leptos::task::spawn_local(async move {
            match crate::server_fns::chat::store_chart_context_for_ask(
                chart_md,
                "Chart Exploration".to_string(),
            )
            .await
            {
                Ok(chart_id) => {
                    nav(
                        &format!("/chat?chart={chart_id}"),
                        leptos_router::NavigateOptions::default(),
                    );
                }
                Err(e) => {
                    tracing::warn!(error = %e, "Failed to store chart context for ask");
                }
            }
        });
    });

    // ── State ────────────────────────────────────────────────────────────
    let selected_watch_id = RwSignal::new(String::new());
    let expanded_alerts = RwSignal::new(HashSet::<i32>::new());
    let show_deleted = RwSignal::new(false);
    let page = RwSignal::new(0i64);
    let selected_alerts = RwSignal::new(HashSet::<i32>::new());

    // ── Resources ────────────────────────────────────────────────────────
    // Backed by the Layout-level QueryCache so both `watches` and `alerts`
    // entries are reused across navigation instead of re-fetching on mount
    // (KYO-22 Part 2). Mutations call `query_cache.invalidate` — the old
    // `refetch_trigger` counter is gone.
    let query_cache = expect_context::<QueryCache>();

    // Fetch watches for filter dropdown — shared with WatchesPage.
    let watches_resource = use_query(
        "watches",
        || (),
        |_: ()| list_watches(),
    );

    // Fetch alerts history, reactive to filter/page state.
    let alerts_resource = use_query(
        "alerts",
        move || (selected_watch_id.get(), page.get(), show_deleted.get()),
        |(watch_id, current_page, include_deleted): (String, i64, bool)| async move {
            let wid = if watch_id.is_empty() {
                None
            } else {
                Some(watch_id)
            };
            get_alerts(
                wid,
                Some(ALERTS_PER_PAGE),
                Some(current_page * ALERTS_PER_PAGE),
                Some(include_deleted),
            )
            .await
        },
    );

    // ── Clear selection when page or filters change ──────────────────────
    Effect::new(move || {
        let _ = page.try_get();
        let _ = selected_watch_id.try_get();
        let _ = show_deleted.try_get();
        selected_alerts.try_set(HashSet::new());
    });

    // ── Auto-expand alert if expanded_alert_id is provided ──────────────
    // Uses try_get() on both signals so the Effect silently no-ops if either
    // signal is accessed after the component's scope has been disposed (e.g.
    // the user navigated away while the alerts resource was still loading).
    Effect::new(move || {
        let Some(target_id) = expanded_alert_id.try_get().flatten() else { return };
        if let Some(Ok(page)) = alerts_resource.try_get().flatten() {
            let alerts = &page.alerts;
            let mut new_set = HashSet::new();
            new_set.insert(target_id);
            expanded_alerts.try_set(new_set);

            // Auto-mark as read
            if let Some(alert) = alerts.iter().find(|a| a.id == target_id)
                && alert.read_at.is_none() && alert.deleted_at.is_none()
            {
                leptos::task::spawn_local(async move {
                    let _ = mark_alert_read(target_id).await;
                    query_cache.try_invalidate("alerts");
                    query_cache.try_invalidate("unread_alerts");
                });
            }
        }
    });

    // ── Toggle expand ────────────────────────────────────────────────────
    let toggle_expanded = move |alert_id: i32| {
        expanded_alerts.update(|set| {
            if set.contains(&alert_id) {
                set.remove(&alert_id);
            } else {
                set.insert(alert_id);
                // Auto-mark as read when expanding an unread alert
                if let Some(Ok(page)) = alerts_resource.get() { let alerts = &page.alerts;
                    if let Some(alert) = alerts.iter().find(|a| a.id == alert_id)
                        && alert.read_at.is_none() && alert.deleted_at.is_none()
                    {
                        leptos::task::spawn_local(async move {
                            let _ = mark_alert_read(alert_id).await;
                            query_cache.try_invalidate("alerts");
                            query_cache.try_invalidate("unread_alerts");
                        });
                    }
                }
            }
        });
    };

    // ── Selection helpers ────────────────────────────────────────────────
    let toggle_alert_selection = move |alert_id: i32| {
        selected_alerts.update(|set| {
            if set.contains(&alert_id) {
                set.remove(&alert_id);
            } else {
                set.insert(alert_id);
            }
        });
    };

    let toggle_select_all = move |alerts: &[AlertItem]| {
        let selectable: Vec<i32> = alerts
            .iter()
            .filter(|a| a.deleted_at.is_none())
            .map(|a| a.id)
            .collect();

        selected_alerts.update(|set| {
            if set.len() == selectable.len() && !selectable.is_empty() {
                set.clear();
            } else {
                *set = selectable.into_iter().collect();
            }
        });
    };

    // ── Per-row actions (shared across rows) ──────────────────────────────
    // Declared here, in `AlertsHistory`'s stable top-level scope — NOT inside
    // the per-row `.map()` further down, which lives inside `<Transition>`'s
    // reactive children closure and is disposed + rebuilt every time
    // `alerts_resource` refetches. Every one of these four mutations
    // triggers exactly that refetch via `query_cache.invalidate("alerts")`,
    // so a per-row `Action` would be disposed the instant ANY row's mutation
    // (including a different row's, or a bulk action) completed — silently
    // dropping an in-flight result. One shared `Action` per mutation kind,
    // scoped like the bulk actions below, survives refetches.
    //
    // `mark_unread`/`delete`/`restore` are safe to share across concurrent
    // dispatches from different rows: their `Effect`s are alert-id-agnostic
    // (generic error log + blanket cache invalidation), so even if two
    // completions raced, neither write is meaningfully "lost" — the other's
    // `invalidate("alerts")` still refetches the state left by both.
    //
    // `continue_chat` is different: losing a completion means losing a
    // navigation (the server already created a chat session). Rather than
    // thread `alert_id` through the Action's output to reconstruct which row
    // resolved, dispatch is serialized — every trigger point (the dropdown's
    // "..." button, its mobile menu item, and the standalone desktop button)
    // is disabled while `continue_chat_action.pending()`, which makes a
    // second concurrent dispatch structurally impossible rather than merely
    // unlikely. A fully independent per-row in-flight state (so two
    // different alerts' "Continue in Chat" could run at once) would need
    // per-row `Action` instances that survive refetch — i.e. a keyed
    // `<For each=... key=|a| a.id>` — which is a larger structural change
    // than this ticket scoped.
    let continue_chat_action: Action<i32, Result<String, ServerFnError>> =
        Action::new(move |id: &i32| {
            let alert_id = *id;
            async move { continue_alert_in_chat(alert_id).await }
        });
    Effect::new(move |_| {
        if let Some(result) = continue_chat_action.value().get() {
            match result {
                Ok(session_id) => on_continue_chat.run(session_id),
                Err(e) => leptos::logging::error!("Failed to continue in chat: {e}"),
            }
        }
    });

    let mark_unread_action: Action<i32, Result<(), ServerFnError>> =
        Action::new(move |id: &i32| {
            let alert_id = *id;
            async move { mark_alert_unread(alert_id).await }
        });
    Effect::new(move |_| {
        if let Some(result) = mark_unread_action.value().get() {
            if let Err(e) = result {
                leptos::logging::error!("Failed to mark unread: {e}");
            }
            query_cache.invalidate("alerts");
            query_cache.invalidate("unread_alerts");
        }
    });

    let delete_action: Action<i32, Result<(), ServerFnError>> = Action::new(move |id: &i32| {
        let alert_id = *id;
        async move { delete_alert(alert_id).await }
    });
    Effect::new(move |_| {
        if let Some(result) = delete_action.value().get() {
            if let Err(e) = result {
                leptos::logging::error!("Failed to delete alert: {e}");
            }
            query_cache.invalidate("alerts");
            query_cache.invalidate("unread_alerts");
        }
    });

    let restore_action: Action<i32, Result<(), ServerFnError>> = Action::new(move |id: &i32| {
        let alert_id = *id;
        async move { restore_alert(alert_id).await }
    });
    Effect::new(move |_| {
        if let Some(result) = restore_action.value().get() {
            if let Err(e) = result {
                leptos::logging::error!("Failed to restore alert: {e}");
            }
            query_cache.invalidate("alerts");
            query_cache.invalidate("unread_alerts");
        }
    });

    // ── Bulk actions ─────────────────────────────────────────────────────
    // Each bulk mutation is an `Action` (KYO-226) instead of raw `spawn_local`
    // — the selected-id list is threaded through as the Action's input so the
    // eventual write reflects exactly what was dispatched, not whatever
    // `selected_alerts` happens to hold when the Effect fires. The three
    // `pending()` signals are OR'd together for the toolbar buttons' disabled
    // state, matching the old shared `bulk_action_pending` flag that disabled
    // all three buttons whenever any one bulk action was in flight.
    let bulk_mark_read_action: Action<Vec<i32>, Result<(), ServerFnError>> =
        Action::new(move |ids: &Vec<i32>| {
            let ids = ids.clone();
            async move { bulk_mark_alerts_read(ids).await }
        });
    Effect::new(move |_| {
        if let Some(result) = bulk_mark_read_action.value().get() {
            if let Err(e) = result {
                leptos::logging::error!("Bulk mark read failed: {e}");
            }
            selected_alerts.set(HashSet::new());
            query_cache.invalidate("alerts");
            query_cache.invalidate("unread_alerts");
        }
    });
    let handle_bulk_mark_read = move |_: web_sys::MouseEvent| {
        let ids: Vec<i32> = selected_alerts.get_untracked().into_iter().collect();
        if ids.is_empty() {
            return;
        }
        bulk_mark_read_action.dispatch(ids);
    };

    let bulk_mark_unread_action: Action<Vec<i32>, Result<(), ServerFnError>> =
        Action::new(move |ids: &Vec<i32>| {
            let ids = ids.clone();
            async move { bulk_mark_alerts_unread(ids).await }
        });
    Effect::new(move |_| {
        if let Some(result) = bulk_mark_unread_action.value().get() {
            if let Err(e) = result {
                leptos::logging::error!("Bulk mark unread failed: {e}");
            }
            selected_alerts.set(HashSet::new());
            query_cache.invalidate("alerts");
            query_cache.invalidate("unread_alerts");
        }
    });
    let handle_bulk_mark_unread = move |_: web_sys::MouseEvent| {
        let ids: Vec<i32> = selected_alerts.get_untracked().into_iter().collect();
        if ids.is_empty() {
            return;
        }
        bulk_mark_unread_action.dispatch(ids);
    };

    let bulk_delete_action: Action<Vec<i32>, Result<(), ServerFnError>> =
        Action::new(move |ids: &Vec<i32>| {
            let ids = ids.clone();
            async move { bulk_delete_alerts(ids).await }
        });
    Effect::new(move |_| {
        if let Some(result) = bulk_delete_action.value().get() {
            if let Err(e) = result {
                leptos::logging::error!("Bulk delete failed: {e}");
            }
            selected_alerts.set(HashSet::new());
            query_cache.invalidate("alerts");
            query_cache.invalidate("unread_alerts");
        }
    });
    let handle_bulk_delete = move |_: web_sys::MouseEvent| {
        let ids: Vec<i32> = selected_alerts.get_untracked().into_iter().collect();
        if ids.is_empty() {
            return;
        }

        // Confirm before destructive bulk action
        #[cfg(feature = "hydrate")]
        {
            let count = ids.len();
            let confirmed = web_sys::window()
                .and_then(|w| {
                    w.confirm_with_message(&format!(
                        "Delete {} selected alert{}? This can be undone from the deleted view.",
                        count,
                        if count == 1 { "" } else { "s" }
                    ))
                    .ok()
                })
                .unwrap_or(false);
            if !confirmed {
                return;
            }
        }

        bulk_delete_action.dispatch(ids);
    };

    // Combined pending state — any in-flight bulk action disables all three
    // bulk-action buttons, matching the old shared `bulk_action_pending` flag.
    let bulk_action_pending = Signal::derive(move || {
        bulk_mark_read_action.pending().get()
            || bulk_mark_unread_action.pending().get()
            || bulk_delete_action.pending().get()
    });

    let handle_clear_selection = move |_: web_sys::MouseEvent| {
        selected_alerts.set(HashSet::new());
    };

    // ── Watch options for the filter dropdown ────────────────────────────
    let watch_options = Memo::new(move |_| {
        let mut opts = vec![("_all".to_string(), "All watches".to_string())];
        if let Some(Ok(watches)) = watches_resource.get() {
            for w in watches {
                opts.push((w.watch_id.clone(), w.name.clone()));
            }
        }
        opts
    });

    let filter_value = Memo::new(move |_| {
        let wid = selected_watch_id.get();
        if wid.is_empty() {
            "_all".to_string()
        } else {
            wid
        }
    });

    // ── Render ───────────────────────────────────────────────────────────
    view! {
        // KYO-120: Chart info modal is a sibling of the alerts list so it
        // survives the Suspense fallback and stays mounted while a user
        // scrolls/pages through alerts. Matches `dashboard_viewer.rs` pattern.
        <ChartInfoModal
            open=Signal::derive(move || chart_info_open.get())
            yaml=chart_info_yaml
            on_close=on_chart_info_close
        />
        <Transition fallback=move || view! { <AlertsListSkeleton /> }>
            {move || {
                let alerts_result = alerts_resource.get();
                let watches_result = watches_resource.get();

                // Still loading — let <Transition> show the fallback skeleton.
                let alerts_result = alerts_result?;

                // Error state
                let alerts_page = match alerts_result {
                    Ok(data) => data,
                    Err(e) => {
                        return Some(view! {
                            <div class="rounded-lg border border-error-border bg-error text-error-foreground p-4 flex items-center gap-2">
                                <Icon icon=phosphor_leptos::WARNING_CIRCLE attr:class="h-4 w-4" />
                                <span>{format!("Failed to load alerts: {e}")}</span>
                            </div>
                        }.into_any());
                    }
                };

                let watches: Vec<WatchListItem> = watches_result
                    .and_then(|r| r.ok())
                    .unwrap_or_default();

                let alerts = alerts_page.alerts;
                let total = alerts_page.total;
                let selectable_alerts: Vec<&AlertItem> = alerts.iter().filter(|a| a.deleted_at.is_none()).collect();
                let selectable_count = selectable_alerts.len();
                let current_selected = selected_alerts.get();
                let has_selection = !current_selected.is_empty();

                let total_pages = ((total as f64) / (ALERTS_PER_PAGE as f64)).ceil() as i64;
                let has_more = (page.get() + 1) < total_pages;
                let has_previous = page.get() > 0;
                let current_page = page.get();

                // Is all selected?
                let _is_all_selected = selectable_count > 0 && current_selected.len() == selectable_count;

                // Render the full component
                Some(view! {
                    <div class="space-y-4">
                        // Toolbar — swaps between filters and bulk actions based on selection
                        <div class="flex items-center gap-3 flex-wrap min-h-10">
                            // Select-all checkbox — pl-4 aligns with per-card checkboxes below
                            {(selectable_count > 0).then(|| {
                                let _selectable_ids: Vec<i32> = selectable_alerts.iter().map(|a| a.id).collect();
                                view! {
                                    <div class="pl-4">
                                        <Checkbox
                                            checked=Signal::derive(move || {
                                                let sel = selected_alerts.get();
                                                let selectable: Vec<i32> = alerts_resource.get()
                                                    .and_then(|r| r.ok())
                                                    .map(|page| page.alerts)
                                                    .unwrap_or_default()
                                                    .iter()
                                                    .filter(|a| a.deleted_at.is_none())
                                                    .map(|a| a.id)
                                                    .collect();
                                                !selectable.is_empty() && sel.len() == selectable.len()
                                            })
                                            indeterminate=Signal::derive(move || {
                                                let sel = selected_alerts.get();
                                                let selectable_count = alerts_resource.get()
                                                    .and_then(|r| r.ok())
                                                    .map(|page| page.alerts)
                                                    .unwrap_or_default()
                                                    .iter()
                                                    .filter(|a| a.deleted_at.is_none())
                                                    .count();
                                                !sel.is_empty() && sel.len() < selectable_count
                                            })
                                            on_change=Callback::new(move |_checked: bool| {
                                                if let Some(Ok(page)) = alerts_resource.get() { let alerts = &page.alerts;
                                                    toggle_select_all(alerts);
                                                }
                                            })
                                        />
                                    </div>
                                }
                            })}

                            {if has_selection {
                                // Selection active: show count + bulk actions + cancel
                                // Matches Chats list bulk action bar pattern.
                                let count = current_selected.len();
                                view! {
                                    <span class="text-sm font-medium text-foreground whitespace-nowrap">
                                        {format!("{count} selected")}
                                    </span>
                                    <Button
                                        variant=ButtonVariant::Ghost
                                        size=ButtonSize::Sm
                                        disabled=bulk_action_pending
                                        on:click=handle_bulk_mark_read
                                    >
                                        <Icon icon=phosphor_leptos::ENVELOPE_OPEN attr:class="h-4 w-4 sm:mr-1.5" />
                                        <span class="hidden sm:inline">"Mark Read"</span>
                                    </Button>
                                    <Button
                                        variant=ButtonVariant::Ghost
                                        size=ButtonSize::Sm
                                        disabled=bulk_action_pending
                                        on:click=handle_bulk_mark_unread
                                    >
                                        <Icon icon=phosphor_leptos::ENVELOPE attr:class="h-4 w-4 sm:mr-1.5" />
                                        <span class="hidden sm:inline">"Mark Unread"</span>
                                    </Button>
                                    <Button
                                        variant=ButtonVariant::GhostDestructive
                                        size=ButtonSize::Sm
                                        disabled=bulk_action_pending
                                        on:click=handle_bulk_delete
                                    >
                                        <Icon icon=phosphor_leptos::TRASH attr:class="h-4 w-4 sm:mr-1.5" />
                                        <span class="hidden sm:inline">"Delete"</span>
                                    </Button>
                                    <Button
                                        variant=ButtonVariant::GhostMuted
                                        size=ButtonSize::Sm
                                        class="ml-auto"
                                        on:click=handle_clear_selection
                                    >
                                        "Cancel"
                                    </Button>
                                }.into_any()
                            } else {
                                // No selection: show filters
                                view! {
                                    <div class="flex items-center gap-2 min-w-0 flex-1 sm:flex-none">
                                        <Label class="text-muted-foreground hidden sm:inline">"Filter:"</Label>
                                        <div class="w-full sm:w-[200px]">
                                            <Select
                                                value=Signal::derive(move || filter_value.get())
                                                options=Signal::derive(move || watch_options.get())
                                                on_change=move |val: String| {
                                                    if val == "_all" {
                                                        selected_watch_id.set(String::new());
                                                    } else {
                                                        selected_watch_id.set(val);
                                                    }
                                                    page.set(0);
                                                }
                                                placeholder="All watches".to_string()
                                            />
                                        </div>
                                    </div>
                                    <Badge variant=BadgeVariant::Secondary class="hidden sm:inline-flex">
                                        {format!("{} alert{}", total, if total != 1 { "s" } else { "" })}
                                    </Badge>
                                    <div class="ml-auto">
                                        <Switch
                                            checked=Signal::derive(move || show_deleted.get())
                                            on_change=Callback::new(move |checked: bool| {
                                                show_deleted.set(checked);
                                                page.set(0);
                                            })
                                            label="Show deleted"
                                        />
                                    </div>
                                }.into_any()
                            }}
                        </div>

                        // Alerts list
                        {if alerts.is_empty() {
                            view! {
                                <EmptyState
                                    icon=std::sync::Arc::new(|| view! { <Icon icon=phosphor_leptos::BELL_RINGING weight=IconWeight::Duotone size="64px" /> }.into_any())
                                    title="No alerts yet"
                                    description="When your watches detect something noteworthy, alerts will appear here."
                                />
                            }.into_any()
                        } else {
                            let watches_for_cards = watches.clone();
                            view! {
                                <div class="max-w-4xl mx-auto">
                                <div class="space-y-2">
                                    {alerts.into_iter().map(|alert| {
                                        let alert_id = alert.id;
                                        let is_deleted = alert.deleted_at.is_some();
                                        let is_unread = alert.read_at.is_none() && !is_deleted;
                                        let is_selected = current_selected.contains(&alert_id);
                                        let watch_name = get_watch_name(&alert, &watches_for_cards);
                                        let mode = alert.mode.clone().unwrap_or_default();
                                        let is_report = mode == "report";
                                        let started_at = format_date(&alert.started_at);

                                        // Alert title from execution_trace or watch name
                                        let alert_title = alert.execution_trace
                                            .as_ref()
                                            .and_then(|t| t.get("alert_title"))
                                            .and_then(|v| v.as_str())
                                            .map(String::from)
                                            .unwrap_or_else(|| watch_name.clone());

                                        // Summary from execution_trace
                                        let summary = alert.execution_trace
                                            .as_ref()
                                            .and_then(|t| t.get("summary"))
                                            .and_then(|v| v.as_str())
                                            .map(String::from);

                                        // Agent response for expanded content
                                        let agent_response = alert.agent_response.clone().unwrap_or_default();

                                        // Card classes — matches chat_list.rs pattern:
                                        // no shadow by default, hover:shadow-sm for
                                        // interactive lift.
                                        let card_class = format!(
                                            "rounded-lg border overflow-hidden hover:shadow-sm transition-all {}{}",
                                            if is_deleted {
                                                "opacity-60 border-border bg-muted/30"
                                            } else if is_unread {
                                                "border-l-4 border-l-primary border-y-border border-r-border bg-primary/10"
                                            } else {
                                                "border-border bg-card"
                                            },
                                            if is_selected { " border-primary/50 ring-2 ring-primary/20" } else { "" }
                                        );

                                        view! {
                                            <div class=card_class>
                                                // Alert header
                                                <div class="flex items-center min-w-0">
                                                    // Checkbox — always visible for non-deleted alerts
                                                    {(!is_deleted).then(|| view! {
                                                        <div class="pl-3 sm:pl-4 flex items-center shrink-0">
                                                            <Checkbox
                                                                checked=Signal::derive(move || selected_alerts.get().contains(&alert_id))
                                                                on_change=Callback::new(move |_: bool| {
                                                                    toggle_alert_selection(alert_id);
                                                                })
                                                            />
                                                        </div>
                                                    })}

                                                    // Clickable area to expand
                                                    <button
                                                        class=format!(
                                                            "flex-1 min-w-0 p-4 flex items-center justify-between hover:bg-muted/50 transition-colors text-left {}",
                                                            if !is_deleted { "pl-2" } else { "" }
                                                        )
                                                        on:click=move |_| toggle_expanded(alert_id)
                                                    >
                                                        <div class="flex items-center gap-2 min-w-0 flex-1">
                                                            // Mode icon
                                                            {if is_report {
                                                                let icon_class = format!(
                                                                    "h-5 w-5 shrink-0 hidden sm:block {}",
                                                                    if is_unread { "text-primary" } else { "text-muted-foreground" }
                                                                );
                                                                view! { <Icon icon=phosphor_leptos::CHART_BAR attr:class=icon_class /> }.into_any()
                                                            } else {
                                                                let icon_class = format!(
                                                                    "h-5 w-5 shrink-0 hidden sm:block {}",
                                                                    if is_unread { "text-primary" } else { "text-muted-foreground" }
                                                                );
                                                                view! { <Icon icon=phosphor_leptos::BELL attr:class=icon_class /> }.into_any()
                                                            }}
                                                            <div class="min-w-0 flex-1">
                                                                <div class="flex items-center gap-2">
                                                                    <span class=format!(
                                                                        "font-display text-foreground truncate {}",
                                                                        if is_unread { "font-semibold" } else { "font-medium" }
                                                                    )>
                                                                        {alert_title}
                                                                    </span>
                                                                    {is_deleted.then(|| view! {
                                                                        <Badge variant=BadgeVariant::Secondary class="text-xs shrink-0">
                                                                            "Deleted"
                                                                        </Badge>
                                                                    })}
                                                                </div>
                                                                {summary.map(|s| view! {
                                                                    <p class="text-sm text-muted-foreground truncate mt-0.5">
                                                                        {s}
                                                                    </p>
                                                                })}
                                                                <div class="flex items-center gap-2 text-xs text-muted-foreground mt-0.5 min-w-0">
                                                                    <span class="truncate">{watch_name}</span>
                                                                    <span class="shrink-0">{"\u{2022}"}</span>
                                                                    <Icon icon=phosphor_leptos::CLOCK attr:class="h-3 w-3 shrink-0" />
                                                                    <span class="shrink-0">{started_at}</span>
                                                                </div>
                                                            </div>
                                                        </div>
                                                        // Expand chevron
                                                        {move || {
                                                            if expanded_alerts.get().contains(&alert_id) {
                                                                view! { <Icon icon=phosphor_leptos::CARET_UP attr:class="h-5 w-5 text-muted-foreground shrink-0 ml-1" /> }.into_any()
                                                            } else {
                                                                view! { <Icon icon=phosphor_leptos::CARET_DOWN attr:class="h-5 w-5 text-muted-foreground shrink-0 ml-1" /> }.into_any()
                                                            }
                                                        }}
                                                    </button>

                                                    // Action buttons
                                                    <div class="pr-2 sm:pr-3 flex items-center shrink-0">
                                                        // Continue in Chat button — hidden on mobile, shown in dropdown instead.
                                                        // Disabled state and spinner both key off the shared
                                                        // `continue_chat_action`'s `pending()` (KYO-226 follow-up) rather
                                                        // than a hand-maintained "which alert id is busy" signal — the
                                                        // action's own machinery is the single source of truth, and it
                                                        // can't get stuck the way a manually-reset signal could if its
                                                        // resetting `Effect` were ever disposed mid-flight.
                                                        <button
                                                            class="hidden sm:inline-flex items-center justify-center gap-2 whitespace-nowrap rounded-md text-sm font-medium transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:pointer-events-none disabled:opacity-50 [&_svg]:pointer-events-none [&_svg]:size-4 [&_svg]:shrink-0 text-foreground hover:bg-secondary hover:text-accent-foreground h-8 rounded-md px-3 text-xs"
                                                            disabled=Signal::derive(move || continue_chat_action.pending().get())
                                                            title="Continue in Chat"
                                                            on:click=move |_| {
                                                                if !continue_chat_action.pending().get_untracked() {
                                                                    continue_chat_action.dispatch(alert_id);
                                                                }
                                                            }
                                                        >
                                                            {move || {
                                                                if continue_chat_action.pending().get() {
                                                                    view! { <Spinner /> }.into_any()
                                                                } else {
                                                                    view! { <Icon icon=phosphor_leptos::CHATS attr:class="h-4 w-4" /> }.into_any()
                                                                }
                                                            }}
                                                        </button>

                                                        // More actions dropdown
                                                        <AlertDropdownMenu
                                                            is_deleted=is_deleted
                                                            is_unread=is_unread
                                                            alert_id=alert_id
                                                            continue_chat_action=continue_chat_action
                                                            mark_unread_action=mark_unread_action
                                                            delete_action=delete_action
                                                            restore_action=restore_action
                                                        />
                                                    </div>
                                                </div>

                                                // Alert content (expanded)
                                                {move || {
                                                    let agent_response = agent_response.clone();
                                                    expanded_alerts.get().contains(&alert_id).then(|| {
                                                        // KYO-119: Pass workspace_id so `MarkdownRenderer`
                                                        // can register `KyomiDatasourceProvider` on its
                                                        // owner — the alerts inbox mounts outside
                                                        // `DashboardChartProviders`, so chartml blocks
                                                        // with `data: { datasource, query }` would
                                                        // otherwise fail with "no provider registered
                                                        // for kind 'datasource'". Empty string skips
                                                        // registration when the user context hasn't
                                                        // loaded a workspace yet.
                                                        let ws_id = workspace_id.get().unwrap_or_default();
                                                        view! {
                                                            <div class="px-3 sm:px-4 py-3 border-t border-border bg-muted/30 overflow-x-auto">
                                                                <MarkdownRenderer
                                                                    content=Signal::derive(move || agent_response.clone())
                                                                    workspace_id=ws_id
                                                                    on_chart_info=on_chart_info
                                                                    on_ask_about_chart=on_ask_about_chart
                                                                />
                                                            </div>
                                                        }
                                                    })
                                                }}
                                            </div>
                                        }
                                    }).collect_view()}
                                </div>
                                </div>
                            }.into_any()
                        }}

                        // Pagination
                        {(has_more || has_previous).then(|| {
                            view! {
                                <div class="flex items-center justify-between pt-4 border-t border-border">
                                    <Button
                                        variant=ButtonVariant::Ghost
                                        size=ButtonSize::Sm
                                        disabled=!has_previous
                                        on:click=move |_| page.update(|p| *p -= 1)
                                    >
                                        "Previous"
                                    </Button>
                                    <span class="text-sm text-muted-foreground">
                                        {format!("Page {} of {}", current_page + 1, total_pages.max(1))}
                                    </span>
                                    <Button
                                        variant=ButtonVariant::Ghost
                                        size=ButtonSize::Sm
                                        disabled=!has_more
                                        on:click=move |_| page.update(|p| *p += 1)
                                    >
                                        "Next"
                                    </Button>
                                </div>
                            }
                        })}
                    </div>
                }.into_any())
            }}
        </Transition>
    }
}
