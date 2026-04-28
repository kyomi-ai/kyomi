// SPDX-License-Identifier: AGPL-3.0-or-later

//! Layout shell — sidebar + content area.
//!
//! Matches `apps/frontend/src/components/Sidebar.jsx` structure and classes.
//! Design system: `docs/DESIGN_SYSTEM.md` — sidebar gets `shadow-lg`.
//!
//! React sidebar structure:
//! - Header: collapse toggle (left), logo images (right of toggle)
//! - Nav: New Chat, Chats, Dashboards, Watches, Knowledge, SQL Editor
//! - Recent Chats: scrollable list, hidden when collapsed
//! - User Menu: avatar + name + workspace, dropdown with Settings/Help/Feedback/Logout

use leptos::prelude::*;
use phosphor_leptos::{Icon, IconWeight};
use leptos_router::hooks::use_navigate;
use leptos_router::NavigateOptions;

use crate::components::chat::WebSocketProvider;
use crate::components::empty_state::EmptyStateVariant;
use crate::components::feedback_modal::FeedbackModal;
use crate::components::popover::{Placement, Popover};
use crate::components::EmptyState;
use crate::query_cache::{provide_query_cache, use_query};
#[cfg(target_arch = "wasm32")]
use crate::query_cache::QueryCache;
use crate::server_fns::context::get_user_context;
use crate::server_fns::security::logout;
use crate::server_fns::home::get_landing_config;
use crate::server_fns::sidebar::{get_recent_sessions, get_sidebar_user};
use crate::server_fns::watches::get_unread_alerts_count;

