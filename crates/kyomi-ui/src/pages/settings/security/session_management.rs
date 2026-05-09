// SPDX-License-Identifier: AGPL-3.0-or-later

//! Session Management card — security settings section for active sessions.
//!
//! Replaces `apps/frontend/src/components/SessionManagement.jsx` (352 lines).
//!
//! Shows:
//! - List of active sessions with device info, IP, last active time
//! - "Current" badge on the current session
//! - "MCP" badge on OAuth client sessions
//! - Revoke button on each session (except current)
//! - "Log Out from All Devices" button when >1 session
//! - Device icons (Smartphone/Monitor/Plug based on device type)
//! - Security tip alert when sessions exist
//! - Uses Card, Button, Badge, ConfirmDialog

use leptos::prelude::*;
use phosphor_leptos::Icon;
use crate::components::{
    Alert, AlertDescription, AlertVariant, Badge, BadgeVariant, Button, ButtonVariant,
    Card, CardContent, CardDescription, CardHeader, CardTitle, ConfirmDialog, EmptyState,
};
use crate::server_fns::security::{
    get_sessions, logout_all_sessions, revoke_session, SessionEntry,
};

/// Parse a user agent string to extract browser, OS, and mobile status.
///
/// Matches the React `parseUserAgent()` logic exactly.
fn parse_user_agent(user_agent: &str) -> (&'static str, &'static str, bool) {
    let ua = user_agent.to_lowercase();

    // Detect browser
    let browser = if ua.contains("firefox") && !ua.contains("seamonkey") {
        "Firefox"
    } else if ua.contains("seamonkey") {
        "Seamonkey"
    } else if ua.contains("chrome") && !ua.contains("chromium") && !ua.contains("edg") {
        "Chrome"
    } else if ua.contains("chromium") {
        "Chromium"
    } else if ua.contains("safari") && !ua.contains("chrome") {
        "Safari"
    } else if ua.contains("edg") {
        "Edge"
    } else if ua.contains("opera") || ua.contains("opr") {
        "Opera"
    } else {
        "Unknown Browser"
    };

    // Detect OS
    let (os, is_mobile) = if ua.contains("android") {
        ("Android", true)
    } else if ua.contains("ipad") {
        ("iPad", true)
    } else if ua.contains("iphone") {
        ("iPhone", true)
    } else if ua.contains("mac os x") || ua.contains("macintosh") {
        ("macOS", false)
    } else if ua.contains("windows") {
        ("Windows", false)
    } else if ua.contains("linux") {
        ("Linux", false)
    } else {
        ("Unknown OS", false)
    };

    (browser, os, is_mobile)
}

/// Get the display name for a session.
///
/// OAuth sessions show the client name; browser sessions show "Browser on OS".
fn session_display_name(session: &SessionEntry) -> String {
    if let Some(ref name) = session.oauth_client_name {
        return name.clone();
    }
    let (browser, os, _) = match session.user_agent.as_deref() {
        Some(ua) => parse_user_agent(ua),
        None => ("Unknown Browser", "Unknown OS", false),
    };
    format!("{browser} on {os}")
}

/// Format an RFC 3339 date string for display.
///
/// Uses JS `Date.toLocaleDateString()` on the client (matches React exactly),
/// and falls back to a truncated ISO string on the server.
fn format_date(date_str: &str) -> String {
    #[cfg(target_arch = "wasm32")]
    {
        use js_sys::{Date, Intl, Object, Reflect};
        use wasm_bindgen::JsValue;

        let date = Date::new(&JsValue::from_str(date_str));
        if date.get_time().is_nan() {
            return "Unknown".to_string();
        }

        // Build options: { month: "short", day: "numeric", year: "numeric",
        //                   hour: "2-digit", minute: "2-digit" }
        let options = Object::new();
        let _ = Reflect::set(&options, &"month".into(), &"short".into());
        let _ = Reflect::set(&options, &"day".into(), &"numeric".into());
        let _ = Reflect::set(&options, &"year".into(), &"numeric".into());
        let _ = Reflect::set(&options, &"hour".into(), &"2-digit".into());
        let _ = Reflect::set(&options, &"minute".into(), &"2-digit".into());

        let locale = js_sys::Array::of1(&"en-US".into());
        let formatter = Intl::DateTimeFormat::new(&locale, &options);
        let format_fn = formatter.format();
        format_fn
            .call1(&wasm_bindgen::JsValue::NULL, &date)
            .ok()
            .and_then(|v| v.as_string())
            .unwrap_or_else(|| "Unknown".to_string())
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        // Server-side fallback: show "Mar 21, 2026, 02:30 PM"-ish from RFC 3339.
        // Fine for SSR since client will hydrate with the JS version.
        date_str
            .get(..16)
            .unwrap_or("Unknown")
            .replace('T', " ")
            .to_string()
    }
}

