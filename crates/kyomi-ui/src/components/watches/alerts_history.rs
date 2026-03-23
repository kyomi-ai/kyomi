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

use crate::components::{
    Badge, BadgeVariant, Button, ButtonSize, ButtonVariant, Checkbox, DynSelect, Label, Spinner,
    Switch,
};
use crate::server_fns::watches::{
    bulk_delete_alerts, bulk_mark_alerts_read, bulk_mark_alerts_unread, continue_alert_in_chat,
    delete_alert, get_alerts, list_watches, mark_alert_read, mark_alert_unread, restore_alert,
};
use crate::types::{AlertItem, WatchListItem};

// ─── Constants ──────────────────────────────────────────────────────────────

const ALERTS_PER_PAGE: i64 = 20;

// ─── SVG Icons ──────────────────────────────────────────────────────────────

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

/// ChartBar icon (Heroicons outline).
#[component]
fn ChartBarIcon(#[prop(into, optional)] class: String) -> impl IntoView {
    view! {
        <svg class=class xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 24 24" stroke-width="1.5" stroke="currentColor">
            <path stroke-linecap="round" stroke-linejoin="round" d="M3 13.125C3 12.504 3.504 12 4.125 12h2.25c.621 0 1.125.504 1.125 1.125v6.75C7.5 20.496 6.996 21 6.375 21h-2.25A1.125 1.125 0 0 1 3 19.875v-6.75ZM9.75 8.625c0-.621.504-1.125 1.125-1.125h2.25c.621 0 1.125.504 1.125 1.125v11.25c0 .621-.504 1.125-1.125 1.125h-2.25a1.125 1.125 0 0 1-1.125-1.125V8.625ZM16.5 4.125c0-.621.504-1.125 1.125-1.125h2.25C20.496 3 21 3.504 21 4.125v15.75c0 .621-.504 1.125-1.125 1.125h-2.25a1.125 1.125 0 0 1-1.125-1.125V4.125Z" />
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

/// ChevronDown icon (Lucide).
#[component]
fn ChevronDownIcon(#[prop(into, optional)] class: String) -> impl IntoView {
    view! {
        <svg class=class xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <path d="m6 9 6 6 6-6" />
        </svg>
    }
}

/// ChevronUp icon (Lucide).
#[component]
fn ChevronUpIcon(#[prop(into, optional)] class: String) -> impl IntoView {
    view! {
        <svg class=class xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <path d="m18 15-6-6-6 6" />
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

/// Undo2 icon (Lucide).
#[component]
fn UndoIcon(#[prop(into, optional)] class: String) -> impl IntoView {
    view! {
        <svg class=class xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <path d="M9 14 4 9l5-5" />
            <path d="M4 9h10.5a5.5 5.5 0 0 1 5.5 5.5a5.5 5.5 0 0 1-5.5 5.5H11" />
        </svg>
    }
}

/// MailOpen icon (Lucide).
#[component]
fn MailOpenIcon(#[prop(into, optional)] class: String) -> impl IntoView {
    view! {
        <svg class=class xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <path d="M21.2 8.4c.5.38.8.97.8 1.6v10a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V10a2 2 0 0 1 .8-1.6l8-6a2 2 0 0 1 2.4 0l8 6Z" />
            <path d="m22 10-8.97 5.7a1.94 1.94 0 0 1-2.06 0L2 10" />
        </svg>
    }
}

/// Mail icon (Lucide).
#[component]
fn MailIcon(#[prop(into, optional)] class: String) -> impl IntoView {
    view! {
        <svg class=class xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <rect width="20" height="16" x="2" y="4" rx="2" />
            <path d="m22 7-8.97 5.7a1.94 1.94 0 0 1-2.06 0L2 7" />
        </svg>
    }
}

/// MoreVertical icon (Lucide).
#[component]
fn MoreVerticalIcon(#[prop(into, optional)] class: String) -> impl IntoView {
    view! {
        <svg class=class xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <circle cx="12" cy="12" r="1" />
            <circle cx="12" cy="5" r="1" />
            <circle cx="12" cy="19" r="1" />
        </svg>
    }
}

/// X icon (Lucide).
#[component]
fn XIcon(#[prop(into, optional)] class: String) -> impl IntoView {
    view! {
        <svg class=class xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <path d="M18 6 6 18" />
            <path d="m6 6 12 12" />
        </svg>
    }
}

/// ChatBubbleLeftRight icon (Heroicons outline).
#[component]
fn ChatBubbleIcon(#[prop(into, optional)] class: String) -> impl IntoView {
    view! {
        <svg class=class xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 24 24" stroke-width="1.5" stroke="currentColor">
            <path stroke-linecap="round" stroke-linejoin="round" d="M20.25 8.511c.884.284 1.5 1.128 1.5 2.097v4.286c0 1.136-.847 2.1-1.98 2.193-.34.027-.68.052-1.02.072v3.091l-3-3c-1.354 0-2.694-.055-4.02-.163a2.115 2.115 0 0 1-.825-.242m9.345-8.334a2.126 2.126 0 0 0-.476-.095 48.64 48.64 0 0 0-8.048 0c-1.131.094-1.976 1.057-1.976 2.192v4.286c0 .837.46 1.58 1.155 1.951m9.345-8.334V6.637c0-1.621-1.152-3.026-2.76-3.235A48.455 48.455 0 0 0 11.25 3c-2.115 0-4.198.137-6.24.402-1.608.209-2.76 1.614-2.76 3.235v6.226c0 1.621 1.152 3.026 2.76 3.235.577.075 1.157.14 1.74.194V21l4.155-4.155" />
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

// ─── Dropdown Menu ──────────────────────────────────────────────────────────

/// Simple dropdown menu for per-alert actions.
///
/// Opens on click, closes on click-outside or item selection.
#[component]
fn AlertDropdownMenu(
    /// Whether this alert is deleted.
    is_deleted: bool,
    /// Whether this alert is unread (non-deleted, no read_at).
    is_unread: bool,
    /// Alert execution ID.
    alert_id: i32,
    /// Signal to trigger a data refetch after mutations.
    refetch_trigger: RwSignal<u32>,
    /// Callback to handle "Continue in Chat" with the session_id.
    on_continue_chat: Callback<String>,
) -> impl IntoView {
    let (is_open, set_is_open) = signal(false);
    let container_ref = NodeRef::<leptos::html::Div>::new();

    // Click-outside detection
    #[cfg(target_arch = "wasm32")]
    {
        use send_wrapper::SendWrapper;
        use wasm_bindgen::prelude::*;

        let cleanup: StoredValue<Option<SendWrapper<Box<dyn FnOnce()>>>> =
            StoredValue::new(None);

        Effect::new(move |_| {
            if let Some(teardown) = cleanup.try_update_value(|v| v.take()).flatten() {
                teardown.take()();
            }

            if is_open.get() {
                let window = web_sys::window().expect("window");
                let container_el = container_ref.get();

                let cb = Closure::<dyn Fn(web_sys::Event)>::new(move |ev: web_sys::Event| {
                    if let Some(target) = ev.target() {
                        let target_node: web_sys::Node = target.unchecked_into();
                        if let Some(ref el) = container_el {
                            let html_el: &web_sys::HtmlElement = el;
                            let node: &web_sys::Node = html_el.as_ref();
                            if !node.contains(Some(&target_node)) {
                                set_is_open.set(false);
                            }
                        } else {
                            set_is_open.set(false);
                        }
                    }
                });

                let _ = window.add_event_listener_with_callback_and_bool(
                    "click",
                    cb.as_ref().unchecked_ref(),
                    true,
                );

                let window_clone = window.clone();
                let cb_ref: js_sys::Function =
                    cb.as_ref().unchecked_ref::<js_sys::Function>().clone();
                let teardown: Box<dyn FnOnce()> = Box::new(move || {
                    let _ = window_clone.remove_event_listener_with_callback_and_bool(
                        "click",
                        &cb_ref,
                        true,
                    );
                    drop(cb);
                });
                cleanup.set_value(Some(SendWrapper::new(teardown)));
            }
        });

        on_cleanup(move || {
            if let Some(teardown) = cleanup.try_update_value(|v| v.take()).flatten() {
                teardown.take()();
            }
        });
    }

    let (action_pending, set_action_pending) = signal(false);

    // Continue in Chat handler
    let handle_continue_chat = move |_: web_sys::MouseEvent| {
        set_action_pending.set(true);
        set_is_open.set(false);
        leptos::task::spawn_local(async move {
            match continue_alert_in_chat(alert_id).await {
                Ok(session_id) => {
                    on_continue_chat.run(session_id);
                }
                Err(e) => {
                    leptos::logging::error!("Failed to continue in chat: {e}");
                }
            }
            set_action_pending.set(false);
        });
    };

    // Mark as unread handler
    let handle_mark_unread = move |_: web_sys::MouseEvent| {
        set_is_open.set(false);
        leptos::task::spawn_local(async move {
            if let Err(e) = mark_alert_unread(alert_id).await {
                leptos::logging::error!("Failed to mark unread: {e}");
            }
            refetch_trigger.update(|v| *v += 1);
        });
    };

    // Delete handler
    let handle_delete = move |_: web_sys::MouseEvent| {
        set_is_open.set(false);
        leptos::task::spawn_local(async move {
            if let Err(e) = delete_alert(alert_id).await {
                leptos::logging::error!("Failed to delete alert: {e}");
            }
            refetch_trigger.update(|v| *v += 1);
        });
    };

    // Restore handler
    let handle_restore = move |_: web_sys::MouseEvent| {
        set_is_open.set(false);
        leptos::task::spawn_local(async move {
            if let Err(e) = restore_alert(alert_id).await {
                leptos::logging::error!("Failed to restore alert: {e}");
            }
            refetch_trigger.update(|v| *v += 1);
        });
    };

    view! {
        <div node_ref=container_ref class="relative">
            // Trigger button
            <button
                class="inline-flex items-center justify-center gap-2 whitespace-nowrap rounded-md text-sm font-medium transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:pointer-events-none disabled:opacity-50 [&_svg]:pointer-events-none [&_svg]:size-4 [&_svg]:shrink-0 text-foreground hover:bg-accent hover:text-accent-foreground h-8 rounded-md px-3 text-xs"
                on:click=move |_| set_is_open.update(|v| *v = !*v)
                disabled=action_pending
            >
                <MoreVerticalIcon class="h-4 w-4" />
            </button>

            // Dropdown content
            {move || is_open.get().then(|| {
                view! {
                    <div class="absolute right-0 top-full mt-1 z-[1100] min-w-[10rem] rounded-md border border-border bg-popover text-popover-foreground shadow-md p-1">
                        // Continue in Chat — visible in dropdown on mobile
                        <button
                            class="relative flex w-full cursor-default select-none items-center rounded-sm py-1.5 px-2 text-sm outline-none hover:bg-accent hover:text-accent-foreground sm:hidden"
                            on:click=handle_continue_chat
                        >
                            <ChatBubbleIcon class="h-4 w-4 mr-2" />
                            "Continue in Chat"
                        </button>

                        // Mark as unread (only for read, non-deleted alerts)
                        {(!is_unread && !is_deleted).then(|| view! {
                            <button
                                class="relative flex w-full cursor-default select-none items-center rounded-sm py-1.5 px-2 text-sm outline-none hover:bg-accent hover:text-accent-foreground"
                                on:click=handle_mark_unread
                            >
                                <MailOpenIcon class="h-4 w-4 mr-2" />
                                "Mark as unread"
                            </button>
                        })}

                        // Delete/Restore
                        {if is_deleted {
                            view! {
                                <button
                                    class="relative flex w-full cursor-default select-none items-center rounded-sm py-1.5 px-2 text-sm outline-none hover:bg-accent hover:text-accent-foreground"
                                    on:click=handle_restore
                                >
                                    <UndoIcon class="h-4 w-4 mr-2" />
                                    "Restore"
                                </button>
                            }.into_any()
                        } else {
                            view! {
                                <button
                                    class="relative flex w-full cursor-default select-none items-center rounded-sm py-1.5 px-2 text-sm outline-none hover:bg-accent hover:text-accent-foreground text-destructive"
                                    on:click=handle_delete
                                >
                                    <TrashIcon class="h-4 w-4 mr-2" />
                                    "Delete"
                                </button>
                            }.into_any()
                        }}
                    </div>
                }
            })}
        </div>
    }
}

// ─── Helper functions ───────────────────────────────────────────────────────

/// Format a date string for display.
/// Matches React: `date.toLocaleString(undefined, { month: 'short', day: 'numeric', year: 'numeric', hour: '2-digit', minute: '2-digit' })`.
fn format_date(date_str: &str) -> String {
    // Simple ISO 8601 parsing — extract date and time parts.
    // Full `Intl.DateTimeFormat` is not available server-side; we do a
    // best-effort format. On the client this is fine since the actual data
    // is just displayed as text.
    if date_str.is_empty() {
        return String::new();
    }

    // Try to parse "2026-03-15T10:30:00+00:00" or similar
    let date_part = date_str.split('T').next().unwrap_or(date_str);
    let time_part = date_str
        .split('T')
        .nth(1)
        .unwrap_or("")
        .split('+')
        .next()
        .unwrap_or("")
        .split('Z')
        .next()
        .unwrap_or("");

    let parts: Vec<&str> = date_part.split('-').collect();
    if parts.len() < 3 {
        return date_str.to_string();
    }

    let month = match parts.get(1).copied().unwrap_or("01") {
        "01" => "Jan",
        "02" => "Feb",
        "03" => "Mar",
        "04" => "Apr",
        "05" => "May",
        "06" => "Jun",
        "07" => "Jul",
        "08" => "Aug",
        "09" => "Sep",
        "10" => "Oct",
        "11" => "Nov",
        "12" => "Dec",
        _ => "???",
    };

    let day = parts.get(2).copied().unwrap_or("01");
    let year = parts.first().copied().unwrap_or("2026");

    let time_parts: Vec<&str> = time_part.split(':').collect();
    let hour = time_parts.first().copied().unwrap_or("00");
    let minute = time_parts.get(1).copied().unwrap_or("00");

    format!("{month} {day}, {year}, {hour}:{minute}")
}

/// Get the display name for a watch from an alert, looking up in the watches list if needed.
fn get_watch_name(alert: &AlertItem, watches: &[WatchListItem]) -> String {
    if let Some(ref name) = alert.watch_name {
        if !name.is_empty() {
            return name.clone();
        }
    }
    if let Some(ref watch_id) = alert.watch_id {
        if let Some(watch) = watches.iter().find(|w| &w.watch_id == watch_id) {
            return watch.name.clone();
        }
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
    /// Optional pre-expanded alert ID (from URL params).
    expanded_alert_id: Option<i32>,
    /// Called when user clicks "Continue in Chat" on an alert.
    on_continue_chat: Callback<String>,
) -> impl IntoView {
    // ── State ────────────────────────────────────────────────────────────
    let selected_watch_id = RwSignal::new(String::new());
    let expanded_alerts = RwSignal::new(HashSet::<i32>::new());
    let show_deleted = RwSignal::new(false);
    let page = RwSignal::new(0i64);
    let selected_alerts = RwSignal::new(HashSet::<i32>::new());
    let (bulk_action_pending, set_bulk_action_pending) = signal(false);

    // Trigger for refetching data after mutations.
    let refetch_trigger = RwSignal::new(0u32);

    // ── Resources ────────────────────────────────────────────────────────

    // Fetch watches for filter dropdown
    let watches_resource = Resource::new(|| (), move |_| async move { list_watches().await });

    // Fetch alerts history (reactive to filter/page changes + refetch trigger)
    let alerts_resource = Resource::new(
        move || {
            (
                selected_watch_id.get(),
                page.get(),
                show_deleted.get(),
                refetch_trigger.get(),
            )
        },
        move |(watch_id, current_page, include_deleted, _trigger)| async move {
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
        let _ = page.get();
        let _ = selected_watch_id.get();
        let _ = show_deleted.get();
        selected_alerts.set(HashSet::new());
    });

    // ── Auto-expand alert if expanded_alert_id is provided ──────────────
    if let Some(target_id) = expanded_alert_id {
        Effect::new(move || {
            let _ = refetch_trigger.get(); // re-run when data changes
            if let Some(Ok(page)) = alerts_resource.get() { let alerts = &page.alerts;
                let mut new_set = HashSet::new();
                new_set.insert(target_id);
                expanded_alerts.set(new_set);

                // Auto-mark as read
                if let Some(alert) = alerts.iter().find(|a| a.id == target_id) {
                    if alert.read_at.is_none() && alert.deleted_at.is_none() {
                        let trigger = refetch_trigger;
                        leptos::task::spawn_local(async move {
                            let _ = mark_alert_read(target_id).await;
                            trigger.update(|v| *v += 1);
                        });
                    }
                }
            }
        });
    }

    // ── Toggle expand ────────────────────────────────────────────────────
    let toggle_expanded = move |alert_id: i32| {
        expanded_alerts.update(|set| {
            if set.contains(&alert_id) {
                set.remove(&alert_id);
            } else {
                set.insert(alert_id);
                // Auto-mark as read when expanding an unread alert
                if let Some(Ok(page)) = alerts_resource.get() { let alerts = &page.alerts;
                    if let Some(alert) = alerts.iter().find(|a| a.id == alert_id) {
                        if alert.read_at.is_none() && alert.deleted_at.is_none() {
                            let trigger = refetch_trigger;
                            leptos::task::spawn_local(async move {
                                let _ = mark_alert_read(alert_id).await;
                                trigger.update(|v| *v += 1);
                            });
                        }
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

    // ── Bulk actions ─────────────────────────────────────────────────────
    let handle_bulk_mark_read = move |_: web_sys::MouseEvent| {
        let ids: Vec<i32> = selected_alerts.get_untracked().into_iter().collect();
        if ids.is_empty() {
            return;
        }
        set_bulk_action_pending.set(true);
        leptos::task::spawn_local(async move {
            if let Err(e) = bulk_mark_alerts_read(ids).await {
                leptos::logging::error!("Bulk mark read failed: {e}");
            }
            selected_alerts.set(HashSet::new());
            set_bulk_action_pending.set(false);
            refetch_trigger.update(|v| *v += 1);
        });
    };

    let handle_bulk_mark_unread = move |_: web_sys::MouseEvent| {
        let ids: Vec<i32> = selected_alerts.get_untracked().into_iter().collect();
        if ids.is_empty() {
            return;
        }
        set_bulk_action_pending.set(true);
        leptos::task::spawn_local(async move {
            if let Err(e) = bulk_mark_alerts_unread(ids).await {
                leptos::logging::error!("Bulk mark unread failed: {e}");
            }
            selected_alerts.set(HashSet::new());
            set_bulk_action_pending.set(false);
            refetch_trigger.update(|v| *v += 1);
        });
    };

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

        set_bulk_action_pending.set(true);
        leptos::task::spawn_local(async move {
            if let Err(e) = bulk_delete_alerts(ids).await {
                leptos::logging::error!("Bulk delete failed: {e}");
            }
            selected_alerts.set(HashSet::new());
            set_bulk_action_pending.set(false);
            refetch_trigger.update(|v| *v += 1);
        });
    };

    let handle_clear_selection = move |_: web_sys::MouseEvent| {
        selected_alerts.set(HashSet::new());
    };

    // ── Continue in Chat — tracks which alert ID is currently pending ────
    let (continue_chat_alert_id, set_continue_chat_alert_id) = signal(Option::<i32>::None);

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
        <Suspense fallback=move || view! {
            <div class="flex items-center justify-center py-12">
                <Spinner class="text-muted-foreground" />
            </div>
        }>
            {move || {
                let alerts_result = alerts_resource.get();
                let watches_result = watches_resource.get();

                // Loading state
                let alerts_page = match alerts_result {
                    Some(Ok(data)) => data,
                    Some(Err(e)) => {
                        return view! {
                            <div class="rounded-lg border border-error-border bg-error text-error-foreground p-4 flex items-center gap-2">
                                <AlertCircleIcon class="h-4 w-4" />
                                <span>{format!("Failed to load alerts: {e}")}</span>
                            </div>
                        }.into_any();
                    }
                    None => {
                        return view! {
                            <div class="flex items-center justify-center py-12">
                                <Spinner class="text-muted-foreground" />
                            </div>
                        }.into_any();
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
                view! {
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
                                                    toggle_select_all(&alerts);
                                                }
                                            })
                                        />
                                    </div>
                                }
                            })}

                            {if has_selection {
                                // Selection active: show count + bulk actions + cancel
                                let count = current_selected.len();
                                view! {
                                    <span class="text-sm font-medium text-foreground whitespace-nowrap">
                                        {format!("{count} selected")}
                                    </span>
                                    <div class="flex items-center gap-1">
                                        <Button
                                            variant=ButtonVariant::Ghost
                                            size=ButtonSize::Sm
                                            disabled=bulk_action_pending.get()
                                            on:click=handle_bulk_mark_read
                                        >
                                            <MailOpenIcon class="h-4 w-4 sm:mr-1.5" />
                                            <span class="hidden sm:inline">"Mark Read"</span>
                                        </Button>
                                        <Button
                                            variant=ButtonVariant::Ghost
                                            size=ButtonSize::Sm
                                            disabled=bulk_action_pending.get()
                                            on:click=handle_bulk_mark_unread
                                        >
                                            <MailIcon class="h-4 w-4 sm:mr-1.5" />
                                            <span class="hidden sm:inline">"Mark Unread"</span>
                                        </Button>
                                        <Button
                                            variant=ButtonVariant::Ghost
                                            size=ButtonSize::Sm
                                            class="text-destructive hover:text-destructive"
                                            disabled=bulk_action_pending.get()
                                            on:click=handle_bulk_delete
                                        >
                                            <TrashIcon class="h-4 w-4 sm:mr-1.5" />
                                            <span class="hidden sm:inline">"Delete"</span>
                                        </Button>
                                    </div>
                                    <Button
                                        variant=ButtonVariant::Ghost
                                        size=ButtonSize::Sm
                                        class="ml-auto"
                                        on:click=handle_clear_selection
                                    >
                                        <XIcon class="h-4 w-4 sm:mr-1.5" />
                                        <span class="hidden sm:inline">"Cancel"</span>
                                    </Button>
                                }.into_any()
                            } else {
                                // No selection: show filters
                                view! {
                                    <div class="flex items-center gap-2 min-w-0 flex-1 sm:flex-none">
                                        <Label class="text-muted-foreground hidden sm:inline">"Filter:"</Label>
                                        <div class="w-full sm:w-[200px]">
                                            <DynSelect
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
                                    <div class="flex items-center gap-2 ml-auto">
                                        <Switch
                                            checked=Signal::derive(move || show_deleted.get())
                                            on_change=Callback::new(move |checked: bool| {
                                                show_deleted.set(checked);
                                                page.set(0);
                                            })
                                        />
                                        <Label class="text-sm text-muted-foreground cursor-pointer">
                                            "Show deleted"
                                        </Label>
                                    </div>
                                }.into_any()
                            }}
                        </div>

                        // Alerts list
                        {if alerts.is_empty() {
                            view! {
                                <div class="flex flex-col items-center justify-center py-12 text-center">
                                    <div class="h-16 w-16 rounded-full bg-muted flex items-center justify-center mb-4">
                                        <BellIcon class="h-8 w-8 text-muted-foreground" />
                                    </div>
                                    <h3 class="text-lg font-medium text-foreground mb-2">"No alerts yet"</h3>
                                    <p class="text-muted-foreground max-w-md">
                                        "When your watches detect something noteworthy, alerts will appear here."
                                    </p>
                                </div>
                            }.into_any()
                        } else {
                            let watches_for_cards = watches.clone();
                            view! {
                                <div class="space-y-3">
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

                                        // Card classes — matches React exactly
                                        let card_class = format!(
                                            "rounded-lg border overflow-hidden {}{}",
                                            if is_deleted {
                                                "opacity-60 border-border bg-muted/30"
                                            } else if is_unread {
                                                "border-l-4 border-l-primary border-y-border border-r-border bg-primary/10"
                                            } else {
                                                "border-border bg-card"
                                            },
                                            if is_selected { " ring-2 ring-primary/50" } else { "" }
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
                                                            "flex-1 min-w-0 py-3 pr-2 sm:pr-4 flex items-center justify-between hover:bg-muted/50 transition-colors text-left {}",
                                                            if is_deleted { "pl-3 sm:pl-4" } else { "pl-2" }
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
                                                                view! { <ChartBarIcon class=icon_class /> }.into_any()
                                                            } else {
                                                                let icon_class = format!(
                                                                    "h-5 w-5 shrink-0 hidden sm:block {}",
                                                                    if is_unread { "text-primary" } else { "text-muted-foreground" }
                                                                );
                                                                view! { <BellIcon class=icon_class /> }.into_any()
                                                            }}
                                                            <div class="min-w-0 flex-1">
                                                                <div class="flex items-center gap-2">
                                                                    <span class=format!(
                                                                        "text-foreground truncate {}",
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
                                                                    <ClockIcon class="h-3 w-3 shrink-0" />
                                                                    <span class="shrink-0">{started_at}</span>
                                                                </div>
                                                            </div>
                                                        </div>
                                                        // Expand chevron
                                                        {move || {
                                                            if expanded_alerts.get().contains(&alert_id) {
                                                                view! { <ChevronUpIcon class="h-5 w-5 text-muted-foreground shrink-0 ml-1" /> }.into_any()
                                                            } else {
                                                                view! { <ChevronDownIcon class="h-5 w-5 text-muted-foreground shrink-0 ml-1" /> }.into_any()
                                                            }
                                                        }}
                                                    </button>

                                                    // Action buttons
                                                    <div class="pr-2 sm:pr-3 flex items-center shrink-0">
                                                        // Continue in Chat button — hidden on mobile, shown in dropdown instead
                                                        <button
                                                            class="hidden sm:inline-flex items-center justify-center gap-2 whitespace-nowrap rounded-md text-sm font-medium transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:pointer-events-none disabled:opacity-50 [&_svg]:pointer-events-none [&_svg]:size-4 [&_svg]:shrink-0 text-foreground hover:bg-accent hover:text-accent-foreground h-8 rounded-md px-3 text-xs"
                                                            disabled=move || continue_chat_alert_id.get() == Some(alert_id)
                                                            title="Continue in Chat"
                                                            on:click=move |_| {
                                                                set_continue_chat_alert_id.set(Some(alert_id));
                                                                let on_chat = on_continue_chat;
                                                                leptos::task::spawn_local(async move {
                                                                    match continue_alert_in_chat(alert_id).await {
                                                                        Ok(session_id) => {
                                                                            on_chat.run(session_id);
                                                                        }
                                                                        Err(e) => {
                                                                            leptos::logging::error!("Failed to continue in chat: {e}");
                                                                        }
                                                                    }
                                                                    set_continue_chat_alert_id.set(None);
                                                                });
                                                            }
                                                        >
                                                            {move || {
                                                                if continue_chat_alert_id.get() == Some(alert_id) {
                                                                    view! { <Spinner /> }.into_any()
                                                                } else {
                                                                    view! { <ChatBubbleIcon class="h-4 w-4" /> }.into_any()
                                                                }
                                                            }}
                                                        </button>

                                                        // More actions dropdown
                                                        <AlertDropdownMenu
                                                            is_deleted=is_deleted
                                                            is_unread=is_unread
                                                            alert_id=alert_id
                                                            refetch_trigger=refetch_trigger
                                                            on_continue_chat=on_continue_chat
                                                        />
                                                    </div>
                                                </div>

                                                // Alert content (expanded)
                                                {move || {
                                                    let agent_response = agent_response.clone();
                                                    expanded_alerts.get().contains(&alert_id).then(|| {
                                                        view! {
                                                            <div class="px-3 sm:px-4 py-3 border-t border-border bg-muted/30 overflow-x-auto">
                                                                // Simple prose rendering — full MarkdownRenderer integration comes later
                                                                <div class="prose prose-sm text-sm">
                                                                    {agent_response}
                                                                </div>
                                                            </div>
                                                        }
                                                    })
                                                }}
                                            </div>
                                        }
                                    }).collect_view()}
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
                }.into_any()
            }}
        </Suspense>
    }
}