/// Main layout shell wrapping all Leptos pages.
///
/// Matches React Sidebar.jsx mobile behaviour:
/// - Mobile (<768px): sidebar hidden by default, hamburger header bar at top
/// - Desktop (768px+): sidebar always visible (collapsed/expanded)
#[component]
pub fn Layout(children: Children) -> impl IntoView {
    // Restore collapsed state from localStorage so it persists across navigation.
    // Each route creates its own Layout instance, so without persistence the
    // sidebar always resets to expanded on page change.
    let initial_collapsed = {
        #[cfg(target_arch = "wasm32")]
        {
            web_sys::window()
                .and_then(|w| w.local_storage().ok().flatten())
                .and_then(|s| s.get_item("sidebar_collapsed").ok().flatten())
                .map(|v| v == "true")
                .unwrap_or(false)
        }
        #[cfg(not(target_arch = "wasm32"))]
        { false }
    };
    let (collapsed, set_collapsed) = signal(initial_collapsed);
    let (is_mobile, set_is_mobile) = signal(false);
    #[cfg(not(target_arch = "wasm32"))]
    let _ = set_is_mobile;
    let (mobile_open, set_mobile_open) = signal(false);

    // Trigger signal to refetch sidebar user info after a token refresh.
    // Bumping this value causes the LocalResource to re-execute without a page reload.
    let (auth_retry, set_auth_retry) = signal(0u32);
    #[cfg(not(target_arch = "wasm32"))]
    let _ = &set_auth_retry;

    // Fetch sidebar user info — used for both the sidebar UI and WebSocket auth signals.
    // Uses LocalResource to avoid "reading resource outside Suspense" warnings —
    // this resource is read in Memo closures which run outside Suspense.
    // Depends on `auth_retry` so we can refetch after a silent token refresh.
    let user_info = LocalResource::new(move || {
        let _retry = auth_retry.get(); // track the trigger signal
        get_sidebar_user()
    });

    // Fetch the full UserContext once at the Layout level and provide it via
    // Leptos context. Pages read this with `expect_context` instead of each
    // creating their own resource — which eliminates the `get_user_context`
    // network request (and the accompanying skeleton flash) on every
    // navigation. The Layout is a parent route and mounts once for the whole
    // authed session, so this resource survives page changes.
    let user_ctx = LocalResource::new(get_user_context);
    provide_context(user_ctx);

    // Install the list-query cache (KYO-22 Part 2). Lives at the Layout
    // level so cached list data survives navigation between authed pages.
    provide_query_cache();

    // Create and provide the reactive in-memory sync store (KYO-169).
    // Lives at the Layout level alongside the query cache so all authed pages
    // share a single store instance that survives navigation.
    let sync_store = crate::cache::store::SyncStore::new();
    provide_context(sync_store);

    // Auth guard: tracks whether the user has been authenticated.
    // Starts as `false`; set to `true` once `user_info` resolves successfully.
    // While `false`, the layout renders a loading state instead of the full
    // app shell, preventing unauthenticated users from seeing protected pages.
    // Matches React's `<ProtectedRoute>` in App.jsx.
    #[cfg(target_arch = "wasm32")]
    let (auth_confirmed, set_auth_confirmed) = signal(false);
    #[cfg(not(target_arch = "wasm32"))]
    let (auth_confirmed, _) = signal(false);

    // Tracks whether a token refresh is already in-flight (prevents duplicate refreshes).
    #[cfg(target_arch = "wasm32")]
    let (refreshing, set_refreshing) = signal(false);

    // Retry budget: tracks timestamps (ms since epoch) of each refresh attempt
    // within a sliding window. Prevents infinite refresh loops when the server
    // keeps returning auth errors even after a successful /auth/refresh call.
    // Separate concern from `refreshing` — that guards concurrency, this guards
    // total attempts over time.
    #[cfg(target_arch = "wasm32")]
    const REFRESH_BUDGET_WINDOW_MS: f64 = 30_000.0; // 30 seconds
    #[cfg(target_arch = "wasm32")]
    const REFRESH_BUDGET_MAX_ATTEMPTS: usize = 3;
    #[cfg(target_arch = "wasm32")]
    let refresh_timestamps: RwSignal<Vec<f64>> = RwSignal::new(Vec::new());

    // Auth guard: when user_info resolves, either confirm auth or redirect to /login.
    // On WASM: if auth fails, try a silent token refresh and refetch the resource
    // (no page reload). If refresh also fails, redirect to /login.
    // On SSR/native: just gate rendering — the client will handle the redirect.
    #[cfg(target_arch = "wasm32")]
    {
        use crate::utils::auth_refresh;
        let user_info_for_effect = user_info;
        Effect::new(move || {
            match user_info_for_effect.get() {
                Some(Ok(user)) => {
                    // Apply the user's saved theme preference and sync to localStorage.
                    // Server is source of truth for cross-device consistency.
                    crate::components::theme::set_theme(&user.theme_preference);
                    crate::components::theme::save_theme_to_local_storage(&user.theme_preference);
                    // Auth succeeded — reset the retry budget so future token
                    // expirations start with a fresh allowance.
                    refresh_timestamps.set(Vec::new());
                    // Allow layout to render
                    set_auth_confirmed.set(true);
                }
                Some(Err(e)) => {
                    let msg = e.to_string();
                    if auth_refresh::is_auth_error(&msg) {
                        // Don't start a second refresh while one is in-flight
                        if refreshing.get_untracked() {
                            return;
                        }

                        // Check the sliding-window retry budget before attempting
                        // another refresh. If the budget is exhausted, redirect to
                        // /login immediately instead of looping forever.
                        let now = js_sys::Date::now();
                        let window_start = now - REFRESH_BUDGET_WINDOW_MS;
                        let recent: Vec<f64> = refresh_timestamps
                            .get_untracked()
                            .into_iter()
                            .filter(|&t| t >= window_start)
                            .collect();

                        if recent.len() >= REFRESH_BUDGET_MAX_ATTEMPTS {
                            // Budget exhausted — stop retrying and send the user
                            // back to /login to re-authenticate from scratch.
                            if let Some(win) = web_sys::window() {
                                let path = win.location().pathname().unwrap_or_default();
                                let url = if path.is_empty() || path == "/" {
                                    "/login".to_string()
                                } else {
                                    format!("/login?redirect={path}")
                                };
                                let _ = win.location().set_href(&url);
                            }
                            return;
                        }

                        // Record this attempt and proceed with the refresh.
                        let mut updated = recent;
                        updated.push(now);
                        refresh_timestamps.set(updated);

                        set_refreshing.set(true);
                        // Silent token refresh: call /api/v1/auth/refresh, then
                        // bump the trigger signal to refetch get_sidebar_user —
                        // no visible page reload.
                        leptos::task::spawn_local(async move {
                            let ok = auth_refresh::try_refresh().await;
                            set_refreshing.set(false);
                            if ok {
                                // Cookies refreshed — refetch the resource
                                set_auth_retry.update(|n| *n += 1);
                            } else {
                                // Refresh token also invalid — redirect to login
                                if let Some(win) = web_sys::window() {
                                    let path = win.location().pathname().unwrap_or_default();
                                    let url = if path.is_empty() || path == "/" {
                                        "/login".to_string()
                                    } else {
                                        format!("/login?redirect={path}")
                                    };
                                    let _ = win.location().set_href(&url);
                                }
                            }
                        });
                    } else {
                        if let Some(window) = web_sys::window() {
                            let _ = window.location().set_href("/login");
                        }
                    }
                }
                None => {
                    // Resource still loading — keep waiting
                }
            }
        });
    }

    // On SSR, auth_confirmed stays `false` intentionally — SSR always renders
    // the "Loading..." placeholder. Effects do not run during Leptos SSR, so
    // there is no server-side auth confirmation. The WASM Effect above handles
    // auth confirmation and redirect after hydration. (This is also a CSR-only
    // app served via trunk, so the SSR path is academic.)

    let ws_user_id = Memo::new(move |_| {
        user_info
            .get()
            .and_then(|r| r.ok())
            .map(|u| u.user_id)
    });
    let ws_workspace_id = Memo::new(move |_| {
        user_info
            .get()
            .and_then(|r| r.ok())
            .and_then(|u| u.workspace_id)
    });

    let is_routing = expect_context::<ReadSignal<bool>>();

    // Hydrate the SyncStore from IndexedDB on mount (WASM only).
    //
    // The Effect watches `ws_workspace_id` reactively — hydration runs once the
    // workspace ID is available (after auth resolves) and re-runs if the user
    // switches workspace. On SSR the block is absent; the store stays empty and
    // Hydrate the SyncStore from IndexedDB on mount (client-only). This gives
    // instant rendering on return visits while the sync engine catches up.
    #[cfg(target_arch = "wasm32")]
    {
        use leptos::task::spawn_local;
        let sync_store_for_hydrate = sync_store;
        let ws_id_for_hydrate = ws_workspace_id;

        Effect::new(move |_| {
            let Some(workspace_id) = ws_id_for_hydrate.get() else {
                return;
            };
            if workspace_id.is_empty() {
                return;
            }
            let store = sync_store_for_hydrate;
            store.reset();
            spawn_local(async move {
                if let Ok(cache_db) = crate::cache::db::init_cache_db(&workspace_id).await {
                    crate::cache::sync_engine::hydrate_store_from_db(&cache_db, &workspace_id, &store).await;
                }
            });
        });
    }

    // Persist collapsed state to localStorage whenever it changes.
    #[cfg(target_arch = "wasm32")]
    {
        Effect::new(move |_| {
            let val = collapsed.get();
            if let Some(storage) = web_sys::window()
                .and_then(|w| w.local_storage().ok().flatten())
            {
                let _ = storage.set_item("sidebar_collapsed", if val { "true" } else { "false" });
            }
        });
    }

    // Initialize feedback context collector (console.error interception).
    // Must run once at WASM startup before any errors could occur.
    #[cfg(target_arch = "wasm32")]
    crate::utils::feedback_context::init();

    // Detect mobile on mount + resize
    #[cfg(target_arch = "wasm32")]
    {
        use send_wrapper::SendWrapper;
        use wasm_bindgen::prelude::*;

        // Check initial viewport width on mount
        Effect::new(move |_| {
            let window = web_sys::window().expect("window");
            let width = window
                .inner_width()
                .ok()
                .and_then(|v| v.as_f64())
                .unwrap_or(1024.0);
            let mobile = width < 768.0;
            set_is_mobile.set(mobile);
        });

        // Listen for resize — attached once, cleaned up on unmount
        {
            let window = web_sys::window().expect("window");
            let cb = Closure::<dyn Fn()>::new(move || {
                let Some(w) = web_sys::window() else { return };
                let width = w
                    .inner_width()
                    .ok()
                    .and_then(|v| v.as_f64())
                    .unwrap_or(1024.0);
                let mobile = width < 768.0;
                set_is_mobile.set(mobile);
                if mobile {
                    set_mobile_open.set(false);
                }
            });
            let _ = window.add_event_listener_with_callback(
                "resize",
                cb.as_ref().unchecked_ref(),
            );
            let cb_ref: js_sys::Function =
                cb.as_ref().unchecked_ref::<js_sys::Function>().clone();
            let cb_wrapper = SendWrapper::new(cb);
            let window_clone = window;
            on_cleanup(move || {
                let _ =
                    window_clone.remove_event_listener_with_callback("resize", &cb_ref);
                drop(cb_wrapper);
            });
        }
    }

    // Close mobile sidebar on navigation
    let location = leptos_router::hooks::use_location();
    Effect::new(move |_| {
        let _ = location.pathname.get();
        if is_mobile.get_untracked() {
            set_mobile_open.set(false);
        }
    });

    // Subscription gate: redirect to billing when trial expired or subscription cancelled.
    // Only enforced for non-personal-mode users navigating to non-settings pages.
    #[cfg(target_arch = "wasm32")]
    let navigate_billing = use_navigate();
    #[cfg(target_arch = "wasm32")]
    {
        let user_info_for_sub_gate = user_info;
        let location_for_gate = leptos_router::hooks::use_location();
        Effect::new(move || {
            // Don't gate until auth is confirmed
            if !auth_confirmed.get() {
                return;
            }
            let Some(Ok(user)) = user_info_for_sub_gate.get() else { return };
            // Personal mode / self-hosted don't have billing
            if user.is_personal_mode {
                return;
            }
            // Don't gate the settings/billing page itself (or any settings page)
            let path = location_for_gate.pathname.get();
            if path.starts_with("/settings") || path.starts_with("/login") || path.starts_with("/signup") {
                return;
            }
            // Stripe manages trial expiry via webhooks. When the trial ends
            // without a payment method, Stripe fires `invoice.payment_failed`
            // which sets subscription_status to "past_due". We only need to
            // gate on the final status — no client-side trial_ends_at check.
            let needs_gate = matches!(
                user.subscription_status.as_str(),
                "cancelled" | "past_due"
            );
            if needs_gate {
                navigate_billing("/settings/billing", NavigateOptions::default());
            }
        });
    }

    view! {
        // Auth guard loading screen — shown while auth check is in progress.
        // Hidden once auth_confirmed becomes true (user is authenticated).
        // Matches React `<ProtectedRoute>` loading state.
        <div
            class="min-h-screen flex items-center justify-center"
            style=move || if auth_confirmed.get() { "display:none" } else { "" }
        >
            "Loading..."
        </div>
        // Main layout — hidden until auth is confirmed to prevent
        // unauthenticated users from seeing the sidebar/app shell.
        // If auth fails, refresh_and_reload() redirects to /login
        // before this ever becomes visible.
        <div style=move || if auth_confirmed.get() { "" } else { "display:none" }>
            <WebSocketProvider user_id=ws_user_id.into() workspace_id=ws_workspace_id.into()>
                // Bridges Layout-level QueryCache to the WS `dashboard_update`
                // channel so list pages stay fresh without each page owning
                // its own subscription. See [KYO-9].
                <QueryCacheWsBridge/>
                // Drives the bootstrap/delta/live sync protocol over WebSocket
                // and keeps SyncStore + IndexedDB current. See [KYO-169].
                <SyncEngineStarter workspace_id=ws_workspace_id.into()/>
                <div class="h-dvh flex flex-col bg-background app-shell">
                    // ── Mobile header bar (md:hidden) — matches React Sidebar.jsx line 266 ──
                    <div class="mobile-header md:hidden fixed top-0 left-0 right-0 h-16 bg-background border-b border-border z-40 flex items-center px-4">
                        <button
                            on:click=move |_| set_mobile_open.update(|o| *o = !*o)
                            class="p-2 hover:bg-secondary rounded-lg transition-colors relative z-10"
                            aria-label="Toggle menu"
                        >
                            <svg class="w-5 h-5 text-foreground" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                                <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M4 6h16M4 12h16M4 18h16"/>
                            </svg>
                        </button>
                        <div class="absolute left-1/2 -translate-x-1/2">
                            <img src="/kyomi_full_logo.svg" alt="Kyomi" class="h-10 dark:hidden"/>
                            <img src="/kyomi_full_logo_white.svg" alt="Kyomi" class="h-10 hidden dark:block"/>
                        </div>
                    </div>

                    // ── Mobile overlay (matches React line 283) ─────────────────
                    <Show when=move || is_mobile.get() && mobile_open.get()>
                        <div
                            class="fixed inset-0 z-20 md:hidden"
                            style="background-color: var(--color-overlay)"
                            on:click=move |_| set_mobile_open.set(false)
                        />
                    </Show>

                    <div class="flex relative flex-1 overflow-hidden">
                        <Sidebar
                            collapsed=collapsed
                            set_collapsed=set_collapsed
                            is_mobile=is_mobile
                            mobile_open=mobile_open
                        />
                        <main
                            class=move || format!(
                                "flex-1 overflow-y-auto transition-[margin,opacity] duration-300 ease-in-out {}",
                                if is_routing.get() { "opacity-60 pointer-events-none" } else { "" }
                            )
                            style=move || {
                                if is_mobile.get() {
                                    // Mobile: no sidebar margin, top padding for the fixed header
                                    "padding-top: 4rem".to_string()
                                } else if collapsed.get() {
                                    "margin-left: 4rem".to_string()
                                } else {
                                    "margin-left: 20rem".to_string()
                                }
                            }
                        >
                            {children()}
                        </main>
                    </div>
                </div>
            </WebSocketProvider>
        </div>
    }
}