/// Session Management component.
///
/// Loads active sessions on mount. Users can refresh, revoke individual
/// sessions, or log out from all devices.
#[component]
pub fn SessionManagement() -> impl IntoView {
    // ── Server data ──────────────────────────────────────────────────────
    let sessions_resource = Resource::new(|| (), |_| get_sessions());

    // ── UI state ─────────────────────────────────────────────────────────
    let loading = RwSignal::new(true);
    let sessions = RwSignal::new(Vec::<SessionEntry>::new());
    let error = RwSignal::new(Option::<String>::None);
    let revoking_id = RwSignal::new(Option::<String>::None);

    // ── Confirm dialog state ─────────────────────────────────────────────
    let dialog_open = RwSignal::new(false);
    let dialog_title = RwSignal::new(String::new());
    let dialog_message = RwSignal::new(String::new());
    let dialog_confirm_text = RwSignal::new(String::new());
    // We store the pending action as a signal: None = no pending action,
    // Some("all") = logout all, Some(token_id) = revoke specific session.
    let pending_action = RwSignal::new(Option::<String>::None);

    // ── Sync resource into signals ───────────────────────────────────────
    Effect::new(move || {
        if let Some(result) = sessions_resource.get() {
            loading.set(false);
            match result {
                Ok(data) => {
                    error.set(None);
                    sessions.set(data);
                }
                Err(e) => {
                    error.set(Some(format!("Failed to load sessions: {e}")));
                }
            }
        }
    });

    // ── Refresh handler ──────────────────────────────────────────────────
    let handle_refresh = move |_| {
        loading.set(true);
        sessions_resource.refetch();
    };

    // ── Revoke session flow ──────────────────────────────────────────────
    let open_revoke_dialog = move |session: SessionEntry| {
        let display_name = session_display_name(&session);
        dialog_title.set("Disconnect Session?".to_string());
        dialog_message.set(format!(
            "Are you sure you want to disconnect \"{display_name}\"? That client will need to re-authenticate."
        ));
        dialog_confirm_text.set("Disconnect".to_string());
        pending_action.set(Some(session.token_id.clone()));
        dialog_open.set(true);
    };

    // ── Logout all flow ──────────────────────────────────────────────────
    let open_logout_all_dialog = move |_| {
        dialog_title.set("Log Out From All Devices?".to_string());
        dialog_message.set(
            "Are you sure you want to log out from all devices? You will need to log in again."
                .to_string(),
        );
        dialog_confirm_text.set("Log Out All".to_string());
        pending_action.set(Some("__all__".to_string()));
        dialog_open.set(true);
    };

    // ── Dialog confirm handler ───────────────────────────────────────────
    let on_confirm = Callback::new(move |()| {
        dialog_open.set(false);
        let action = pending_action.get_untracked();
        pending_action.set(None);

        let Some(action_id) = action else { return };

        if action_id == "__all__" {
            // Logout all sessions
            loading.set(true);
            leptos::task::spawn_local(async move {
                match logout_all_sessions().await {
                    Ok(_) => {
                        // Redirect to login page
                        if let Some(window) = web_sys::window() {
                            let _ = window.location().set_href("/login");
                        }
                    }
                    Err(e) => {
                        error.try_set(Some(format!("Failed to logout from all devices: {e}")));
                        loading.try_set(false);
                    }
                }
            });
        } else {
            // Revoke specific session
            let token_id = action_id.clone();
            revoking_id.set(Some(action_id));
            error.set(None);

            leptos::task::spawn_local(async move {
                match revoke_session(token_id).await {
                    Ok(_) => {
                        revoking_id.try_set(None);
                        // Reload sessions
                        loading.try_set(true);
                        match get_sessions().await {
                            Ok(data) => {
                                sessions.try_set(data);
                                loading.try_set(false);
                            }
                            Err(e) => {
                                error.try_set(Some(format!("Failed to reload sessions: {e}")));
                                loading.try_set(false);
                            }
                        }
                    }
                    Err(e) => {
                        revoking_id.try_set(None);
                        error.try_set(Some(format!("Failed to disconnect session: {e}")));
                    }
                }
            });
        }
    });

    let on_cancel = Callback::new(move |()| {
        dialog_open.set(false);
        pending_action.set(None);
    });

    view! {
        <Card>
            <CardHeader>
                <div class="flex items-center justify-between">
                    <div>
                        <CardTitle>"Active Sessions"</CardTitle>
                        <CardDescription>"Manage your active login sessions across different devices."</CardDescription>
                    </div>
                    <button
                        class="inline-flex items-center justify-center gap-2 whitespace-nowrap rounded-md text-sm font-medium transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:pointer-events-none disabled:opacity-50 border border-input bg-background text-foreground shadow-sm hover:bg-secondary hover:text-accent-foreground h-9 w-9"
                        on:click=handle_refresh
                        disabled=move || loading.get()
                        title="Refresh sessions"
                    >
                        <span class=move || if loading.get() { "animate-spin" } else { "" }>
                            <Icon icon=phosphor_leptos::ARROWS_CLOCKWISE size="16px"/>
                        </span>
                    </button>
                </div>
            </CardHeader>
            <CardContent>
                // Error alert
                <Show when=move || error.get().is_some()>
                    <div class="mb-6">
                        <Alert variant=AlertVariant::Error>
                            <AlertDescription>
                                {move || error.get().unwrap_or_default()}
                            </AlertDescription>
                        </Alert>
                    </div>
                </Show>

                <div class="mb-6">
                    // Loading state (no sessions loaded yet)
                    <Show
                        when=move || !(loading.get() && sessions.get().is_empty())
                        fallback=|| view! {
                            <div class="text-center py-8">
                                <span class="animate-spin h-8 w-8 border-2 border-primary border-t-transparent rounded-full inline-block"/>
                                <p class="text-muted-foreground mt-2">"Loading sessions..."</p>
                            </div>
                        }
                    >
                        // Empty state
                        <Show
                            when=move || !sessions.get().is_empty()
                            fallback=|| view! {
                                <EmptyState
                                    title="No active sessions found"
                                    description="Your active sessions will appear here"
                                    class="border-2 border-dashed bg-muted"
                                />
                            }
                        >
                            // Sessions table
                            <div class="overflow-x-auto">
                                <table class="min-w-full divide-y divide-border">
                                    <thead class="bg-muted">
                                        <tr>
                                            <th class="px-6 py-3 text-left text-xs font-medium text-muted-foreground uppercase tracking-wider">
                                                "Device"
                                            </th>
                                            <th class="px-6 py-3 text-left text-xs font-medium text-muted-foreground uppercase tracking-wider">
                                                "Location"
                                            </th>
                                            <th class="px-6 py-3 text-left text-xs font-medium text-muted-foreground uppercase tracking-wider">
                                                "Last Active"
                                            </th>
                                            <th class="px-6 py-3 text-left text-xs font-medium text-muted-foreground uppercase tracking-wider">
                                                "Created"
                                            </th>
                                            <th class="px-6 py-3 text-right text-xs font-medium text-muted-foreground uppercase tracking-wider">
                                                "Actions"
                                            </th>
                                        </tr>
                                    </thead>
                                    <tbody class="bg-background divide-y divide-border">
                                        <For
                                            each=move || sessions.get()
                                            key=|s| s.token_id.clone()
                                            let:session
                                        >
                                            <SessionRow
                                                session=session
                                                revoking_id=revoking_id
                                                on_revoke=Callback::new(open_revoke_dialog)
                                            />
                                        </For>
                                    </tbody>
                                </table>
                            </div>
                        </Show>
                    </Show>
                </div>

                // Logout all devices section (only when >1 session)
                <Show when=move || { sessions.get().len() > 1 }>
                    <div class="border-t border-border pt-6">
                        <div class="flex items-start justify-between mb-4">
                            <div>
                                <h3 class="text-lg font-semibold text-foreground mb-2">"Sign Out All Devices"</h3>
                                <p class="text-sm text-muted-foreground">
                                    "This will end all active sessions and require you to log in again on all devices."
                                </p>
                            </div>
                        </div>
                        <Button
                            variant=ButtonVariant::Destructive
                            on:click=open_logout_all_dialog
                            disabled=loading.get_untracked()
                        >
                            {move || if loading.get() { "Logging out..." } else { "Log Out from All Devices" }}
                        </Button>
                    </div>
                </Show>

                // Security tip
                <Show when=move || !sessions.get().is_empty()>
                    <div class="mt-6">
                        <Alert variant=AlertVariant::Info>
                            <AlertDescription>
                                <strong>"Security tip:"</strong>
                                " If you see any unfamiliar sessions, log out from all devices immediately and change your password."
                            </AlertDescription>
                        </Alert>
                    </div>
                </Show>
            </CardContent>
        </Card>

        // Confirm Dialog
        {move || {
            let title = dialog_title.get();
            let message = dialog_message.get();
            let confirm_text = dialog_confirm_text.get();
            view! {
                <ConfirmDialog
                    open=Signal::from(dialog_open)
                    title=title
                    message=message
                    confirm_text=confirm_text
                    on_confirm=on_confirm
                    on_cancel=on_cancel
                />
            }
        }}
    }
}

