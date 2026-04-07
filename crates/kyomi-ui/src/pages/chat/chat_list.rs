// SPDX-License-Identifier: AGPL-3.0-or-later

//! Chats list page — matches `apps/frontend/src/pages/ChatsList.jsx`.
//!
//! Displays a searchable, filterable list of chat sessions with status badges,
//! delete/bulk-delete, and real-time WebSocket updates. Follows the same
//! patterns as `dashboards_list.rs`.
//!
//! ## Features (Tasks 6.1–6.4)
//!
//! - Core layout: header with "New Chat" button, session list with Suspense
//! - Search with 300ms debounce calling `search_chat_messages`
//! - Filter buttons: All / My Conversations / Shared with Me / Slack
//! - Pinned filter toggle (star icon)
//! - Individual delete with confirm dialog
//! - Bulk selection with select-all and bulk delete bar
//! - Real-time WebSocket updates via `shared_conversation_activity`
//! - Custom `sessions-deleted` DOM event dispatch/subscription

use leptos::prelude::*;

use crate::components::{Badge, BadgeVariant, Checkbox, ConfirmDialog, Spinner};
use crate::server_fns::chat::{
    bulk_delete_sessions, delete_chat_session, list_chat_sessions, ChatSessionItem,
};
use crate::server_fns::context::get_user_context;

use super::format_relative_time;

// ─────────────────────────────────────────────────────────────────────────────
// Chat status / ownership helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Chat ownership status — matches React `getChatStatus`.
#[derive(Clone, Copy, PartialEq)]
enum ChatStatus {
    Private,
    SharedByMe,
    SharedWithMe,
}

/// Determine the chat status for the current user — matches React `getChatStatus`.
fn get_chat_status(session: &ChatSessionItem, current_user_id: &str) -> ChatStatus {
    let is_owned = is_session_owned(session, current_user_id);

    if is_owned && session.shared {
        ChatStatus::SharedByMe
    } else if is_owned && !session.shared {
        ChatStatus::Private
    } else {
        ChatStatus::SharedWithMe
    }
}

/// Whether the current user owns the session — matches React `isOwned`.
///
/// React also checks `session.user_id === user?.user_id` as a fallback, but
/// `SessionListItem` from `kyomi_auth::chat_service` does not expose a top-level
/// `user_id` field. The `created_by` check is equivalent because the service
/// layer always populates `created_by` for sessions the user owns, so the
/// fallback is unnecessary here.
fn is_session_owned(session: &ChatSessionItem, current_user_id: &str) -> bool {
    if let Some(ref created_by) = session.created_by {
        created_by.user_id == current_user_id
    } else {
        // Legacy sessions without created_by — owned if not shared
        !session.shared
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Chat filter enum
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq)]
enum ChatFilter {
    All,
    Mine,
    SharedWithMe,
    Slack,
}

// ─────────────────────────────────────────────────────────────────────────────
// Main page component
// ─────────────────────────────────────────────────────────────────────────────