// Hydration from IndexedDB is handled by sync_engine::hydrate_store_from_db,
// called from the Layout hydration Effect above.

/// Bridges the Layout-level [`QueryCache`] to relevant WebSocket channels.
///
/// Hoists non-sync subscriptions up to the Layout so they survive navigation.
/// Subscriptions for entity types managed by the sync engine
/// (`dashboard_update` → dashboards/knowledge, `watch_update` → watches,
/// `chat_sessions` → chat session list) have been removed — the sync engine
/// keeps those stores current via `SyncStore` and IndexedDB (KYO-169).
///
/// Remaining subscriptions:
/// - `session_created`, `title_update`, `shared_conversation_activity`:
///   invalidate the sidebar's `recent_sessions` cache (KYO-23).
/// - `watch_alert`, `watch_state_update`: invalidate the unread alerts badge.
/// - `datasource_update`: invalidate the datasources list.
///
/// Rendered as a child of `<WebSocketProvider>` (not directly in `Layout`)
/// so that `use_context::<WebSocketContext>()` resolves — the provider
/// installs its context into its own subtree, not its parent's scope.
#[component]
fn QueryCacheWsBridge() -> impl IntoView {
    // The whole effect is WASM-only because `WebSocketContext` is never
    // installed on SSR/native targets — mirrors the per-page pattern that
    // this replaces.
    #[cfg(target_arch = "wasm32")]
    {
        use crate::components::chat::websocket_client::WebSocketContext;

        let query_cache = expect_context::<QueryCache>();
        let ws_ctx = use_context::<WebSocketContext>();

        Effect::new(move |_| {
            let Some(ws) = ws_ctx.as_ref().cloned() else {
                return;
            };

            // ── Recent sessions cache invalidation (KYO-23) ──────────
            // New chat created — sidebar should show it.
            let cache_sessions_1 = query_cache;
            let session_created_unsub = ws.subscribe("session_created", move |_msg| {
                cache_sessions_1.invalidate("recent_sessions");
            });

            // Chat title changed — sidebar should reflect the new title.
            let cache_sessions_2 = query_cache;
            let title_update_unsub = ws.subscribe("title_update", move |_msg| {
                cache_sessions_2.invalidate("recent_sessions");
            });

            // New message in a shared conversation — sidebar list may
            // reorder by recency.
            let cache_sessions_3 = query_cache;
            let shared_activity_unsub = ws.subscribe("shared_conversation_activity", move |_msg| {
                cache_sessions_3.invalidate("recent_sessions");
            });

            // ── Unread alerts cache invalidation (KYO-23) ────────────
            // Alert triggered — badge count should increase.
            let cache_alerts_1 = query_cache;
            let watch_alert_unsub = ws.subscribe("watch_alert", move |_msg| {
                cache_alerts_1.invalidate("unread_alerts");
            });

            // Watch execution status changed — badge count may change
            // (e.g. execution completed, alert triggered or cleared).
            let cache_alerts_2 = query_cache;
            let watch_state_unsub = ws.subscribe("watch_state_update", move |_msg| {
                cache_alerts_2.invalidate("unread_alerts");
            });

            // Datasource CRUD mutation — create/update/delete of a datasource by
            // this user or another workspace member. Refresh the datasources list.
            let cache_datasources = query_cache;
            let datasource_update_unsub = ws.subscribe("datasource_update", move |_msg| {
                cache_datasources.invalidate("datasources");
            });

            let session_created_unsub = send_wrapper::SendWrapper::new(session_created_unsub);
            let title_update_unsub = send_wrapper::SendWrapper::new(title_update_unsub);
            let shared_activity_unsub = send_wrapper::SendWrapper::new(shared_activity_unsub);
            let watch_alert_unsub = send_wrapper::SendWrapper::new(watch_alert_unsub);
            let watch_state_unsub = send_wrapper::SendWrapper::new(watch_state_unsub);
            let datasource_update_unsub = send_wrapper::SendWrapper::new(datasource_update_unsub);
            on_cleanup(move || {
                session_created_unsub.take()();
                title_update_unsub.take()();
                shared_activity_unsub.take()();
                watch_alert_unsub.take()();
                watch_state_unsub.take()();
                datasource_update_unsub.take()();
            });
        });
    }

    view! { <></> }
}

