// SPDX-License-Identifier: AGPL-3.0-or-later

//! Layout shell — sidebar + content area.
//!
//! Matches the React `Sidebar.jsx` layout structure and classes.
//! See `docs/DESIGN_SYSTEM.md` for layout patterns.
//!
//! Sidebar: shadow-lg (DESIGN_SYSTEM.md), absolute positioned, collapsible.
//! Nav items: w-full h-10, gap-3, text-sm font-medium, hover:bg-accent.

use leptos::prelude::*;
use leptos_icons::Icon;

/// Sidebar widths matching React: collapsed = 64px, expanded = 320px.
const SIDEBAR_COLLAPSED: &str = "64px";
const SIDEBAR_EXPANDED: &str = "320px";

/// Main layout shell wrapping all Leptos pages.
#[component]
pub fn Layout(children: Children) -> impl IntoView {
    let (expanded, set_expanded) = signal(true);

    view! {
        <div class="h-screen flex flex-col bg-background">
            <div class="flex relative flex-1 overflow-hidden">
                <Sidebar expanded=expanded set_expanded=set_expanded/>
                <main
                    class="flex-1 overflow-y-auto transition-[margin-left] duration-300"
                    style=move || {
                        if expanded.get() {
                            format!("margin-left: {SIDEBAR_EXPANDED}")
                        } else {
                            format!("margin-left: {SIDEBAR_COLLAPSED}")
                        }
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
    expanded: ReadSignal<bool>,
    set_expanded: WriteSignal<bool>,
) -> impl IntoView {
    view! {
        <aside
            class="absolute left-0 top-0 bottom-0 z-10 flex flex-col bg-card border-r border-border shadow-lg transition-[width] duration-300 overflow-hidden"
            style=move || {
                if expanded.get() {
                    format!("width: {SIDEBAR_EXPANDED}")
                } else {
                    format!("width: {SIDEBAR_COLLAPSED}")
                }
            }
        >
            // Header: logo + collapse toggle (h-16, border-b)
            <div class="h-16 flex items-center justify-between px-4 border-b border-border shrink-0">
                <a
                    href="/"
                    class="flex items-center gap-3 overflow-hidden"
                >
                    // Kyomi logo — amber circle with plus (matches React "New Chat" icon style)
                    <div class="w-8 h-8 rounded-full bg-primary flex items-center justify-center shrink-0">
                        <span class="text-primary-foreground font-bold text-sm">"K"</span>
                    </div>
                    <span
                        class="text-lg font-semibold text-foreground whitespace-nowrap transition-opacity duration-300"
                        style=move || if expanded.get() { "opacity: 1" } else { "opacity: 0" }
                    >
                        "Kyomi"
                    </span>
                </a>
                <button
                    class="p-1.5 rounded-md text-muted-foreground hover:text-foreground hover:bg-accent transition-colors shrink-0"
                    on:click=move |_| set_expanded.update(|e| *e = !*e)
                >
                    <Icon icon=icondata_lu::LuPanelLeft width="16" height="16"/>
                </button>
            </div>

            // Navigation items — matches React sidebar nav structure
            <nav class="flex-1 py-2 px-2 space-y-1 overflow-hidden">
                <NavItem href="/chat" icon=icondata_lu::LuMessageSquarePlus label="New Chat" expanded=expanded/>
                <NavItem href="/chats" icon=icondata_lu::LuMessagesSquare label="Chats" expanded=expanded/>
                <NavItem href="/dashboards" icon=icondata_lu::LuChartBar label="Dashboards" expanded=expanded/>
                <NavItem href="/watches" icon=icondata_lu::LuEye label="Watches" expanded=expanded/>
                <NavItem href="/knowledge" icon=icondata_lu::LuBookOpen label="Knowledge" expanded=expanded/>
                <NavItem href="/sql-editor" icon=icondata_lu::LuDatabase label="SQL Editor" expanded=expanded/>
            </nav>

            // User section at bottom
            <div class="border-t border-border px-2 py-3 shrink-0">
                <NavItem href="/settings/profile" icon=icondata_lu::LuSettings label="Settings" expanded=expanded active=true/>
            </div>
        </aside>
    }
}

/// A single navigation item in the sidebar.
///
/// Matches React sidebar button classes:
/// - `w-full h-10 flex items-center rounded-lg hover:bg-accent transition-colors`
/// - Active: `bg-accent`
/// - Icon: `w-5 h-5 text-muted-foreground flex-shrink-0`
/// - Label: `text-sm font-medium text-foreground whitespace-nowrap transition-opacity duration-300`
#[component]
fn NavItem(
    href: &'static str,
    icon: &'static icondata_core::IconData,
    label: &'static str,
    expanded: ReadSignal<bool>,
    #[prop(default = false)]
    active: bool,
) -> impl IntoView {
    let active_class = if active { " bg-accent" } else { "" };
    let base = format!(
        "w-full h-10 flex items-center gap-3 pl-2.5 pr-3 rounded-lg hover:bg-accent transition-colors{active_class}"
    );

    view! {
        <a href=href class=base title=label>
            <span class="w-5 h-5 text-muted-foreground flex-shrink-0 flex items-center justify-center">
                <Icon icon=icon width="20" height="20"/>
            </span>
            <span
                class="text-sm font-medium text-foreground whitespace-nowrap overflow-hidden transition-opacity duration-300"
                style=move || if expanded.get() { "opacity: 1" } else { "opacity: 0; width: 0; overflow: hidden" }
            >
                {label}
            </span>
        </a>
    }
}
