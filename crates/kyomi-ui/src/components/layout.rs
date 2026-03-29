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
use leptos_icons::Icon;
use leptos_router::hooks::use_navigate;
use leptos_router::NavigateOptions;

use crate::components::chat::WebSocketProvider;
use crate::server_fns::security::logout;
use crate::server_fns::sidebar::{get_recent_sessions, get_sidebar_user};
use crate::server_fns::watches::get_unread_alerts_count;

/// Main layout shell wrapping all Leptos pages.
///
/// Matches React Sidebar.jsx mobile behaviour:
/// - Mobile (<768px): sidebar hidden by default, hamburger header bar at top
/// - Desktop (768px+): sidebar always visible (collapsed/expanded)
#[component]
pub fn Layout(children: Children) -> impl IntoView {
    let (collapsed, set_collapsed) = signal(false);
    let (is_mobile, set_is_mobile) = signal(false);
    #[cfg(not(target_arch = "wasm32"))]
    let _ = set_is_mobile;
    let (mobile_open, set_mobile_open) = signal(false);

    // Fetch sidebar user info — used for both the sidebar UI and WebSocket auth signals.
    let user_info = Resource::new(|| (), |_| get_sidebar_user());

    // Auth guard: tracks whether the user has been authenticated.
    // Starts as `false`; set to `true` once `user_info` resolves successfully.
    // While `false`, the layout renders a loading state instead of the full
    // app shell, preventing unauthenticated users from seeing protected pages.
    // Matches React's `<ProtectedRoute>` in App.jsx.
    #[cfg(target_arch = "wasm32")]
    let (auth_confirmed, set_auth_confirmed) = signal(false);
    #[cfg(not(target_arch = "wasm32"))]
    let (auth_confirmed, _) = signal(false);

    // Auth guard: when user_info resolves, either confirm auth or redirect to /login.
    // On WASM: if auth fails, try token refresh first (handles expired access_token
    // with valid refresh_token). If refresh also fails, redirect to /login.
    // On SSR/native: just gate rendering — the client will handle the redirect.
    #[cfg(target_arch = "wasm32")]
    {
        use crate::utils::auth_refresh;
        let user_info_for_effect = user_info;
        Effect::new(move || {
            match user_info_for_effect.get() {
                Some(Ok(_)) => {
                    // Auth succeeded — allow layout to render
                    set_auth_confirmed.set(true);
                }
                Some(Err(e)) => {
                    let msg = e.to_string();
                    if auth_refresh::is_auth_error(&msg) {
                        auth_refresh::refresh_and_reload();
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
            if mobile {
                set_collapsed.set(true);
            }
        });

        // Listen for resize — attached once, cleaned up on unmount
        {
            let window = web_sys::window().expect("window");
            let cb = Closure::<dyn Fn()>::new(move || {
                let w = web_sys::window().unwrap();
                let width = w
                    .inner_width()
                    .ok()
                    .and_then(|v| v.as_f64())
                    .unwrap_or(1024.0);
                let mobile = width < 768.0;
                set_is_mobile.set(mobile);
                if mobile {
                    set_collapsed.set(true);
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
                <div class="h-screen flex flex-col bg-background">
                    // ── Mobile header bar (md:hidden) — matches React Sidebar.jsx line 266 ──
                    <div class="md:hidden fixed top-0 left-0 right-0 h-16 bg-background border-b border-border z-40 flex items-center px-4">
                        <button
                            on:click=move |_| set_mobile_open.update(|o| *o = !*o)
                            class="p-2 hover:bg-accent rounded-lg transition-colors relative z-10"
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
                            class="flex-1 overflow-y-auto transition-all duration-300 ease-in-out"
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
    let sessions = Resource::new(|| (), |_| get_recent_sessions());
    let user_info = Resource::new(|| (), |_| get_sidebar_user());
    // Fetch unread alerts count for the Watches sidebar badge.
    // Mirrors React's `useQuery(['unread-alerts-count'])` in Sidebar.jsx.
    // Uses LocalResource to avoid hydration mismatch — badge is client-only UI.
    let unread_alerts = LocalResource::new(get_unread_alerts_count);
    let (user_menu_open, set_user_menu_open) = signal(false);

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
    // Matches React: Sidebar.jsx lines 171-173
    #[cfg(target_arch = "wasm32")]
    {
        use send_wrapper::SendWrapper;
        use wasm_bindgen::prelude::*;

        let window = web_sys::window().expect("window");
        let cb = Closure::<dyn Fn(web_sys::Event)>::new(move |_: web_sys::Event| {
            sessions.refetch();
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
            class="bg-background border-r border-border text-foreground flex-col z-30 absolute left-0 shadow-lg transition-all duration-300 ease-in-out"
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
            <div class="hidden md:flex px-3 h-16 border-b border-border items-center justify-between">
                <div class="flex items-center">
                    // Collapse toggle — React places this FIRST (left side)
                    <button
                        on:click=move |_| set_collapsed.update(|c| *c = !*c)
                        class="p-2.5 hover:bg-accent rounded-md transition-colors flex-shrink-0"
                    >
                        <span class="text-muted-foreground"><Icon icon=icondata_lu::LuPanelLeft width="20" height="20"/></span>
                    </button>
                    // Logo — to the right of the toggle, fades when collapsed
                    <div
                        class="flex items-center overflow-hidden transition-all duration-300"
                        style=move || {
                            if collapsed.get() { "opacity: 0; width: 0; margin-left: 0" } else { "opacity: 1; margin-left: 0.5rem" }
                        }
                    >
                        // Light mode logo
                        <img src="/kyomi_full_logo.svg" alt="Kyomi" class="h-12 dark:hidden"/>
                        // Dark mode logo
                        <img src="/kyomi_full_logo_white.svg" alt="Kyomi" class="h-12 hidden dark:block"/>
                    </div>
                </div>
            </div>

            // ── Navigation ─────────────────────────────────────────────────
            // React: "flex-1 flex flex-col px-3 py-4 min-h-0"
            <div class="flex-1 flex flex-col px-3 py-4 min-h-0 overflow-hidden">
                <div class="space-y-1 mb-4">
                    // New Chat — special styling with amber circle + plus icon
                    <a
                        href="/chat"
                        class=move || format!(
                            "w-full h-10 flex items-center rounded-lg hover:bg-accent transition-colors {}",
                            if collapsed.get() { "gap-3 px-2.5" } else { "gap-3 pl-2.5 pr-3 py-2.5" }
                        )
                    >
                        <div class="w-5 h-5 rounded-full flex items-center justify-center bg-primary flex-shrink-0">
                            <span class="text-primary-foreground"><Icon icon=icondata_lu::LuPlus width="12" height="12"/></span>
                        </div>
                        <span
                            class="text-sm font-medium text-foreground whitespace-nowrap overflow-hidden transition-opacity duration-300"
                            style=move || if collapsed.get() { "opacity: 0" } else { "opacity: 1" }
                        >
                            "New chat"
                        </span>
                    </a>

                    <NavItem href="/chats" icon=icondata_lu::LuMessagesSquare label="Chats" collapsed=collapsed/>
                    <NavItem href="/dashboards" icon=icondata_lu::LuChartBar label="Dashboards" collapsed=collapsed/>
                    <NavItem
                        href="/watches"
                        icon=icondata_lu::LuEye
                        label="Watches"
                        collapsed=collapsed
                        badge_count=Signal::derive(move || {
                            match unread_alerts.get() {
                                Some(Ok(count)) => count,
                                // Resource not yet loaded, or server fn error — show no badge
                                Some(Err(_)) | None => 0,
                            }
                        })
                    />
                    <NavItem href="/knowledge" icon=icondata_lu::LuBookOpen label="Knowledge" collapsed=collapsed/>
                    <NavItem href="/sql-editor" icon=icondata_lu::LuDatabase label="SQL Editor" collapsed=collapsed/>
                </div>

                // ── Recent Chats ───────────────────────────────────────────
                // React: "border-t border-border pt-4 flex-1 flex flex-col min-h-0"
                <div
                    class="border-t border-border pt-4 flex-1 flex flex-col min-h-0 transition-opacity duration-300"
                    style=move || if collapsed.get() { "opacity: 0; pointer-events: none" } else { "opacity: 1" }
                >
                    <div class="flex items-center justify-between px-3 mb-2">
                        <div class="text-xs text-muted-foreground font-medium">"Recent Chats"</div>
                    </div>
                    <div class="space-y-1 overflow-y-auto">
                        <Transition fallback=|| view! {
                            <div class="text-xs text-muted-foreground px-3 py-2 italic">"Loading..."</div>
                        }>
                            {move || sessions.get().map(|result| match result {
                                Ok(sessions) if sessions.is_empty() => view! {
                                    <div class="text-xs text-muted-foreground px-3 py-2 italic">"No chats yet"</div>
                                }.into_any(),
                                Ok(sessions) => view! {
                                    <For
                                        each=move || sessions.clone()
                                        key=|s| s.session_id.clone()
                                        let:session
                                    >
                                        <a
                                            href=format!("/chat/{}", session.session_id)
                                            class="block px-3 py-2 rounded-lg cursor-pointer transition-all duration-300 text-sm text-foreground hover:bg-accent truncate"
                                        >
                                            {session.title.clone()}
                                        </a>
                                    </For>
                                }.into_any(),
                                Err(_) => view! {
                                    <div class="text-xs text-muted-foreground px-3 py-2 italic">"No chats yet"</div>
                                }.into_any(),
                            })}
                        </Transition>
                    </div>
                </div>
            </div>

            // ── User Account Section ───────────────────────────────────────
            // React: "border-t border-border px-3 py-4 relative"
            <div class="border-t border-border px-3 py-4 relative">
                <Transition fallback=|| ()>
                    {move || user_info.get().map(|result| {
                        let user = match result {
                            Ok(u) => u,
                            Err(_) => return view! { <span></span> }.into_any(),
                        };

                        let initial = if user.is_personal_mode {
                            "⚙".to_string()
                        } else {
                            user.name.as_deref()
                                .or(Some(&user.email))
                                .map(|s| s.chars().next().unwrap_or('U').to_string())
                                .unwrap_or_else(|| "U".to_string())
                        };

                        let display_name = if user.is_personal_mode {
                            "Settings".to_string()
                        } else {
                            user.name.clone().unwrap_or_else(|| user.email.clone())
                        };

                        let workspace = user.workspace_name.clone().unwrap_or_else(|| "My Workspace".to_string());
                        let is_personal = user.is_personal_mode;
                        let email = user.email.clone();

                        view! {
                            <div class="relative">
                                <button
                                    on:click=move |_| set_user_menu_open.update(|o| *o = !*o)
                                    class=move || format!(
                                        "flex items-center w-full h-10 hover:bg-accent rounded-lg transition-colors {}",
                                        if collapsed.get() { "gap-3 px-2" } else { "gap-3 pl-2 pr-3 py-2.5" }
                                    )
                                >
                                    <div class="w-6 h-6 bg-primary rounded-full flex items-center justify-center flex-shrink-0">
                                        <span class="text-xs font-medium text-primary-foreground">{initial.clone()}</span>
                                    </div>
                                    <div
                                        class="flex-1 min-w-0 text-left overflow-hidden transition-all duration-300"
                                        style=move || if collapsed.get() { "opacity: 0; width: 0" } else { "opacity: 1" }
                                    >
                                        <div class="text-sm font-medium text-foreground truncate">{display_name.clone()}</div>
                                        {if !is_personal {
                                            Some(view! { <div class="text-xs text-muted-foreground truncate">{workspace.clone()}</div> })
                                        } else {
                                            None
                                        }}
                                    </div>
                                    <span
                                        class="text-muted-foreground flex-shrink-0 transition-all duration-300"
                                        style=move || if collapsed.get() { "opacity: 0; width: 0" } else { "opacity: 1" }
                                    >
                                        <Icon icon=icondata_lu::LuChevronDown width="16" height="16"/>
                                    </span>
                                </button>

                                // User menu dropdown
                                <Show when=move || user_menu_open.get()>
                                    <div class="absolute bottom-full left-0 mb-2 bg-popover border border-border rounded-lg shadow-lg py-1 z-50 min-w-48">
                                        // User info header — hide in personal mode
                                        {if !is_personal {
                                            Some(view! {
                                                <div class="px-4 py-3 border-b border-border">
                                                    <div class="text-sm font-medium text-popover-foreground">{display_name.clone()}</div>
                                                    <div class="text-xs text-muted-foreground truncate">{email.clone()}</div>
                                                </div>
                                            })
                                        } else {
                                            None
                                        }}
                                        <a
                                            href="/settings"
                                            class="w-full text-left px-4 py-2 text-sm text-popover-foreground hover:bg-accent flex items-center space-x-3"
                                        >
                                            <Icon icon=icondata_lu::LuSettings width="16" height="16"/>
                                            <span>"Settings"</span>
                                        </a>
                                        <a
                                            href="https://kyomi.ai/docs"
                                            target="_blank"
                                            rel="noopener noreferrer"
                                            class="w-full text-left px-4 py-2 text-sm text-popover-foreground hover:bg-accent flex items-center space-x-3"
                                        >
                                            <Icon icon=icondata_lu::LuBookOpen width="16" height="16"/>
                                            <span>"Help & Docs"</span>
                                        </a>
                                        {if !is_personal {
                                            Some(view! {
                                                <button
                                                    on:click=move |_| {
                                                        set_user_menu_open.set(false);
                                                        logout_action.dispatch(());
                                                    }
                                                    class="w-full text-left px-4 py-2 text-sm text-error-foreground hover:bg-error/10 flex items-center space-x-3"
                                                >
                                                    <Icon icon=icondata_lu::LuLogOut width="16" height="16"/>
                                                    <span>"Logout"</span>
                                                </button>
                                            })
                                        } else {
                                            None
                                        }}
                                    </div>
                                </Show>
                            </div>
                        }.into_any()
                    })}
                </Transition>
            </div>
        </div>
    }
}

/// A single navigation item in the sidebar.
///
/// Matches React sidebar button classes exactly:
/// - `w-full h-10 flex items-center rounded-lg hover:bg-accent transition-colors`
/// - Collapsed: `gap-3 px-2.5`, Expanded: `gap-3 pl-2.5 pr-3 py-2.5`
/// - Icon: `w-5 h-5 text-muted-foreground flex-shrink-0`
/// - Label: `text-sm font-medium text-foreground whitespace-nowrap overflow-hidden transition-opacity duration-300`
#[component]
fn NavItem(
    href: &'static str,
    icon: &'static icondata_core::IconData,
    label: &'static str,
    collapsed: ReadSignal<bool>,
    /// Optional badge count (e.g. unread alerts). When > 0, shows a count badge
    /// (expanded) or a dot indicator (collapsed). Mirrors React Sidebar.jsx badge.
    #[prop(optional, into)]
    badge_count: Option<Signal<i64>>,
) -> impl IntoView {
    let count = badge_count.unwrap_or_else(|| Signal::derive(|| 0));

    view! {
        <a
            href=href
            class=move || format!(
                "w-full h-10 flex items-center rounded-lg hover:bg-accent transition-colors {}",
                if collapsed.get() { "gap-3 px-2.5" } else { "gap-3 pl-2.5 pr-3 py-2.5" }
            )
        >
            <div class="relative flex-shrink-0">
                <span class="w-5 h-5 text-muted-foreground flex items-center justify-center">
                    <Icon icon=icon width="20" height="20"/>
                </span>
                // Collapsed dot indicator — React: "absolute -top-1 -right-1 h-2 w-2 rounded-full bg-primary"
                {move || (count.get() > 0 && collapsed.get()).then(|| view! {
                    <span class="absolute -top-1 -right-1 h-2 w-2 rounded-full bg-primary"/>
                })}
            </div>
            <span
                class="text-sm font-medium text-foreground whitespace-nowrap overflow-hidden transition-opacity duration-300"
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