/// Starts the KYO-169 sync engine once the WebSocket is connected and the
/// workspace ID is available.
///
/// Rendered as a sibling of `<QueryCacheWsBridge/>` — both must sit inside
/// `<WebSocketProvider>` so that `use_context::<WebSocketContext>()` resolves.
///
/// The component is invisible (returns an empty fragment). Its only job is to
/// call [`crate::cache::sync_engine::start_sync_engine`] at the right moment.
/// The sync engine itself watches `connection_state` to re-send sync requests
/// on reconnect, so start_sync_engine is called at most once per workspace_id.
#[component]
fn SyncEngineStarter(
    /// Reactive signal with the current workspace ID, or `None` if not yet
    /// authenticated. Passed in from the Layout scope (same value provided to
    /// `WebSocketProvider`). The engine starts as soon as this becomes `Some`.
    workspace_id: Signal<Option<String>>,
) -> impl IntoView {
    // The whole effect is WASM-only: IndexedDB and WebSocket are browser APIs.
    // On SSR, workspace_id is unused — mark it explicitly so the compiler
    // doesn't emit an `unused_variables` warning (lint suppressions are banned).
    #[cfg(not(target_arch = "wasm32"))]
    let _ = workspace_id;

    #[cfg(target_arch = "wasm32")]
    {
        use crate::cache::store::SyncStore;
        use crate::cache::sync_engine::start_sync_engine;
        use crate::components::chat::websocket_client::WebSocketContext;

        let store = expect_context::<SyncStore>();
        let ws_ctx = use_context::<WebSocketContext>();

        // Track whether the engine has already been started for the current
        // workspace so we don't call start_sync_engine more than once per
        // workspace. Stored as a StoredValue so the closure can mutate it.
        let started_for: StoredValue<Option<String>> = StoredValue::new(None);

        Effect::new(move |_| {
            let Some(ref wid) = workspace_id.get() else {
                return;
            };
            if wid.is_empty() {
                return;
            }
            // Already started for this workspace — nothing to do.
            if started_for.get_value().as_deref() == Some(wid.as_str()) {
                return;
            }
            let Some(ws) = ws_ctx.as_ref().cloned() else {
                return;
            };
            // Mark as started before calling so a reactive re-run from inside
            // start_sync_engine can't trigger a second start.
            started_for.set_value(Some(wid.clone()));
            tracing::info!(workspace_id = %wid, "SyncEngineStarter: starting sync engine");
            start_sync_engine(ws, store, wid.clone());
        });
    }

    view! { <></> }
}