/// Chats list page with search, filters, delete, bulk operations, and
/// real-time WebSocket updates.
#[component]
pub fn ChatsListPage() -> impl IntoView {
    // ── User context (for ownership checks + multi_user_enabled) ─────────
    let user_ctx_resource = Resource::new(|| (), |_| get_user_context());

    // ── Sessions state ───────────────────────────────────────────────────
    let (sessions, set_sessions) = signal(Vec::<ChatSessionItem>::new());
    let (is_loading, set_is_loading) = signal(true);

    // ── Search (with 300ms debounce) ─────────────────────────────────────
    let (search_input, set_search_input) = signal(String::new());
    let (is_searching, set_is_searching) = signal(false);

    // ── Pinned filter ────────────────────────────────────────────────────
    let (show_pinned_only, set_show_pinned_only) = signal(false);

    // ── Chat filter ──────────────────────────────────────────────────────
    let (chat_filter, set_chat_filter) = signal(ChatFilter::All);

    // ── Selection state ──────────────────────────────────────────────────
    let (selected_chats, set_selected_chats) = signal(Vec::<String>::new());
    let (is_bulk_deleting, set_is_bulk_deleting) = signal(false);

    // ── Delete confirmation ──────────────────────────────────────────────
    let (confirm_open, set_confirm_open) = signal(false);
    let (confirm_title, set_confirm_title) = signal(String::new());
    let (confirm_message, set_confirm_message) = signal(String::new());
    // Stores the action to execute on confirm: single delete ID or bulk flag
    let (pending_delete, set_pending_delete) = signal(Option::<PendingDelete>::None);

    // ── Load sessions function ───────────────────────────────────────────
    // We use a Resource for the initial load, then manage state reactively.
    let sessions_resource = Resource::new(
        move || show_pinned_only.get(),
        list_chat_sessions,
    );

    // Sync resource results into the sessions signal
    Effect::new(move |_| {
        if let Some(Ok(data)) = sessions_resource.get() {
            set_sessions.set(data);
            set_is_loading.set(false);
        } else if let Some(Err(_)) = sessions_resource.get() {
            set_sessions.set(Vec::new());
            set_is_loading.set(false);
        }
    });

    // ── Debounced search ─────────────────────────────────────────────────
    #[cfg(target_arch = "wasm32")]
    {
        use crate::server_fns::chat::search_chat_messages;
        use send_wrapper::SendWrapper;

        let timeout_handle: StoredValue<Option<SendWrapper<gloo_timers::callback::Timeout>>> =
            StoredValue::new(None);

        Effect::new(move |_| {
            let value = search_input.get();

            // Cancel any pending timeout
            timeout_handle.update_value(|h| {
                drop(h.take());
            });

            if value.trim().is_empty() {
                // No search — refetch full list
                set_is_searching.set(false);
                sessions_resource.refetch();
                return;
            }

            let handle = gloo_timers::callback::Timeout::new(300, move || {
                set_is_searching.set(true);
                let pinned = show_pinned_only.get_untracked();
                leptos::task::spawn_local(async move {
                    match search_chat_messages(value).await {
                        Ok(mut results) => {
                            if pinned {
                                results.retain(|s| s.pinned_count > 0);
                            }
                            set_sessions.set(results);
                        }
                        Err(_) => {
                            set_sessions.set(Vec::new());
                        }
                    }
                    set_is_searching.set(false);
                });
            });

            timeout_handle.set_value(Some(SendWrapper::new(handle)));
        });

        on_cleanup(move || {
            timeout_handle.update_value(|h| {
                drop(h.take());
            });
        });
    }

    // SSR: no debounce needed
    #[cfg(not(target_arch = "wasm32"))]
    {
        Effect::new(move |_| {
            let value = search_input.get();
            if value.trim().is_empty() {
                set_is_searching.set(false);
                sessions_resource.refetch();
            }
            // On SSR, search happens on next page load — no async calls
        });
    }

    // ── Clear selection when filter/search/pinned changes ────────────────
    Effect::new(move |_| {
        // Track all three signals
        let _ = chat_filter.get();
        let _ = search_input.get();
        let _ = show_pinned_only.get();
        set_selected_chats.set(Vec::new());
    });

    // ── Task 6.4: WebSocket subscription for shared_conversation_activity ─
    #[cfg(target_arch = "wasm32")]
    {
        use crate::components::chat::websocket_client::WebSocketContext;
        if let Some(ws_ctx) = use_context::<WebSocketContext>() {
            let unsub = ws_ctx.subscribe("shared_conversation_activity", move |msg| {
                set_sessions.update(|sessions| {
                    if let Some(session_id) = &msg.session_id {
                        if let Some(session) = sessions
                            .iter_mut()
                            .find(|s| s.session_id == *session_id)
                        {
                            if let Some(ref ts) = msg.timestamp {
                                session.updated_at = ts.clone();
                            }
                            session.unread_count += 1;
                        }
                    }
                    // Re-sort by updated_at (most recent first)
                    sessions.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
                });
            });

            let unsub = send_wrapper::SendWrapper::new(unsub);
            on_cleanup(move || {
                unsub.take()();
            });
        }
    }

    // ── Task 6.4: Listen for sessions-deleted DOM event ──────────────────
    // NOTE: This Effect::new closure only runs once despite being inside an
    // effect, because all captured signals (`set_sessions`, `set_selected_chats`)
    // are write-only — no reactive reads trigger re-execution. The `on_cleanup`
    // inside correctly removes the listener when the component unmounts.
    #[cfg(target_arch = "wasm32")]
    {
        use send_wrapper::SendWrapper;
        use wasm_bindgen::prelude::*;

        Effect::new(move |_| {
            let window = web_sys::window().expect("window");

            let cb = Closure::<dyn Fn(web_sys::CustomEvent)>::new(move |event: web_sys::CustomEvent| {
                // Parse the detail
                if let Ok(detail) = js_sys::Reflect::get(&event.detail(), &"source".into()) {
                    if let Some(source) = detail.as_string() {
                        if source == "chatsList" {
                            return; // Ignore our own events
                        }
                    }
                }

                if let Ok(ids_val) = js_sys::Reflect::get(&event.detail(), &"sessionIds".into()) {
                    if let Some(ids_array) = ids_val.dyn_ref::<js_sys::Array>() {
                        let deleted_ids: Vec<String> = ids_array
                            .iter()
                            .filter_map(|v| v.as_string())
                            .collect();

                        if !deleted_ids.is_empty() {
                            set_sessions.update(|sessions| {
                                sessions.retain(|s| !deleted_ids.contains(&s.session_id));
                            });
                            set_selected_chats.update(|selected| {
                                selected.retain(|id| !deleted_ids.contains(id));
                            });
                        }
                    }
                }
            });

            let _ = window.add_event_listener_with_callback(
                "sessions-deleted",
                cb.as_ref().unchecked_ref(),
            );

            // Store closure reference for cleanup
            let cleanup_cb = SendWrapper::new(cb);
            on_cleanup(move || {
                if let Some(window) = web_sys::window() {
                    let _ = window.remove_event_listener_with_callback(
                        "sessions-deleted",
                        cleanup_cb.as_ref().unchecked_ref(),
                    );
                }
            });
        });
    }

    // ── Dispatch sessions-deleted custom event ───────────────────────────
    #[cfg(target_arch = "wasm32")]
    fn dispatch_sessions_deleted(session_ids: &[String]) {
        if let Some(window) = web_sys::window() {
            let ids_array = js_sys::Array::new();
            for id in session_ids {
                ids_array.push(&wasm_bindgen::JsValue::from_str(id));
            }

            let detail = js_sys::Object::new();
            let _ = js_sys::Reflect::set(&detail, &"sessionIds".into(), &ids_array);
            let _ = js_sys::Reflect::set(
                &detail,
                &"source".into(),
                &wasm_bindgen::JsValue::from_str("chatsList"),
            );

            let init = web_sys::CustomEventInit::new();
            init.set_detail(&detail);

            if let Ok(event) = web_sys::CustomEvent::new_with_event_init_dict("sessions-deleted", &init)
            {
                let _ = window.dispatch_event(&event);
            }
        }
    }

    // ── Confirm dialog callbacks ─────────────────────────────────────────
    let on_confirm = Callback::new(move |()| {
        set_confirm_open.set(false);
        let pending = pending_delete.get_untracked();
        match pending {
            Some(PendingDelete::Single(session_id)) => {
                leptos::task::spawn_local(async move {
                    if delete_chat_session(session_id.clone()).await.is_ok() {
                        set_sessions.update(|sessions| {
                            sessions.retain(|s| s.session_id != session_id);
                        });
                        #[cfg(target_arch = "wasm32")]
                        dispatch_sessions_deleted(&[session_id]);
                    }
                });
            }
            Some(PendingDelete::Bulk) => {
                let ids = selected_chats.get_untracked();
                if ids.is_empty() {
                    return;
                }
                set_is_bulk_deleting.set(true);
                leptos::task::spawn_local(async move {
                    if bulk_delete_sessions(ids.clone()).await.is_ok() {
                        set_sessions.update(|sessions| {
                            sessions.retain(|s| !ids.contains(&s.session_id));
                        });
                        set_selected_chats.set(Vec::new());
                        #[cfg(target_arch = "wasm32")]
                        dispatch_sessions_deleted(&ids);
                    }
                    set_is_bulk_deleting.set(false);
                });
            }
            None => {}
        }
        set_pending_delete.set(None);
    });

    let on_cancel = Callback::new(move |()| {
        set_confirm_open.set(false);
        set_pending_delete.set(None);
    });

    // ── Derived: filtered + sorted sessions ──────────────────────────────
    let filtered_sessions = move || -> Vec<ChatSessionItem> {
        let current_sessions = sessions.get();
        let filter = chat_filter.get();

        let user_id = user_ctx_resource
            .get()
            .and_then(|r| r.ok())
            .map(|ctx| ctx.user_id.clone())
            .unwrap_or_default();

        let mut filtered: Vec<ChatSessionItem> = match filter {
            ChatFilter::All => current_sessions,
            ChatFilter::Mine => current_sessions
                .into_iter()
                .filter(|s| {
                    let status = get_chat_status(s, &user_id);
                    status == ChatStatus::Private || status == ChatStatus::SharedByMe
                })
                .collect(),
            ChatFilter::SharedWithMe => current_sessions
                .into_iter()
                .filter(|s| get_chat_status(s, &user_id) == ChatStatus::SharedWithMe)
                .collect(),
            ChatFilter::Slack => current_sessions
                .into_iter()
                .filter(|s| s.slack_channel_id.is_some() && s.shared)
                .collect(),
        };

        // Sort by most recent first. React sorts by `last_activity_at || created_at`,
        // but `SessionListItem` from `kyomi_auth::chat_service` does not expose
        // `last_activity_at`. We use `updated_at` which the service layer updates
        // on every message, making it functionally equivalent.
        filtered.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        filtered
    };

    // ── Derived: multi_user_enabled capability ───────────────────────────
    let multi_user_enabled = move || -> bool {
        user_ctx_resource
            .get()
            .and_then(|r| r.ok())
            .and_then(|ctx| ctx.capabilities.get("multi_user_enabled").copied())
            .unwrap_or(false)
    };

    // ── Derived: current user_id ─────────────────────────────────────────
    let current_user_id = move || -> String {
        user_ctx_resource
            .get()
            .and_then(|r| r.ok())
            .map(|ctx| ctx.user_id.clone())
            .unwrap_or_default()
    };

    // ── Derived: selectable sessions (owned by current user) ─────────────
    let selectable_session_ids = move || -> Vec<String> {
        let uid = current_user_id();
        filtered_sessions()
            .iter()
            .filter(|s| is_session_owned(s, &uid))
            .map(|s| s.session_id.clone())
            .collect()
    };

    let has_selection = move || !selected_chats.get().is_empty();
    let selection_count = move || selected_chats.get().len();
    let is_all_selected = move || {
        let selectable = selectable_session_ids();
        let selected = selected_chats.get();
        !selectable.is_empty() && selected.len() == selectable.len()
    };

    // ── Handlers ─────────────────────────────────────────────────────────

    let handle_delete_click = move |session_id: String| {
        set_pending_delete.set(Some(PendingDelete::Single(session_id)));
        set_confirm_title.set("Delete Chat?".to_string());
        set_confirm_message
            .set("Are you sure you want to delete this chat? This action cannot be undone.".to_string());
        set_confirm_open.set(true);
    };

    let handle_bulk_delete = move |_| {
        let count = selection_count();
        if count == 0 {
            return;
        }
        set_pending_delete.set(Some(PendingDelete::Bulk));
        let suffix = if count != 1 { "s" } else { "" };
        set_confirm_title.set(format!("Delete {count} chat{suffix}?"));
        set_confirm_message
            .set("Are you sure you want to delete these chats? This action cannot be undone.".to_string());
        set_confirm_open.set(true);
    };

    let toggle_select_all = move |_| {
        if is_all_selected() {
            set_selected_chats.set(Vec::new());
        } else {
            set_selected_chats.set(selectable_session_ids());
        }
    };

    let toggle_chat_selection = move |session_id: String| {
        set_selected_chats.update(|selected| {
            if let Some(pos) = selected.iter().position(|id| *id == session_id) {
                selected.remove(pos);
            } else {
                selected.push(session_id);
            }
        });
    };

    view! {
        <div class="flex flex-col h-full bg-muted" style="flex-direction: column;">
            // Header
            <div class="h-16 bg-card border-b border-border px-6 flex-shrink-0 flex items-center justify-between">
                <h1 class="text-xl font-semibold font-display text-foreground">"Chats"</h1>

                // New Chat Button
                <a
                    href="/chat"
                    class="inline-flex items-center justify-center gap-2 whitespace-nowrap rounded-md text-sm font-medium transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring h-9 px-4 py-2 bg-primary text-primary-foreground shadow hover:bg-primary/90"
                >
                    <svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                        <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M12 4v16m8-8H4" />
                    </svg>
                    "New Chat"
                </a>
            </div>

            // Search and Filter Toolbar
            <div class="bg-card border-b border-border px-6 py-3 flex-shrink-0">
                // Search Bar and Pinned Filter
                <div class="flex items-center gap-3">
                    // Select-all checkbox
                    <Show when=move || !selectable_session_ids().is_empty()>
                        <div class="flex items-center shrink-0">
                            <Checkbox
                                checked=Signal::derive(is_all_selected)
                                on_change=Callback::new(move |_: bool| toggle_select_all(()))
                            />
                        </div>
                    </Show>

                    <div class="relative flex-1">
                        // Search icon or spinner
                        <Show
                            when=move || is_searching.get()
                            fallback=|| view! {
                                <svg
                                    class="absolute left-3 top-1/2 transform -translate-y-1/2 w-4 h-4 text-muted-foreground"
                                    fill="none"
                                    stroke="currentColor"
                                    viewBox="0 0 24 24"
                                >
                                    <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M21 21l-6-6m2-5a7 7 0 11-14 0 7 7 0 0114 0z" />
                                </svg>
                            }
                        >
                            <Spinner class="absolute left-3 top-1/2 transform -translate-y-1/2 text-muted-foreground" />
                        </Show>
                        <input
                            type="text"
                            prop:value=move || search_input.get()
                            on:input=move |ev| {
                                set_search_input.set(event_target_value(&ev));
                            }
                            placeholder="Search chats..."
                            class="w-full pl-9 pr-9 py-2 text-sm border border-border rounded-lg bg-card text-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
                        />
                        // Clear button
                        <Show when=move || !search_input.get().is_empty() && !is_searching.get()>
                            <button
                                on:click=move |_| set_search_input.set(String::new())
                                class="absolute right-3 top-1/2 transform -translate-y-1/2 text-muted-foreground transition-colors hover:text-foreground"
                            >
                                <svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                                    <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M6 18L18 6M6 6l12 12" />
                                </svg>
                            </button>
                        </Show>
                    </div>

                    // Pinned Filter Button
                    <button
                        on:click=move |_| set_show_pinned_only.update(|v| *v = !*v)
                        class=move || {
                            if show_pinned_only.get() {
                                "p-2 rounded-lg transition-colors bg-accent text-foreground"
                            } else {
                                "p-2 rounded-lg transition-colors bg-card border border-border text-muted-foreground hover:text-foreground hover:bg-accent"
                            }
                        }
                        aria-label=move || {
                            if show_pinned_only.get() {
                                "Show all chats"
                            } else {
                                "Show only pinned chats"
                            }
                        }
                    >
                        <svg
                            class="w-4 h-4"
                            fill=move || if show_pinned_only.get() { "currentColor" } else { "none" }
                            stroke="currentColor"
                            viewBox="0 0 24 24"
                        >
                            <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M11.049 2.927c.3-.921 1.603-.921 1.902 0l1.519 4.674a1 1 0 00.95.69h4.915c.969 0 1.371 1.24.588 1.81l-3.976 2.888a1 1 0 00-.363 1.118l1.518 4.674c.3.922-.755 1.688-1.538 1.118l-3.976-2.888a1 1 0 00-1.176 0l-3.976 2.888c-.783.57-1.838-.197-1.538-1.118l1.518-4.674a1 1 0 00-.363-1.118l-3.976-2.888c-.784-.57-.38-1.81.588-1.81h4.914a1 1 0 00.951-.69l1.519-4.674z" />
                        </svg>
                    </button>
                </div>

                // Filter Buttons OR Bulk Action Bar
                <Show
                    when=has_selection
                    fallback=move || {
                        let multi = multi_user_enabled();
                        view! {
                            <div class="flex items-center gap-2 mt-3">
                                <FilterButton
                                    label="All"
                                    active=Signal::derive(move || chat_filter.get() == ChatFilter::All)
                                    on_click=Callback::new(move |()| set_chat_filter.set(ChatFilter::All))
                                />
                                <Show when=move || multi>
                                    <FilterButton
                                        label="My Conversations"
                                        active=Signal::derive(move || chat_filter.get() == ChatFilter::Mine)
                                        on_click=Callback::new(move |()| set_chat_filter.set(ChatFilter::Mine))
                                    />
                                    <FilterButton
                                        label="Shared with Me"
                                        active=Signal::derive(move || chat_filter.get() == ChatFilter::SharedWithMe)
                                        on_click=Callback::new(move |()| set_chat_filter.set(ChatFilter::SharedWithMe))
                                    />
                                </Show>
                                <button
                                    on:click=move |_| set_chat_filter.set(ChatFilter::Slack)
                                    class=move || {
                                        if chat_filter.get() == ChatFilter::Slack {
                                            "px-3 py-1.5 text-sm rounded-lg transition-colors flex items-center gap-1.5 bg-primary text-primary-foreground"
                                        } else {
                                            "px-3 py-1.5 text-sm rounded-lg transition-colors flex items-center gap-1.5 bg-accent text-foreground hover:bg-accent/80"
                                        }
                                    }
                                >
                                    <svg class="w-3.5 h-3.5" viewBox="0 0 24 24" fill="currentColor">
                                        <path d="M6 15a2 2 0 0 1-2 2a2 2 0 0 1-2-2a2 2 0 0 1 2-2h2v2zm1 0a2 2 0 0 1 2-2a2 2 0 0 1 2 2v5a2 2 0 0 1-2 2a2 2 0 0 1-2-2v-5zm2-8a2 2 0 0 1-2-2a2 2 0 0 1 2-2a2 2 0 0 1 2 2v2H9zm0 1a2 2 0 0 1 2 2a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2a2 2 0 0 1 2-2h5zm8 2a2 2 0 0 1 2-2a2 2 0 0 1 2 2a2 2 0 0 1-2 2h-2v-2zm-1 0a2 2 0 0 1-2 2a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2a2 2 0 0 1 2 2v5zm-2 8a2 2 0 0 1 2 2a2 2 0 0 1-2 2a2 2 0 0 1-2-2v-2h2zm0-1a2 2 0 0 1-2-2a2 2 0 0 1 2-2h5a2 2 0 0 1 2 2a2 2 0 0 1-2 2h-5z"/>
                                    </svg>
                                    "Slack"
                                </button>
                            </div>
                        }
                    }
                >
                    <div class="flex items-center gap-3 mt-3">
                        <span class="text-sm font-medium text-foreground">
                            {move || format!("{} selected", selection_count())}
                        </span>
                        <button
                            on:click=handle_bulk_delete
                            disabled=move || is_bulk_deleting.get()
                            class="flex items-center gap-1.5 px-3 py-1.5 text-sm font-medium text-error-foreground bg-error/10 hover:bg-error/20 rounded-lg transition-colors disabled:opacity-50"
                        >
                            <svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                                <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M19 7l-.867 12.142A2 2 0 0116.138 21H7.862a2 2 0 01-1.995-1.858L5 7m5 4v6m4-6v6m1-10V4a1 1 0 00-1-1h-4a1 1 0 00-1 1v3M4 7h16" />
                            </svg>
                            {move || if is_bulk_deleting.get() { "Deleting..." } else { "Delete" }}
                        </button>
                        <button
                            on:click=move |_| set_selected_chats.set(Vec::new())
                            class="ml-auto px-3 py-1.5 text-sm text-muted-foreground hover:text-foreground hover:bg-accent rounded-lg transition-colors"
                        >
                            "Cancel"
                        </button>
                    </div>
                </Show>

                // Results Count
                <Show when=move || !search_input.get().is_empty()>
                    <div class="text-sm text-muted-foreground mt-2">
                        {move || {
                            let count = sessions.get().len();
                            let query = search_input.get();
                            let label = if count == 1 { "chat" } else { "chats" };
                            format!("{count} {label} matching \"{query}\"")
                        }}
                    </div>
                </Show>
            </div>

            // Chats List
            <div class="flex-1 overflow-y-auto p-4 md:p-6">
                <Transition fallback=move || view! {
                    <div class="flex items-center justify-center py-12">
                        <div class="text-muted-foreground">"Loading chats..."</div>
                    </div>
                }>
                    {move || {
                        // Wait for user context to be available
                        let _user_ctx = user_ctx_resource.get();

                        if is_loading.get() {
                            return view! {
                                <div class="flex items-center justify-center py-12">
                                    <div class="text-muted-foreground">"Loading chats..."</div>
                                </div>
                            }.into_any();
                        }

                        let current_sessions = sessions.get();

                        if current_sessions.is_empty() {
                            let searching = !search_input.get().is_empty();
                            return view! {
                                <div class="flex flex-col items-center justify-center py-12">
                                    // Chat bubble icon
                                    <svg class="w-16 h-16 text-muted-foreground/50 mb-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                                        <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M8 12h.01M12 12h.01M16 12h.01M21 12c0 4.418-4.03 8-9 8a9.863 9.863 0 01-4.255-.949L3 20l1.395-3.72C3.512 15.042 3 13.574 3 12c0-4.418 4.03-8 9-8s9 3.582 9 8z" />
                                    </svg>
                                    <p class="text-muted-foreground text-lg mb-2">
                                        {if searching { "No chats found" } else { "No chats yet" }}
                                    </p>
                                    <p class="text-muted-foreground text-sm mb-4">
                                        {if searching { "Try a different search term" } else { "Start a new conversation to get started" }}
                                    </p>
                                    <Show when=move || !searching>
                                        <a
                                            href="/chat"
                                            class="inline-flex items-center justify-center gap-2 whitespace-nowrap rounded-md text-sm font-medium transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring h-9 px-4 py-2 bg-primary text-primary-foreground shadow hover:bg-primary/90"
                                        >
                                            "Start New Chat"
                                        </a>
                                    </Show>
                                </div>
                            }.into_any();
                        }

                        let filtered = filtered_sessions();
                        let uid = current_user_id();
                        let multi = multi_user_enabled();

                        if filtered.is_empty() {
                            return view! {
                                <div class="flex flex-col items-center justify-center py-12 text-muted-foreground">
                                    <svg class="w-16 h-16 mb-4 opacity-50" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                                        <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M8 12h.01M12 12h.01M16 12h.01M21 12c0 4.418-4.03 8-9 8a9.863 9.863 0 01-4.255-.949L3 20l1.395-3.72C3.512 15.042 3 13.574 3 12c0-4.418 4.03-8 9-8s9 3.582 9 8z" />
                                    </svg>
                                    <p class="text-lg font-medium">"No conversations found"</p>
                                    <p class="text-sm mt-1">"Start a new chat to get started"</p>
                                </div>
                            }.into_any();
                        }

                        let items = filtered
                            .into_iter()
                            .map(|session| {
                                let session_id = session.session_id.clone();
                                let session_id_nav = session.session_id.clone();
                                let session_id_delete = session.session_id.clone();
                                let session_id_select = session.session_id.clone();
                                let title = session.title.clone().unwrap_or_else(|| "Untitled Chat".to_string());
                                let relative_time = format_relative_time(&session.updated_at);
                                let status = get_chat_status(&session, &uid);
                                let owned = is_session_owned(&session, &uid);
                                let is_slack = session.slack_channel_id.is_some() && session.shared;
                                let unread_count = session.unread_count;
                                let created_by_name = session
                                    .created_by
                                    .as_ref()
                                    .map(|cb| cb.display_name.clone());

                                let session_id_check = session_id.clone();
                                let session_id_check2 = session_id.clone();
                                let is_selected = move || {
                                    selected_chats.get().contains(&session_id_check)
                                };
                                let is_selected_class = move || {
                                    selected_chats.get().contains(&session_id_check2)
                                };

                                let handle_delete = handle_delete_click;
                                let toggle_select = toggle_chat_selection;

                                view! {
                                    <div class=move || {
                                        let base = "group flex items-center gap-3 bg-card border rounded-lg hover:border-border hover:shadow-sm transition-all";
                                        if is_selected_class() {
                                            format!("{base} border-primary/50 ring-2 ring-primary/20")
                                        } else {
                                            format!("{base} border-border")
                                        }
                                    }>
                                        // Checkbox for owned sessions
                                        {if owned {
                                            let sid = session_id_select.clone();
                                            Some(view! {
                                                <div class="pl-4 flex items-center shrink-0">
                                                    <Checkbox
                                                        checked=Signal::derive(is_selected)
                                                        on_change=Callback::new(move |_: bool| {
                                                            toggle_select(sid.clone());
                                                        })
                                                    />
                                                </div>
                                            })
                                        } else {
                                            None
                                        }}

                                        <a
                                            href=format!("/chat/{session_id_nav}")
                                            class=move || {
                                                if owned {
                                                    "flex-1 min-w-0 p-4 cursor-pointer pl-0"
                                                } else {
                                                    "flex-1 min-w-0 p-4 cursor-pointer"
                                                }
                                            }
                                        >
                                            <div class="flex items-start justify-between gap-4">
                                                <div class="flex-1 min-w-0">
                                                    <div class="flex items-center gap-2 mb-1">
                                                        <h3 class="text-base font-medium text-foreground truncate">
                                                            {title.clone()}
                                                        </h3>

                                                        // Status badges
                                                        {if is_slack {
                                                            Some(view! {
                                                                <Badge variant=BadgeVariant::Outline class="flex items-center gap-1">
                                                                    <svg class="w-3 h-3" viewBox="0 0 24 24" fill="currentColor">
                                                                        <path d="M6 15a2 2 0 0 1-2 2a2 2 0 0 1-2-2a2 2 0 0 1 2-2h2v2zm1 0a2 2 0 0 1 2-2a2 2 0 0 1 2 2v5a2 2 0 0 1-2 2a2 2 0 0 1-2-2v-5zm2-8a2 2 0 0 1-2-2a2 2 0 0 1 2-2a2 2 0 0 1 2 2v2H9zm0 1a2 2 0 0 1 2 2a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2a2 2 0 0 1 2-2h5zm8 2a2 2 0 0 1 2-2a2 2 0 0 1 2 2a2 2 0 0 1-2 2h-2v-2zm-1 0a2 2 0 0 1-2 2a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2a2 2 0 0 1 2 2v5zm-2 8a2 2 0 0 1 2 2a2 2 0 0 1-2 2a2 2 0 0 1-2-2v-2h2zm0-1a2 2 0 0 1-2-2a2 2 0 0 1 2-2h5a2 2 0 0 1 2 2a2 2 0 0 1-2 2h-5z"/>
                                                                    </svg>
                                                                    "Slack"
                                                                </Badge>
                                                            }.into_any())
                                                        } else if multi {
                                                            match status {
                                                                ChatStatus::Private => Some(view! {
                                                                    <Badge variant=BadgeVariant::Secondary>"Private"</Badge>
                                                                }.into_any()),
                                                                ChatStatus::SharedByMe => Some(view! {
                                                                    <Badge variant=BadgeVariant::Default>"Shared"</Badge>
                                                                }.into_any()),
                                                                ChatStatus::SharedWithMe => Some(view! {
                                                                    <Badge variant=BadgeVariant::Default>
                                                                        {created_by_name.clone().unwrap_or_else(|| "Shared".to_string())}
                                                                    </Badge>
                                                                }.into_any()),
                                                            }
                                                        } else {
                                                            None
                                                        }}

                                                        // Unread count badge
                                                        {if unread_count > 0 {
                                                            Some(view! {
                                                                <Badge variant=BadgeVariant::Warning>
                                                                    {format!("{unread_count} new")}
                                                                </Badge>
                                                            })
                                                        } else {
                                                            None
                                                        }}
                                                    </div>
                                                    <p class="text-sm text-muted-foreground">
                                                        {relative_time}
                                                    </p>
                                                </div>

                                                <div class="flex items-center gap-2">
                                                    // Delete button (only for owned sessions)
                                                    {if owned {
                                                        let sid = session_id_delete.clone();
                                                        Some(view! {
                                                            <button
                                                                on:click=move |ev: leptos::ev::MouseEvent| {
                                                                    ev.prevent_default();
                                                                    ev.stop_propagation();
                                                                    handle_delete(sid.clone());
                                                                }
                                                                class="opacity-0 group-hover:opacity-100 p-2 text-muted-foreground hover:text-error-foreground hover:bg-error/10 rounded-lg transition-all"
                                                                aria-label="Delete chat"
                                                            >
                                                                <svg class="w-5 h-5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                                                                    <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M19 7l-.867 12.142A2 2 0 0116.138 21H7.862a2 2 0 01-1.995-1.858L5 7m5 4v6m4-6v6m1-10V4a1 1 0 00-1-1h-4a1 1 0 00-1 1v3M4 7h16" />
                                                                </svg>
                                                            </button>
                                                        })
                                                    } else {
                                                        None
                                                    }}
                                                </div>
                                            </div>
                                        </a>
                                    </div>
                                }
                            })
                            .collect_view();

                        view! {
                            <div class="max-w-4xl mx-auto" style="display: block;">
                                <div class="space-y-2">
                                    {items}
                                </div>
                            </div>
                        }.into_any()
                    }}
                </Transition>
            </div>

            // Confirm Dialog — wrapped in a reactive closure so the title and
            // message re-render with fresh signal values each time they change.
            // ConfirmDialog accepts `title: String` and `message: String` (owned,
            // non-reactive), so without this wrapper they would be captured once
            // at mount time and stay blank forever.
            {move || {
                let title = confirm_title.get();
                let message = confirm_message.get();
                view! {
                    <ConfirmDialog
                        open=Signal::derive(move || confirm_open.get())
                        title=title
                        message=message
                        confirm_text="Delete"
                        on_confirm=on_confirm
                        on_cancel=on_cancel
                    />
                }
            }}
        </div>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Sub-components
// ─────────────────────────────────────────────────────────────────────────────

/// What kind of delete is pending confirmation.
#[derive(Clone)]
enum PendingDelete {
    Single(String),
    Bulk,
}

/// A filter button in the toolbar — matches React's filter button styling.
#[component]
fn FilterButton(
    /// Button label text.
    #[prop(into)]
    label: String,
    /// Whether this filter is currently active.
    active: Signal<bool>,
    /// Called when clicked.
    on_click: Callback<()>,
) -> impl IntoView {
    view! {
        <button
            on:click=move |_| on_click.run(())
            class=move || {
                if active.get() {
                    "px-3 py-1.5 text-sm rounded-lg transition-colors bg-primary text-primary-foreground"
                } else {
                    "px-3 py-1.5 text-sm rounded-lg transition-colors bg-accent text-foreground hover:bg-accent/80"
                }
            }
        >
            {label}
        </button>
    }
}