/// A single row in the sessions table.
#[component]
fn SessionRow(
    session: SessionEntry,
    revoking_id: RwSignal<Option<String>>,
    on_revoke: Callback<SessionEntry>,
) -> impl IntoView {
    let is_oauth = session.oauth_client_name.is_some();
    let is_current = session.is_current;
    let token_id = session.token_id.clone();
    let display_name = session_display_name(&session);

    // Determine device icon
    let is_mobile = session
        .user_agent
        .as_deref()
        .map(|ua| parse_user_agent(ua).2)
        .unwrap_or(false);

    // Location display
    let location = match (&session.ip_address, &session.country_code) {
        (Some(ip), Some(cc)) => format!("{ip} ({cc})"),
        (Some(ip), None) => ip.clone(),
        _ => "\u{2014}".to_string(), // em dash
    };

    // Date formatting
    let last_active = session
        .last_used
        .as_deref()
        .or(Some(session.created_at.as_str()))
        .map(format_date)
        .unwrap_or_else(|| "Unknown".to_string());
    let created = format_date(&session.created_at);

    // User agent for display under device name (browser sessions only)
    let user_agent_display = if !is_oauth {
        session
            .user_agent
            .clone()
            .unwrap_or_else(|| "No user agent".to_string())
    } else {
        String::new()
    };

    let session_for_revoke = session.clone();

    let is_revoking = {
        let tid = token_id.clone();
        Memo::new(move |_| revoking_id.get().as_deref() == Some(tid.as_str()))
    };

    view! {
        <tr>
            // Device column
            <td class="px-6 py-4 whitespace-nowrap">
                <div class="flex items-center">
                    <div class="flex-shrink-0">
                        {if is_oauth {
                            view! { <span class="text-muted-foreground"><Icon icon=phosphor_leptos::PLUG size="20px"/></span> }.into_any()
                        } else if is_mobile {
                            view! { <span class="text-muted-foreground"><Icon icon=phosphor_leptos::DEVICE_MOBILE size="20px"/></span> }.into_any()
                        } else {
                            view! { <span class="text-muted-foreground"><Icon icon=phosphor_leptos::MONITOR size="20px"/></span> }.into_any()
                        }}
                    </div>
                    <div class="ml-3">
                        <div class="flex items-center gap-2">
                            <div class="text-sm font-medium text-foreground">
                                {display_name}
                            </div>
                            {is_current.then(|| view! {
                                <Badge variant=BadgeVariant::Default>"Current"</Badge>
                            })}
                            {is_oauth.then(|| view! {
                                <Badge variant=BadgeVariant::Secondary>"MCP"</Badge>
                            })}
                        </div>
                        {(!is_oauth).then(|| view! {
                            <div class="text-xs text-muted-foreground truncate max-w-xs mt-2">
                                {user_agent_display.clone()}
                            </div>
                        })}
                    </div>
                </div>
            </td>

            // Location column
            <td class="px-6 py-4 whitespace-nowrap text-sm text-muted-foreground">
                {location}
            </td>

            // Last Active column
            <td class="px-6 py-4 whitespace-nowrap text-sm text-muted-foreground">
                {last_active}
            </td>

            // Created column
            <td class="px-6 py-4 whitespace-nowrap text-sm text-muted-foreground">
                {created}
            </td>

            // Actions column
            <td class="px-6 py-4 whitespace-nowrap text-right">
                {(!is_current).then(|| {
                    let session_clone = session_for_revoke.clone();
                    view! {
                        <button
                            class="inline-flex items-center justify-center gap-2 whitespace-nowrap rounded-md text-sm font-medium transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:pointer-events-none disabled:opacity-50 text-foreground hover:bg-secondary hover:text-accent-foreground h-8 rounded-md px-3 text-xs"
                            on:click=move |_| on_revoke.run(session_clone.clone())
                            disabled=move || is_revoking.get()
                            title="Disconnect this session"
                        >
                            <Show
                                when=move || !is_revoking.get()
                                fallback=|| view! {
                                    <span class="animate-spin h-4 w-4 border-2 border-current border-t-transparent rounded-full"/>
                                }
                            >
                                <span class="text-muted-foreground transition-colors hover:text-destructive"><Icon icon=phosphor_leptos::X size="16px"/></span>
                            </Show>
                        </button>
                    }
                })}
            </td>
        </tr>
    }
}