/// Navigation sidebar matching React `Sidebar.jsx`.
///
/// On mobile: hidden by default, shown as overlay when `mobile_open` is true.
/// On desktop: always visible, collapsible.
#[component]
fn Sidebar(
    collapsed: ReadSignal<bool>,
    set_collapsed: WriteSignal<bool>,
    is_mobile: ReadSignal<bool>,
    mobile_open: ReadSignal<bool>,
) -> impl IntoView {
    // Recent sessions — cached at the Layout level via QueryCache (KYO-23).
    // Stale-while-revalidate: on re-mount the sidebar shows the last-known
    // list instantly (no skeleton flash) and refreshes in the background.
    // WS events (session_created, title_update, shared_conversation_activity)
    // invalidate the cache via QueryCacheWsBridge.
    let sessions = use_query("recent_sessions", || (), |()| get_recent_sessions());
    let user_info = Resource::new(|| (), |_| get_sidebar_user());
    // Unread alerts count — cached at the Layout level via QueryCache (KYO-23).
    // use_query initialises with None and fetches via spawn_local, so it is
    // safe from hydration mismatch (same guarantee as the old LocalResource).
    // WS events (watch_alert, watch_state_update) invalidate via QueryCacheWsBridge.
    let unread_alerts = use_query("unread_alerts", || (), |()| get_unread_alerts_count());
    // Landing page preference — powers the sidebar logo link (KYO-66) so
    // clicking the logo jumps straight to the user's configured landing
    // page. Cached via use_query: HomePage reads the same key on its own
    // resolver run, so navigating `/` after a logo click is a cache hit.
    let landing_config = use_query("landing_config", || (), |()| get_landing_config());
    let landing_href = Signal::derive(move || match landing_config.get() {
        Some(Ok(cfg)) => crate::pages::home::resolve_redirect_target(&cfg),
        // Config not yet loaded or server error — fall back to /chat, which
        // matches the default-landing behaviour in `resolve_redirect_target`
        // when no preference is set.
        Some(Err(_)) | None => "/chat".to_string(),
    });
    // Dashboards nav href — personal default > workspace default > list.
    // Independent of `landing_page`: even when the user's landing page is
    // `chat`, the Dashboards nav item should still deep-link to their
    // configured default dashboard. See `resolve_dashboards_nav_href` for
    // the priority rationale.
    let dashboards_href = Signal::derive(move || match landing_config.get() {
        Some(Ok(cfg)) => crate::pages::home::resolve_dashboards_nav_href(&cfg),
        // Fetch in flight or error — go to the list, same as if no default
        // were configured. Do NOT fall back to `/chat` here (that's the
        // landing-page fallback, wrong for a nav link).
        Some(Err(_)) | None => "/dashboards".to_string(),
    });
    let (user_menu_open, set_user_menu_open) = signal(false);
    let (feedback_open, set_feedback_open) = signal(false);

    // Active state for "New Chat" — exact match on /chat only (not /chats or /chat/xxx).
    let pathname = leptos_router::hooks::use_location().pathname;
    let new_chat_active = Memo::new(move |_| pathname.get() == "/chat");

    // Effective collapsed state: the persisted desktop preference is ignored
    // when the mobile overlay is showing — the overlay is full-width and must
    // render labels and expanded spacing.
    let effective_collapsed = Signal::derive(move || collapsed.get() && !is_mobile.get());

    // Logout action — calls POST /leptos-api/logout to revoke the current session
    // and clear HTTPOnly cookies, then navigates to /login.
    // Matches React: AuthContext.jsx `logout()` + Sidebar.jsx `handleLogout()`.
    let navigate = use_navigate();
    let logout_action = Action::new(move |_: &()| {
        let nav = navigate.clone();
        async move {
            // Best-effort: clear the session server-side even if the call fails.
            let _ = logout().await;
            nav("/login", NavigateOptions { replace: true, ..Default::default() });
        }
    });

    // Listen for 'sessions-deleted' custom events to refresh the recent sessions list.
    // Matches React: Sidebar.jsx lines 171-173.
    // Uses QueryCache invalidation instead of Resource::refetch (KYO-23).
    #[cfg(target_arch = "wasm32")]
    {
        use send_wrapper::SendWrapper;
        use wasm_bindgen::prelude::*;

        let query_cache = expect_context::<QueryCache>();
        let window = web_sys::window().expect("window");
        let cb = Closure::<dyn Fn(web_sys::Event)>::new(move |_: web_sys::Event| {
            query_cache.invalidate("recent_sessions");
        });
        let _ = window.add_event_listener_with_callback(
            "sessions-deleted",
            cb.as_ref().unchecked_ref(),
        );
        let cb_ref: js_sys::Function =
            cb.as_ref().unchecked_ref::<js_sys::Function>().clone();
        let cb_wrapper = SendWrapper::new(cb);
        let window_clone = window;
        on_cleanup(move || {
            let _ =
                window_clone.remove_event_listener_with_callback("sessions-deleted", &cb_ref);
            drop(cb_wrapper);
        });
    }

    // Matches React: isMobile && isSidebarCollapsed ? 'hidden' : 'flex'
    // + isMobile ? 'top-16 bottom-0' : 'inset-y-0'
    view! {
        <div
            class="bg-[var(--color-sidebar)] border-r border-[var(--color-sidebar-border)] text-[var(--color-sidebar-foreground)] flex-col z-30 absolute left-0 shadow-lg transition-[width,transform] duration-300 ease-in-out"
            style=move || {
                let width = if is_mobile.get() {
                    "width: 20rem"
                } else if collapsed.get() {
                    "width: 4rem"
                } else {
                    "width: 20rem"
                };
                let display = if is_mobile.get() && !mobile_open.get() {
                    "display: none"
                } else {
                    "display: flex; flex-direction: column"
                };
                let pos = if is_mobile.get() {
                    "top: 4rem; bottom: 0"
                } else {
                    "top: 0; bottom: 0"
                };
                format!("{width}; {display}; {pos}")
            }
        >
            // ── Header: collapse toggle + logo (hidden on mobile) ──────────
            // React: "hidden md:flex px-3 h-16 border-b border-border items-center justify-between"
            <div class="hidden md:flex px-3 h-16 border-b border-[var(--color-sidebar-border)] items-center justify-between">
                <div class="flex items-center">
                    // Collapse toggle — React places this FIRST (left side)
                    <button
                        on:click=move |_| set_collapsed.update(|c| *c = !*c)
                        class="p-3 hover:bg-[var(--color-sidebar-hover)] rounded-md transition-colors flex-shrink-0 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
                    >
                        <span class="text-[var(--color-sidebar-foreground-secondary)]"><Icon icon=phosphor_leptos::SIDEBAR weight=IconWeight::Light size="20px"/></span>
                    </button>
                    // Logo — full logo to the right of the toggle, fades when collapsed.
                    // React: kyomi_full_logo_white.svg at h-12 in expanded mode.
                    // KYO-66: logo links to the user's configured landing page.
                    // `landing_href` is derived from `get_landing_config` via
                    // `resolve_redirect_target` (shared with HomePage), so the
                    // href reflects the user's preference without duplicating
                    // the resolver.
                    <div
                        class="flex items-center overflow-hidden transition-[width,opacity] duration-300"
                        style=move || {
                            if collapsed.get() { "opacity: 0; width: 0; margin-left: 0" } else { "opacity: 1; margin-left: 0.5rem" }
                        }
                    >
                        <a
                            href=move || landing_href.get()
                            aria-label="Go to home"
                            class="flex items-center hover:opacity-80 transition-opacity duration-200 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring rounded-md"
                        >
                            <img src="/kyomi_full_logo_white.svg" alt="Kyomi" class="h-12"/>
                        </a>
                    </div>
                </div>
            </div>

            // ── Navigation ─────────────────────────────────────────────────
            // React: "flex-1 flex flex-col px-3 py-4 min-h-0"
            <div class="flex-1 flex flex-col px-3 py-4 min-h-0 overflow-hidden">
                <div class="space-y-1 mb-4">
                    // New Chat — special styling with amber circle + plus icon
                    // Active state: amber accent when path is exactly /chat
                    <a
                        href="/chat"
                        class=move || {
                            let spacing = if effective_collapsed.get() { "gap-3 px-2.5" } else { "gap-3 pl-2.5 pr-3 py-2.5" };
                            if new_chat_active.get() {
                                format!("w-full min-h-[44px] flex items-center rounded-lg transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring bg-[rgba(217,119,6,0.12)] text-amber-500 {spacing}")
                            } else {
                                format!("w-full min-h-[44px] flex items-center rounded-lg hover:bg-[var(--color-sidebar-hover)] transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring {spacing}")
                            }
                        }
                    >
                        <div class="w-5 h-5 rounded-full flex items-center justify-center bg-primary flex-shrink-0">
                            <span class="text-primary-foreground"><Icon icon=phosphor_leptos::PLUS weight=IconWeight::Bold size="12px"/></span>
                        </div>
                        <span
                            class="text-sm font-medium text-[var(--color-sidebar-foreground)] whitespace-nowrap overflow-hidden transition-opacity duration-300"
                            style=move || if effective_collapsed.get() { "opacity: 0" } else { "opacity: 1" }
                        >
                            "New chat"
                        </span>
                    </a>

                    <NavItem href=Signal::stored("/chats".to_string()) icon=phosphor_leptos::CHATS label="Chats" collapsed=effective_collapsed also_matches="/chat/"/>
                    // `also_matches="/dashboard"` (no trailing slash) covers
                    // the list `/dashboards`, detail `/dashboard/:id`, and
                    // editor `/dashboard/:id/edit` in one prefix. Needed
                    // because `href` is now dynamic — when it resolves to
                    // `/dashboard/<user-default-id>` the list path no longer
                    // satisfies `starts_with(href)`, so `also_matches` is the
                    // one that keeps the nav item active on the list page.
                    <NavItem href=dashboards_href icon=phosphor_leptos::CHART_BAR label="Dashboards" collapsed=effective_collapsed also_matches="/dashboard"/>
                    <NavItem
                        href=Signal::stored("/watches".to_string())
                        icon=phosphor_leptos::EYE
                        label="Watches"
                        collapsed=effective_collapsed
                        badge_count=Signal::derive(move || {
                            match unread_alerts.get() {
                                Some(Ok(count)) => count,
                                // Resource not yet loaded, or server fn error — show no badge
                                Some(Err(_)) | None => 0,
                            }
                        })
                    />
                    <NavItem href=Signal::stored("/knowledge".to_string()) icon=phosphor_leptos::BOOK_OPEN label="Knowledge" collapsed=effective_collapsed/>
                    <NavItem href=Signal::stored("/sql-editor".to_string()) icon=phosphor_leptos::DATABASE label="SQL Editor" collapsed=effective_collapsed/>
                </div>

                // ── Recent Chats ───────────────────────────────────────────
                // React: "border-t border-border pt-4 flex-1 flex flex-col min-h-0"
                <div
                    class="border-t border-[var(--color-sidebar-border)] pt-4 flex-1 flex flex-col min-h-0 transition-opacity duration-300"
                    style=move || if effective_collapsed.get() { "opacity: 0; pointer-events: none" } else { "opacity: 1" }
                >
                    <div class="flex items-center justify-between px-3 mb-2">
                        <div class="text-xs text-[var(--color-sidebar-foreground-secondary)] font-medium">"Recent Chats"</div>
                    </div>
                    <div class="space-y-1 overflow-y-auto scrollbar-sidebar">
                        // sessions is a use_query Signal (not a Resource), so
                        // Transition/Suspense won't track it. Handle the None
                        // (loading) state explicitly in the match below.
                        {move || match sessions.get() {
                            None => view! {
                                <div class="text-xs text-[var(--color-sidebar-foreground-secondary)] px-3 py-2 italic">"Loading..."</div>
                            }.into_any(),
                            Some(Ok(sessions)) if sessions.is_empty() => view! {
                                <EmptyState
                                    variant=EmptyStateVariant::Sidebar
                                    title="No chats yet"
                                    description="Start a conversation to see it here"
                                    class="py-4 px-2 border-0"
                                />
                            }.into_any(),
                            Some(Ok(sessions)) => {
                                let current_path = pathname;
                                view! {
                                    <For
                                        each=move || sessions.clone()
                                        key=|s| s.session_id.clone()
                                        let:session
                                    >
                                        {
                                            let session_href = format!("/chat/{}", session.session_id);
                                            let session_href_cmp = session_href.clone();
                                            let title = session.title.clone();
                                            view! {
                                                <a
                                                    href=session_href
                                                    class=move || {
                                                        let active = current_path.get() == session_href_cmp;
                                                        if active {
                                                            "block px-3 py-2.5 rounded-lg cursor-pointer transition-colors text-sm text-amber-500 bg-[rgba(217,119,6,0.12)] truncate focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
                                                        } else {
                                                            "block px-3 py-2.5 rounded-lg cursor-pointer transition-colors text-sm text-[var(--color-sidebar-foreground)] hover:bg-[var(--color-sidebar-hover)] truncate focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
                                                        }
                                                    }
                                                >
                                                    {title}
                                                </a>
                                            }
                                        }
                                    </For>
                                }
                            }.into_any(),
                            Some(Err(_)) => view! {
                                <EmptyState
                                    variant=EmptyStateVariant::Sidebar
                                    title="No chats yet"
                                    description="Start a conversation to see it here"
                                    class="py-4 px-2 border-0"
                                />
                            }.into_any(),
                        }}
                    </div>
                </div>
            </div>

            // ── User Account Section ───────────────────────────────────────
            // React: "border-t border-border px-3 py-4 relative"
            // py-2 (not py-4) — button has min-h-[44px] for touch targets, reduced padding compensates
            <div class="border-t border-[var(--color-sidebar-border)] px-3 py-2 relative">
                <Transition fallback=|| ()>
                    {move || user_info.get().map(|result| {
                        let user = match result {
                            Ok(u) => u,
                            Err(_) => return view! { <span></span> }.into_any(),
                        };

                        let initial = if user.is_personal_mode {
                            None
                        } else {
                            Some(user.name.as_deref()
                                .or(Some(&user.email))
                                .map(|s| s.chars().next().unwrap_or('U').to_string())
                                .unwrap_or_else(|| "U".to_string()))
                        };
                        let is_personal_avatar = user.is_personal_mode;

                        let display_name = if user.is_personal_mode {
                            "Settings".to_string()
                        } else {
                            user.name.clone().unwrap_or_else(|| user.email.clone())
                        };

                        let workspace = user.workspace_name.clone().unwrap_or_else(|| "My Workspace".to_string());
                        let is_personal = user.is_personal_mode;
                        let show_feedback = !user.is_personal_mode && !user.is_self_hosted;
                        let email = user.email.clone();
                        let trigger_ref = NodeRef::<leptos::html::Div>::new();

                        view! {
                            <div node_ref=trigger_ref>
                                <button
                                    on:click=move |_| set_user_menu_open.update(|o| *o = !*o)
                                    class=move || format!(
                                        "flex items-center w-full min-h-[44px] hover:bg-[var(--color-sidebar-hover)] rounded-lg transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring {}",
                                        if effective_collapsed.get() { "gap-3 px-2" } else { "gap-3 pl-2 pr-3 py-1" }
                                    )
                                >
                                    <div class="w-6 h-6 bg-primary rounded-full flex items-center justify-center flex-shrink-0">
                                        {if is_personal_avatar {
                                            view! { <span class="text-primary-foreground"><Icon icon=phosphor_leptos::GEAR weight=IconWeight::Fill size="14px"/></span> }.into_any()
                                        } else {
                                            view! { <span class="text-xs font-medium text-primary-foreground">{initial.clone()}</span> }.into_any()
                                        }}
                                    </div>
                                    <div
                                        class="flex-1 min-w-0 text-left overflow-hidden transition-[width,opacity] duration-300"
                                        style=move || if effective_collapsed.get() { "opacity: 0; width: 0" } else { "opacity: 1" }
                                    >
                                        <div class="text-sm font-medium text-[var(--color-sidebar-foreground)] truncate">{display_name.clone()}</div>
                                        {if !is_personal {
                                            Some(view! { <div class="text-xs text-[var(--color-sidebar-foreground-secondary)] truncate">{workspace.clone()}</div> })
                                        } else {
                                            None
                                        }}
                                    </div>
                                    <span
                                        class="text-[var(--color-sidebar-foreground-secondary)] flex-shrink-0 transition-[opacity,width,transform] duration-300"
                                        style=move || {
                                            let visibility = if effective_collapsed.get() { "opacity: 0; width: 0" } else { "opacity: 1" };
                                            let rotation = if user_menu_open.get() { "; transform: rotate(180deg)" } else { "; transform: rotate(0deg)" };
                                            format!("{visibility}{rotation}")
                                        }
                                    >
                                        <Icon icon=phosphor_leptos::CARET_DOWN weight=IconWeight::Regular size="16px"/>
                                    </span>
                                </button>

                                <Popover
                                    trigger_ref=trigger_ref
                                    open=Signal::derive(move || user_menu_open.get())
                                    on_close=Callback::new(move |()| set_user_menu_open.set(false))
                                    placement=Placement::TOP_START
                                    class="bg-[var(--color-sidebar)] border border-[var(--color-sidebar-border)] rounded-lg shadow-[0_8px_24px_rgba(0,0,0,0.3)] py-1 min-w-48"
                                >
                                    // User info header — hide in personal mode
                                    {if !is_personal {
                                        Some(view! {
                                            <div class="px-4 py-3 border-b border-[var(--color-sidebar-border)]">
                                                <div class="text-sm font-medium text-[var(--color-sidebar-foreground)]">{display_name.clone()}</div>
                                                <div class="text-xs text-[var(--color-sidebar-foreground-secondary)] truncate">{email.clone()}</div>
                                            </div>
                                        })
                                    } else {
                                        None
                                    }}
                                    <a
                                        href="/settings"
                                        on:click=move |_| set_user_menu_open.set(false)
                                        class="w-full text-left px-4 py-2 text-sm text-[var(--color-sidebar-foreground)] transition-colors hover:bg-[var(--color-sidebar-hover)] flex items-center space-x-3"
                                    >
                                        <Icon icon=phosphor_leptos::GEAR weight=IconWeight::Light size="16px"/>
                                        <span>"Settings"</span>
                                    </a>
                                    <a
                                        href="https://kyomi.ai/docs"
                                        target="_blank"
                                        rel="noopener noreferrer"
                                        class="w-full text-left px-4 py-2 text-sm text-[var(--color-sidebar-foreground)] transition-colors hover:bg-[var(--color-sidebar-hover)] flex items-center space-x-3"
                                    >
                                        <Icon icon=phosphor_leptos::BOOK_OPEN weight=IconWeight::Light size="16px"/>
                                        <span>"Help & Docs"</span>
                                    </a>
                                    {if show_feedback {
                                        Some(view! {
                                            <button
                                                on:click=move |_| {
                                                    set_user_menu_open.set(false);
                                                    set_feedback_open.set(true);
                                                }
                                                class="w-full text-left px-4 py-2 text-sm text-[var(--color-sidebar-foreground)] transition-colors hover:bg-[var(--color-sidebar-hover)] flex items-center space-x-3"
                                            >
                                                <Icon icon=phosphor_leptos::CHAT_CIRCLE_DOTS weight=IconWeight::Light size="16px"/>
                                                <span>"Send Feedback"</span>
                                            </button>
                                        })
                                    } else {
                                        None
                                    }}
                                    {if !is_personal {
                                        Some(view! {
                                            <button
                                                on:click=move |_| {
                                                    set_user_menu_open.set(false);
                                                    logout_action.dispatch(());
                                                }
                                                class="w-full text-left px-4 py-2 text-sm text-error-foreground transition-colors hover:bg-error/10 flex items-center space-x-3"
                                            >
                                                <Icon icon=phosphor_leptos::SIGN_OUT weight=IconWeight::Light size="16px"/>
                                                <span>"Logout"</span>
                                            </button>
                                        })
                                    } else {
                                        None
                                    }}
                                </Popover>
                            </div>
                        }.into_any()
                    })}
                </Transition>
            </div>
        </div>

        // Feedback modal — rendered outside the sidebar div so it uses the
        // standard app theme (Modal component handles bg-background).
        <FeedbackModal
            open=feedback_open
            on_open_change=Callback::new(move |open: bool| set_feedback_open.set(open))
        />
    }
}

