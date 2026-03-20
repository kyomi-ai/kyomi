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

use crate::server_fns::sidebar::{get_recent_sessions, get_sidebar_user};

/// Main layout shell wrapping all Leptos pages.
#[component]
pub fn Layout(children: Children) -> impl IntoView {
    let (collapsed, set_collapsed) = signal(false);

    view! {
        <div class="h-screen flex flex-col bg-background">
            <div class="flex relative flex-1 overflow-hidden">
                <Sidebar collapsed=collapsed set_collapsed=set_collapsed/>
                <main
                    class="flex-1 overflow-y-auto transition-all duration-300 ease-in-out"
                    style=move || {
                        if collapsed.get() { "margin-left: 4rem" } else { "margin-left: 20rem" }
                    }
                >
                    {children()}
                </main>
            </div>
        </div>
    }
}

/// Navigation sidebar matching React `Sidebar.jsx`.
#[component]
fn Sidebar(
    collapsed: ReadSignal<bool>,
    set_collapsed: WriteSignal<bool>,
) -> impl IntoView {
    let sessions = Resource::new(|| (), |_| get_recent_sessions());
    let user_info = Resource::new(|| (), |_| get_sidebar_user());
    let (user_menu_open, set_user_menu_open) = signal(false);

    view! {
        <div
            class="bg-background border-r border-border text-foreground flex flex-col z-30 absolute left-0 inset-y-0 shadow-lg transition-all duration-300 ease-in-out"
            style=move || {
                if collapsed.get() { "width: 4rem" } else { "width: 20rem" }
            }
        >
            // ── Header: collapse toggle + logo ─────────────────────────────
            // React: "hidden md:flex px-3 h-16 border-b border-border items-center justify-between"
            <div class="flex px-3 h-16 border-b border-border items-center justify-between">
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
                    <NavItem href="/watches" icon=icondata_lu::LuEye label="Watches" collapsed=collapsed/>
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
                        <Suspense fallback=|| view! {
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
                        </Suspense>
                    </div>
                </div>
            </div>

            // ── User Account Section ───────────────────────────────────────
            // React: "border-t border-border px-3 py-4 relative"
            <div class="border-t border-border px-3 py-4 relative">
                <Suspense fallback=|| ()>
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
                                                <a
                                                    href="/login"
                                                    class="w-full text-left px-4 py-2 text-sm text-error-foreground hover:bg-error/10 flex items-center space-x-3"
                                                >
                                                    <Icon icon=icondata_lu::LuLogOut width="16" height="16"/>
                                                    <span>"Logout"</span>
                                                </a>
                                            })
                                        } else {
                                            None
                                        }}
                                    </div>
                                </Show>
                            </div>
                        }.into_any()
                    })}
                </Suspense>
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
) -> impl IntoView {
    view! {
        <a
            href=href
            class=move || format!(
                "w-full h-10 flex items-center rounded-lg hover:bg-accent transition-colors {}",
                if collapsed.get() { "gap-3 px-2.5" } else { "gap-3 pl-2.5 pr-3 py-2.5" }
            )
        >
            <span class="w-5 h-5 text-muted-foreground flex-shrink-0 flex items-center justify-center">
                <Icon icon=icon width="20" height="20"/>
            </span>
            <span
                class="text-sm font-medium text-foreground whitespace-nowrap overflow-hidden transition-opacity duration-300"
                style=move || if collapsed.get() { "opacity: 0" } else { "opacity: 1" }
            >
                {label}
            </span>
        </a>
    }
}