/// A single navigation item in the sidebar.
///
/// Navy sidebar styling:
/// - Default: `text-[var(--color-sidebar-foreground)]`, `hover:bg-[var(--color-sidebar-hover)]`
/// - Active: amber accent background `bg-[rgba(217,119,6,0.12)]` with `text-amber-500`
/// - Collapsed: `gap-3 px-2.5`, Expanded: `gap-3 pl-2.5 pr-3 py-2.5`
/// - Icon: inherits text color from parent `<a>`
/// - Label: `text-sm font-medium whitespace-nowrap overflow-hidden transition-opacity duration-300`
#[component]
fn NavItem(
    /// Destination URL. Reactive so nav items like "Dashboards" can track
    /// per-user defaults (personal > workspace > list) from `landing_config`
    /// without the Sidebar having to special-case each one.
    href: Signal<String>,
    icon: phosphor_leptos::IconData,
    label: &'static str,
    collapsed: Signal<bool>,
    /// Optional badge count (e.g. unread alerts). When > 0, shows a count badge
    /// (expanded) or a dot indicator (collapsed). Mirrors React Sidebar.jsx badge.
    #[prop(optional, into)]
    badge_count: Option<Signal<i64>>,
    /// Additional path prefix that should also activate this nav item.
    /// Used when detail routes differ from list routes (e.g. `/dashboards` list
    /// vs `/dashboard/:id` detail).
    #[prop(optional)]
    also_matches: Option<&'static str>,
) -> impl IntoView {
    let count = badge_count.unwrap_or_else(|| Signal::derive(|| 0));
    let pathname = leptos_router::hooks::use_location().pathname;

    let is_active = Memo::new(move |_| {
        let path = pathname.get();
        let h = href.get();
        path.starts_with(h.as_str())
            || also_matches.is_some_and(|prefix| path.starts_with(prefix))
    });

    // Phosphor weight convention: Light when inactive, Fill when active.
    // The state change is at the shape level, not just color — amber outline + amber fill
    // makes Dashboards/Chats/etc unmistakable even when you squint.
    let icon_weight = Memo::new(move |_| {
        if is_active.get() { IconWeight::Fill } else { IconWeight::Light }
    });

    view! {
        <a
            href=move || href.get()
            class=move || {
                let active = is_active.get();
                let spacing = if collapsed.get() { "gap-3 px-2.5" } else { "gap-3 pl-2.5 pr-3 py-2.5" };
                if active {
                    format!("w-full min-h-[44px] flex items-center rounded-lg transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring bg-[rgba(217,119,6,0.12)] text-amber-500 {spacing}")
                } else {
                    format!("w-full min-h-[44px] flex items-center rounded-lg transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring hover:bg-[var(--color-sidebar-hover)] text-[var(--color-sidebar-foreground)] {spacing}")
                }
            }
        >
            <div class="relative flex-shrink-0">
                <span class="w-5 h-5 flex items-center justify-center">
                    <Icon icon=icon weight=icon_weight size="20px"/>
                </span>
                // Collapsed dot indicator — React: "absolute -top-1 -right-1 h-2 w-2 rounded-full bg-primary"
                {move || (count.get() > 0 && collapsed.get()).then(|| view! {
                    <span class="absolute -top-1 -right-1 h-2 w-2 rounded-full bg-primary"/>
                })}
            </div>
            <span
                class="text-sm font-medium whitespace-nowrap overflow-hidden transition-opacity duration-300"
                style=move || if collapsed.get() { "opacity: 0" } else { "opacity: 1" }
            >
                {label}
            </span>
            // Expanded badge — React: "ml-auto px-1.5 py-0.5 text-xs font-medium rounded-full bg-primary text-primary-foreground"
            {move || (count.get() > 0 && !collapsed.get()).then(|| {
                let n = count.get();
                let display = if n > 99 { "99+".to_string() } else { n.to_string() };
                view! {
                    <span class="ml-auto px-1.5 py-0.5 text-xs font-medium rounded-full bg-primary text-primary-foreground">
                        {display}
                    </span>
                }
            })}
        </a>
    }
}
