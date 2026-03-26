// SPDX-License-Identifier: AGPL-3.0-or-later

//! Version history slide-out panel for dashboards.
//!
//! Shows a list of saved versions with the ability to preview, compare diffs,
//! and restore any previous version. Matches every feature in the React
//! `DashboardHistoryPanel.jsx` component.
//!
//! - Desktop: resizable inline sidebar (320-600px) on the right, with drag handle
//! - Mobile: slide-in panel with backdrop overlay

use leptos::prelude::*;
#[cfg(feature = "hydrate")]
use wasm_bindgen::prelude::*;

use crate::components::{ConfirmDialog, Spinner};
use crate::server_fns::dashboards::{
    diff_versions, get_version, list_versions, restore_version, DiffLine, VersionDiff,
    VersionDetail, VersionSummary,
};

use super::shared::use_is_mobile;

// ─── Constants ──────────────────────────────────────────────────────────────

#[cfg(feature = "hydrate")]
const MIN_WIDTH: f64 = 320.0;
#[cfg(feature = "hydrate")]
const MAX_WIDTH: f64 = 600.0;
const DEFAULT_WIDTH: f64 = 384.0;

// ─── Relative time formatting ───────────────────────────────────────────────

/// Matches the React `formatDate` function: "Just now", "2m ago", "3h ago",
/// "5d ago", then falls back to short date format.
fn format_relative_time(iso: &str) -> String {
    let Ok(dt) = chrono::DateTime::parse_from_rfc3339(iso) else {
        return iso.to_string();
    };
    let now = chrono::Utc::now();
    let diff = now.signed_duration_since(dt);

    let mins = diff.num_minutes();
    if mins < 1 {
        return "Just now".to_string();
    }
    if mins < 60 {
        return format!("{mins}m ago");
    }
    let hours = diff.num_hours();
    if hours < 24 {
        return format!("{hours}h ago");
    }
    let days = diff.num_days();
    if days < 7 {
        return format!("{days}d ago");
    }

    // Fall back to short date — omit year if same as current year
    let current_year = now.format("%Y").to_string();
    let dt_year = dt.format("%Y").to_string();
    if dt_year == current_year {
        dt.format("%b %-d").to_string()
    } else {
        dt.format("%b %-d, %Y").to_string()
    }
}

/// Matches the React `formatTime` function: "3:45 PM" style.
fn format_time(iso: &str) -> String {
    let Ok(dt) = chrono::DateTime::parse_from_rfc3339(iso) else {
        return String::new();
    };
    dt.format("%-I:%M %p").to_string()
}

// ─── DiffViewer sub-component ───────────────────────────────────────────────

/// Line-by-line diff visualization.
///
/// Renders each `DiffLine` with appropriate coloring:
/// - "add" lines: green background with "+" prefix
/// - "delete" lines: red background with "-" prefix
/// - "context" lines: neutral
#[component]
fn DiffViewer(diff_lines: Vec<DiffLine>) -> impl IntoView {
    view! {
        <div class="font-mono text-xs bg-foreground rounded-lg overflow-x-auto">
            <pre class="p-4">
                {diff_lines.into_iter().map(|line| {
                    let (class, prefix) = match line.line_type.as_str() {
                        "add" => ("text-green-400 bg-green-900/30 px-2 -mx-2", "+"),
                        "delete" => ("text-red-400 bg-red-900/30 px-2 -mx-2", "-"),
                        _ => ("text-background/80 px-2 -mx-2", " "),
                    };
                    let text = if line.content.is_empty() {
                        format!("{prefix} ")
                    } else {
                        format!("{prefix}{}", line.content)
                    };
                    view! {
                        <div class=class>{text}</div>
                    }
                }).collect_view()}
            </pre>
        </div>
    }
}

// ─── SVG Icons ──────────────────────────────────────────────────────────────

/// Clock icon (Heroicons outline) — matches `ClockIcon` in React.
#[component]
fn ClockIcon(#[prop(into, optional)] class: String) -> impl IntoView {
    view! {
        <svg class=class xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 24 24" stroke-width="1.5" stroke="currentColor">
            <path stroke-linecap="round" stroke-linejoin="round" d="M12 6v6h4.5m4.5 0a9 9 0 1 1-18 0 9 9 0 0 1 18 0Z" />
        </svg>
    }
}

/// X icon (Heroicons outline) — matches `XMarkIcon` in React.
#[component]
fn XIcon(#[prop(into, optional)] class: String) -> impl IntoView {
    view! {
        <svg class=class xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 24 24" stroke-width="1.5" stroke="currentColor">
            <path stroke-linecap="round" stroke-linejoin="round" d="M6 18 18 6M6 6l12 12" />
        </svg>
    }
}

/// ArrowUturnLeft icon (Heroicons outline) — matches `ArrowUturnLeftIcon` in React.
#[component]
fn RestoreIcon(#[prop(into, optional)] class: String) -> impl IntoView {
    view! {
        <svg class=class xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 24 24" stroke-width="1.5" stroke="currentColor">
            <path stroke-linecap="round" stroke-linejoin="round" d="M9 15 3 9m0 0 6-6M3 9h12a6 6 0 0 1 0 12h-3" />
        </svg>
    }
}

/// DocumentArrowDown icon (Heroicons outline) — "Compare with previous".
#[component]
fn CompareDownIcon(#[prop(into, optional)] class: String) -> impl IntoView {
    view! {
        <svg class=class xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 24 24" stroke-width="1.5" stroke="currentColor">
            <path stroke-linecap="round" stroke-linejoin="round" d="M19.5 14.25v-2.625a3.375 3.375 0 0 0-3.375-3.375h-1.5A1.125 1.125 0 0 1 13.5 7.125v-1.5a3.375 3.375 0 0 0-3.375-3.375H8.25m.75 12 3 3m0 0 3-3m-3 3v-6m-1.5-9H5.625c-.621 0-1.125.504-1.125 1.125v17.25c0 .621.504 1.125 1.125 1.125h12.75c.621 0 1.125-.504 1.125-1.125V11.25a9 9 0 0 0-9-9Z" />
        </svg>
    }
}

/// DocumentArrowUp icon (Heroicons outline) — "Compare with current".
#[component]
fn CompareUpIcon(#[prop(into, optional)] class: String) -> impl IntoView {
    view! {
        <svg class=class xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 24 24" stroke-width="1.5" stroke="currentColor">
            <path stroke-linecap="round" stroke-linejoin="round" d="M19.5 14.25v-2.625a3.375 3.375 0 0 0-3.375-3.375h-1.5A1.125 1.125 0 0 1 13.5 7.125v-1.5a3.375 3.375 0 0 0-3.375-3.375H8.25m6.75 12-3-3m0 0-3 3m3-3v6m-1.5-15H5.625c-.621 0-1.125.504-1.125 1.125v17.25c0 .621.504 1.125 1.125 1.125h12.75c.621 0 1.125-.504 1.125-1.125V11.25a9 9 0 0 0-9-9Z" />
        </svg>
    }
}

// ─── Main component ─────────────────────────────────────────────────────────

/// Slide-out panel showing dashboard version history.
///
/// Matches every feature of the React `DashboardHistoryPanel`:
/// - Version list with current "Latest" badge
/// - Preview mode with warning banner
/// - Side-by-side diff view
/// - Restore with confirmation dialog
/// - Desktop: resizable sidebar (320-600px)
/// - Mobile: slide-in overlay with backdrop
#[component]
pub fn HistoryPanel(
    /// Dashboard ID to show history for.
    dashboard_id: String,
    /// Whether the panel is open.
    #[prop(into)]
    open: Signal<bool>,
    /// Callback to close the panel.
    on_close: Callback<()>,
    /// Callback when previewing a version (passes version content, or None to exit preview).
    #[prop(optional)]
    on_preview: Option<Callback<Option<String>>>,
    /// Callback after restoring a version.
    on_restore: Callback<()>,
) -> impl IntoView {
    let dashboard_id = StoredValue::new(dashboard_id);
    let is_mobile = use_is_mobile();

    // ── Panel width (desktop resize) ────────────────────────────────────
    let (panel_width, set_panel_width) = signal(DEFAULT_WIDTH);
    #[cfg(not(feature = "hydrate"))]
    let _ = set_panel_width;
    let (_is_resizing, set_is_resizing) = signal(false);

    // ── Version list state ──────────────────────────────────────────────
    let (version_counter, set_version_counter) = signal(0u32);

    // Resource that fetches versions when panel opens or counter bumps.
    let versions_resource = Resource::new(
        move || (open.get(), version_counter.get()),
        move |(is_open, _counter)| {
            let did = dashboard_id.get_value();
            async move {
                if !is_open {
                    return Ok(Vec::<VersionSummary>::new());
                }
                list_versions(did).await
            }
        },
    );

    // ── Previewing state ────────────────────────────────────────────────
    // Stores the version detail being previewed. None = not previewing.
    let (previewing, set_previewing) = signal(Option::<VersionDetail>::None);

    // ── Diff state ──────────────────────────────────────────────────────
    let (show_diff, set_show_diff) = signal(false);
    let (diff_data, set_diff_data) = signal(Option::<VersionDiff>::None);
    let (is_diff_loading, set_is_diff_loading) = signal(false);

    // ── Restore state ───────────────────────────────────────────────────
    let (is_restoring, set_is_restoring) = signal(false);
    let (confirm_version, set_confirm_version) = signal(Option::<i32>::None);

    // ── Error state ─────────────────────────────────────────────────────
    let (error, set_error) = signal(Option::<String>::None);

    // ── Reset state when panel opens ────────────────────────────────────
    Effect::new(move || {
        if open.get() {
            set_previewing.set(None);
            set_show_diff.set(false);
            set_diff_data.set(None);
            set_error.set(None);
        }
    });

    // ── Notify parent when panel closes (exit preview) ──────────────────
    Effect::new(move || {
        if !open.get() {
            if let Some(cb) = on_preview { cb.run(None); }
        }
    });

    // ── Handlers ────────────────────────────────────────────────────────

    let handle_preview_version = move |version_number: i32| {
        let did = dashboard_id.get_value();
        set_error.set(None);
        leptos::task::spawn_local(async move {
            match get_version(did, version_number).await {
                Ok(detail) => {
                    if let Some(cb) = on_preview { cb.run(Some(detail.content.clone())); }
                    set_previewing.set(Some(detail));
                    set_show_diff.set(false);
                }
                Err(e) => {
                    set_error.set(Some(format!("Failed to load version: {e}")));
                }
            }
        });
    };

    let handle_exit_preview = move || {
        set_previewing.set(None);
        if let Some(cb) = on_preview { cb.run(None); }
    };

    let handle_view_diff = move |from_version: i32, to_version: i32| {
        let did = dashboard_id.get_value();
        set_is_diff_loading.set(true);
        set_error.set(None);
        leptos::task::spawn_local(async move {
            match diff_versions(did, from_version, to_version).await {
                Ok(diff) => {
                    set_diff_data.set(Some(diff));
                    set_show_diff.set(true);
                    set_previewing.set(None);
                    if let Some(cb) = on_preview { cb.run(None); }
                }
                Err(e) => {
                    set_error.set(Some(format!("Failed to load diff: {e}")));
                }
            }
            set_is_diff_loading.set(false);
        });
    };

    let handle_restore = move |ver_num: i32| {
        let did = dashboard_id.get_value();
        set_is_restoring.set(true);
        set_confirm_version.set(None);
        set_error.set(None);
        leptos::task::spawn_local(async move {
            match restore_version(did, ver_num).await {
                Ok(()) => {
                    set_previewing.set(None);
                    set_show_diff.set(false);
                    if let Some(cb) = on_preview { cb.run(None); }
                    on_restore.run(());
                    // Refetch versions
                    set_version_counter.update(|c| *c += 1);
                }
                Err(e) => {
                    set_error.set(Some(format!("Failed to restore version: {e}")));
                }
            }
            set_is_restoring.set(false);
        });
    };

    // ── Resize drag handling (desktop) ──────────────────────────────────
    // Stores active drag cleanup so on_cleanup can remove listeners if the
    // component unmounts mid-drag.

    #[cfg(feature = "hydrate")]
    let drag_cleanup: StoredValue<Option<send_wrapper::SendWrapper<Box<dyn FnOnce()>>>> =
        StoredValue::new(None);

    let handle_resize_start = move |ev: web_sys::MouseEvent| {
        ev.prevent_default();
        set_is_resizing.set(true);

        #[cfg(feature = "hydrate")]
        {
            use std::cell::RefCell;
            use std::rc::Rc;
            use wasm_bindgen::closure::Closure;

            let start_x = ev.client_x() as f64;
            let start_w = panel_width.get_untracked();

            let Some(window) = web_sys::window() else {
                return;
            };
            let Some(document) = window.document() else {
                return;
            };

            let move_handler =
                Closure::<dyn FnMut(web_sys::MouseEvent)>::new(move |ev: web_sys::MouseEvent| {
                    let diff = start_x - ev.client_x() as f64;
                    let new_width = (start_w + diff).clamp(MIN_WIDTH, MAX_WIDTH);
                    set_panel_width.set(new_width);
                });

            let move_ref = move_handler
                .as_ref()
                .unchecked_ref::<js_sys::Function>()
                .clone();
            let document_for_up = document.clone();
            let move_fn_for_up = move_ref.clone();

            let closures: Rc<RefCell<Option<(
                Closure<dyn FnMut(web_sys::MouseEvent)>,
                Closure<dyn FnMut()>,
            )>>> = Rc::new(RefCell::new(None));
            let closures_for_up = closures.clone();

            let up_handler = Closure::<dyn FnMut()>::new(move || {
                set_is_resizing.set(false);
                let _ = document_for_up
                    .remove_event_listener_with_callback("mousemove", &move_fn_for_up);
                if let Some((_, ref up_cb)) = *closures_for_up.borrow() {
                    let _ = document_for_up.remove_event_listener_with_callback(
                        "mouseup",
                        up_cb.as_ref().unchecked_ref(),
                    );
                }
                if let Some(body) = document_for_up.body() {
                    let _ = body.style().set_property("cursor", "");
                    let _ = body.style().set_property("user-select", "");
                }
                closures_for_up.borrow_mut().take();
                drag_cleanup.set_value(None);
            });

            let _ = document
                .add_event_listener_with_callback("mousemove", move_ref.unchecked_ref());
            let _ = document
                .add_event_listener_with_callback("mouseup", up_handler.as_ref().unchecked_ref());

            *closures.borrow_mut() = Some((move_handler, up_handler));

            let closures_for_teardown = closures;
            let document_for_teardown = document.clone();
            let move_ref_for_teardown = move_ref.clone();
            let teardown: Box<dyn FnOnce()> = Box::new(move || {
                if let Some((_, ref up_cb)) = *closures_for_teardown.borrow() {
                    let _ = document_for_teardown
                        .remove_event_listener_with_callback("mousemove", &move_ref_for_teardown);
                    let _ = document_for_teardown.remove_event_listener_with_callback(
                        "mouseup",
                        up_cb.as_ref().unchecked_ref(),
                    );
                }
                closures_for_teardown.borrow_mut().take();
            });
            drag_cleanup.set_value(Some(send_wrapper::SendWrapper::new(teardown)));

            if let Some(body) = document.body() {
                let _ = body.style().set_property("cursor", "col-resize");
                let _ = body.style().set_property("user-select", "none");
            }
        }
    };

    #[cfg(feature = "hydrate")]
    on_cleanup(move || {
        if let Some(teardown) = drag_cleanup.try_update_value(|v| v.take()).flatten() {
            teardown.take()();
        }
    });

    // ── Confirm dialog signals ──────────────────────────────────────────
    let confirm_open: Signal<bool> = Signal::derive(move || confirm_version.get().is_some());
    let confirm_message = move || {
        confirm_version.get().map_or_else(String::new, |v| {
            format!("Restore to version {v}? This will create a new version with that content.")
        })
    };
    let on_confirm = Callback::new(move |()| {
        if let Some(ver_num) = confirm_version.get_untracked() {
            handle_restore(ver_num);
        }
    });
    let on_cancel = Callback::new(move |()| {
        set_confirm_version.set(None);
    });

    // ── Panel content builder ───────────────────────────────────────────
    // Both mobile and desktop layouts share this inner content.
    let panel_content = move || {
        let current_error = error.get();

        view! {
            <div class="flex flex-col flex-1 min-w-0 h-full">
                // Header
                // React: `flex items-center justify-between px-4 py-3 border-b border-border bg-muted flex-shrink-0`
                <div class="flex items-center justify-between px-4 py-3 border-b border-border bg-muted flex-shrink-0">
                    <div class="flex items-center gap-2">
                        <ClockIcon class="w-5 h-5 text-primary" />
                        <span class="font-medium text-foreground">"Version History"</span>
                    </div>
                    <button
                        class="p-1 text-muted-foreground hover:text-foreground rounded-md hover:bg-accent"
                        aria-label="Close history"
                        on:click=move |_| on_close.run(())
                    >
                        <XIcon class="w-5 h-5" />
                    </button>
                </div>

                // Content area
                <div class="flex-1 overflow-y-auto">
                    // Error banner (shown above content when there is an error)
                    {current_error.clone().map(|err| view! {
                        <div class="p-4 text-center">
                            <p class="text-error-foreground mb-2">{err}</p>
                            <button
                                class="inline-flex items-center justify-center rounded-md text-sm font-medium border border-input bg-background text-foreground shadow-sm hover:bg-accent hover:text-accent-foreground h-8 px-3"
                                on:click=move |_| {
                                    set_error.set(None);
                                    set_version_counter.update(|c| *c += 1);
                                }
                            >
                                "Retry"
                            </button>
                        </div>
                    })}

                    // Main content (only shown when no error)
                    <Show when=move || error.get().is_none()>
                        {move || {
                            let diff_showing = show_diff.get();
                            let diff = diff_data.get();

                            if diff_showing {
                                // ── Diff View ───────────────────────────────
                                if let Some(ref diff) = diff {
                                    view! {
                                        <div class="p-4">
                                            <div class="flex items-center justify-between mb-4">
                                                <h3 class="text-sm font-medium text-foreground">
                                                    {format!("Changes: v{} → v{}", diff.from_version, diff.to_version)}
                                                </h3>
                                                <button
                                                    class="text-sm text-primary hover:text-primary/80"
                                                    on:click=move |_| set_show_diff.set(false)
                                                >
                                                    "Back to list"
                                                </button>
                                            </div>
                                            <div class="bg-muted rounded-lg p-3 mb-4">
                                                <div class="flex items-center gap-4 text-sm">
                                                    <span class="text-success-foreground">
                                                        {format!("+{} additions", diff.additions)}
                                                    </span>
                                                    <span class="text-error-foreground">
                                                        {format!("-{} deletions", diff.deletions)}
                                                    </span>
                                                </div>
                                            </div>
                                            <DiffViewer diff_lines=diff.diff_lines.clone() />
                                        </div>
                                    }.into_any()
                                } else {
                                    view! { <div /> }.into_any()
                                }
                            } else {
                                // ── Version List ────────────────────────────
                                view! {
                                    <Suspense fallback=move || view! {
                                        <div class="flex items-center justify-center py-12">
                                            <Spinner class="text-muted-foreground" />
                                        </div>
                                    }>
                                        {move || {
                                            versions_resource.get().map(|result| match result {
                                                Err(e) => view! {
                                                    <div class="p-4 text-center">
                                                        <p class="text-error-foreground mb-2">{e.to_string()}</p>
                                                        <button
                                                            class="inline-flex items-center justify-center rounded-md text-sm font-medium border border-input bg-background text-foreground shadow-sm hover:bg-accent hover:text-accent-foreground h-8 px-3"
                                                            on:click=move |_| set_version_counter.update(|c| *c += 1)
                                                        >
                                                            "Retry"
                                                        </button>
                                                    </div>
                                                }.into_any(),
                                                Ok(items) if items.is_empty() => view! {
                                                    <div class="p-6 text-center">
                                                        <ClockIcon class="w-12 h-12 mx-auto text-muted-foreground/50 mb-3" />
                                                        <p class="text-muted-foreground text-sm">"No version history yet"</p>
                                                        <p class="text-muted-foreground text-xs mt-1">
                                                            "Versions are created when you save changes"
                                                        </p>
                                                    </div>
                                                }.into_any(),
                                                Ok(items) => {
                                                    // First item is current (most recent), rest are historical.
                                                    // The server returns versions newest first.
                                                    let current_version = items.first().cloned();
                                                    let historical_versions: Vec<VersionSummary> = if items.len() > 1 {
                                                        items[1..].to_vec()
                                                    } else {
                                                        Vec::new()
                                                    };

                                                    view! {
                                                        <div class="divide-y divide-border/50">
                                                            // Preview banner
                                                            {move || previewing.get().map(|pv| view! {
                                                                <div class="px-4 py-3 bg-warning border-b border-warning-border">
                                                                    <div class="flex items-center justify-between">
                                                                        <div>
                                                                            <p class="text-sm font-medium text-warning-foreground">
                                                                                {format!("Previewing Version {}", pv.version_number)}
                                                                            </p>
                                                                            <p class="text-xs text-warning-foreground mt-0.5">
                                                                                "Click \"Exit Preview\" to return to current version"
                                                                            </p>
                                                                        </div>
                                                                        <button
                                                                            class="inline-flex items-center justify-center gap-2 whitespace-nowrap rounded-md text-sm font-medium transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring h-8 px-3 text-warning-foreground border border-warning-border hover:bg-warning"
                                                                            on:click=move |_| handle_exit_preview()
                                                                        >
                                                                            "Exit Preview"
                                                                        </button>
                                                                    </div>
                                                                </div>
                                                            })}

                                                            // Current (latest) version
                                                            {current_version.clone().map(|cv| {
                                                                let cv_ver = cv.version_number;
                                                                let cv_created_at = cv.created_at.clone();
                                                                let cv_change_summary = cv.change_summary.clone();
                                                                let first_historical = historical_versions.first().map(|h| h.version_number);
                                                                let handle_preview_version_clone = handle_preview_version.clone();

                                                                view! {
                                                                    <div
                                                                        class=move || {
                                                                            let is_current_previewing = previewing.get()
                                                                                .map(|pv| pv.version_number == cv_ver)
                                                                                .unwrap_or(false);
                                                                            if is_current_previewing {
                                                                                "px-4 py-3 transition-colors cursor-pointer bg-warning"
                                                                            } else {
                                                                                "px-4 py-3 transition-colors cursor-pointer hover:bg-accent"
                                                                            }
                                                                        }
                                                                        on:click=move |_| {
                                                                            let is_current_previewing = previewing.get_untracked()
                                                                                .map(|pv| pv.version_number == cv_ver)
                                                                                .unwrap_or(false);
                                                                            if is_current_previewing {
                                                                                handle_exit_preview();
                                                                            } else {
                                                                                handle_preview_version_clone(cv_ver);
                                                                            }
                                                                        }
                                                                    >
                                                                        <div class="flex items-start justify-between">
                                                                            <div class="flex-1 min-w-0">
                                                                                <div class="flex items-center gap-2">
                                                                                    <span class="text-sm font-medium text-foreground">
                                                                                        {format!("Version {cv_ver}")}
                                                                                    </span>
                                                                                    // React: `px-1.5 py-0.5 bg-primary/10 text-primary text-xs rounded font-medium`
                                                                                    <span class="px-1.5 py-0.5 bg-primary/10 text-primary text-xs rounded font-medium">
                                                                                        "Latest"
                                                                                    </span>
                                                                                    {move || previewing.get()
                                                                                        .filter(|pv| pv.version_number == cv_ver)
                                                                                        .map(|_| view! {
                                                                                            <span class="px-1.5 py-0.5 bg-warning text-warning-foreground text-xs rounded">
                                                                                                "Previewing"
                                                                                            </span>
                                                                                        })
                                                                                    }
                                                                                </div>
                                                                                <p class="text-xs text-muted-foreground mt-0.5">
                                                                                    {format!("{} at {}", format_relative_time(&cv_created_at), format_time(&cv_created_at))}
                                                                                </p>
                                                                                <p class="text-xs text-muted-foreground mt-1">
                                                                                    {cv_change_summary.unwrap_or_default()}
                                                                                </p>
                                                                            </div>
                                                                        </div>

                                                                        // Actions for current version — only diff with previous if there are historical versions
                                                                        {first_historical.map(|prev_ver| {
                                                                            let handle_view_diff_clone = handle_view_diff.clone();
                                                                            view! {
                                                                                <div
                                                                                    class="flex items-center gap-2 mt-2"
                                                                                    on:click=move |ev| ev.stop_propagation()
                                                                                >
                                                                                    <button
                                                                                        class="p-1.5 text-muted-foreground hover:text-foreground hover:bg-accent rounded transition-colors"
                                                                                        title="Compare with previous"
                                                                                        disabled=move || is_diff_loading.get()
                                                                                        on:click=move |_| handle_view_diff_clone(prev_ver, cv_ver)
                                                                                    >
                                                                                        <CompareDownIcon class="w-4 h-4" />
                                                                                    </button>
                                                                                </div>
                                                                            }
                                                                        })}
                                                                    </div>
                                                                }
                                                            })}

                                                            // Historical versions
                                                            {historical_versions.iter().enumerate().map(|(index, version)| {
                                                                let ver_num = version.version_number;
                                                                let _ver_title = version.title.clone();
                                                                let ver_created_at = version.created_at.clone();
                                                                let ver_change_summary = version.change_summary.clone();
                                                                let ver_created_by = version.created_by_name.clone();
                                                                let prev_ver_num = historical_versions.get(index + 1).map(|v| v.version_number);
                                                                let current_ver_num = current_version.as_ref().map(|cv| cv.version_number);
                                                                let handle_preview_version_clone = handle_preview_version.clone();
                                                                let handle_view_diff_clone = handle_view_diff.clone();
                                                                let handle_view_diff_clone2 = handle_view_diff.clone();

                                                                view! {
                                                                    <div
                                                                        class=move || {
                                                                            let is_this_previewing = previewing.get()
                                                                                .map(|pv| pv.version_number == ver_num)
                                                                                .unwrap_or(false);
                                                                            if is_this_previewing {
                                                                                "px-4 py-3 transition-colors cursor-pointer bg-warning"
                                                                            } else {
                                                                                "px-4 py-3 transition-colors cursor-pointer hover:bg-accent"
                                                                            }
                                                                        }
                                                                        on:click=move |_| {
                                                                            let is_this_previewing = previewing.get_untracked()
                                                                                .map(|pv| pv.version_number == ver_num)
                                                                                .unwrap_or(false);
                                                                            if is_this_previewing {
                                                                                handle_exit_preview();
                                                                            } else {
                                                                                handle_preview_version_clone(ver_num);
                                                                            }
                                                                        }
                                                                    >
                                                                        <div class="flex items-start justify-between">
                                                                            <div class="flex-1 min-w-0">
                                                                                <div class="flex items-center gap-2">
                                                                                    <span class="text-sm font-medium text-foreground">
                                                                                        {format!("Version {ver_num}")}
                                                                                    </span>
                                                                                    {move || previewing.get()
                                                                                        .filter(|pv| pv.version_number == ver_num)
                                                                                        .map(|_| view! {
                                                                                            <span class="px-1.5 py-0.5 bg-warning text-warning-foreground text-xs rounded">
                                                                                                "Previewing"
                                                                                            </span>
                                                                                        })
                                                                                    }
                                                                                </div>
                                                                                <p class="text-xs text-muted-foreground mt-0.5">
                                                                                    {format!("{} at {}", format_relative_time(&ver_created_at), format_time(&ver_created_at))}
                                                                                </p>
                                                                                <p class="text-xs text-muted-foreground mt-1">
                                                                                    {ver_change_summary.unwrap_or_else(|| "No summary".to_string())}
                                                                                </p>
                                                                                {ver_created_by.map(|name| view! {
                                                                                    <p class="text-xs text-muted-foreground/70 mt-0.5">
                                                                                        {format!("by {name}")}
                                                                                    </p>
                                                                                })}
                                                                            </div>
                                                                        </div>

                                                                        // Actions
                                                                        <div
                                                                            class="flex items-center gap-2 mt-2"
                                                                            on:click=move |ev| ev.stop_propagation()
                                                                        >
                                                                            // Compare with previous historical version
                                                                            {prev_ver_num.map(|pv| view! {
                                                                                <button
                                                                                    class="p-1.5 text-muted-foreground hover:text-foreground hover:bg-accent rounded transition-colors"
                                                                                    title="Compare with previous"
                                                                                    disabled=move || is_diff_loading.get()
                                                                                    on:click=move |_| handle_view_diff_clone(pv, ver_num)
                                                                                >
                                                                                    <CompareDownIcon class="w-4 h-4" />
                                                                                </button>
                                                                            })}

                                                                            // Compare with current version
                                                                            {current_ver_num.map(|cv| view! {
                                                                                <button
                                                                                    class="p-1.5 text-muted-foreground hover:text-primary hover:bg-accent rounded transition-colors"
                                                                                    title="Compare with current"
                                                                                    disabled=move || is_diff_loading.get()
                                                                                    on:click=move |_| handle_view_diff_clone2(ver_num, cv)
                                                                                >
                                                                                    <CompareUpIcon class="w-4 h-4" />
                                                                                </button>
                                                                            })}

                                                                            // Restore button
                                                                            <button
                                                                                class="p-1.5 text-muted-foreground hover:text-primary hover:bg-primary/10 rounded transition-colors disabled:opacity-50 disabled:cursor-not-allowed"
                                                                                title="Restore this version"
                                                                                disabled=move || is_restoring.get()
                                                                                on:click=move |_| set_confirm_version.set(Some(ver_num))
                                                                            >
                                                                                {move || if is_restoring.get() && confirm_version.get_untracked().is_none() {
                                                                                    // Show spinner only on the version being restored
                                                                                    // (confirm_version is cleared when restore starts)
                                                                                    view! { <Spinner class="text-primary" /> }.into_any()
                                                                                } else {
                                                                                    view! { <RestoreIcon class="w-4 h-4" /> }.into_any()
                                                                                }}
                                                                            </button>
                                                                        </div>
                                                                    </div>
                                                                }
                                                            }).collect_view()}
                                                        </div>
                                                    }.into_any()
                                                },
                                            })
                                        }}
                                    </Suspense>
                                }.into_any()
                            }
                        }}
                    </Show>
                </div>
            </div>
        }
    };

    // ── Render ───────────────────────────────────────────────────────────

    view! {
        <Show when=move || open.get()>
            {move || {
                let confirm_msg = confirm_message();
                let confirm_open_sig: Signal<bool> = confirm_open.into();

                if is_mobile.get() {
                    // Mobile: Slide-in panel with backdrop
                    // React: `fixed top-32 left-0 right-0 bottom-0 bg-[var(--color-overlay)] z-40`
                    // React: `fixed top-32 right-0 bottom-0 w-80 max-w-[85vw] z-50 bg-card flex flex-col shadow-xl`
                    view! {
                        <div>
                            <div
                                class="fixed top-32 left-0 right-0 bottom-0 bg-[var(--color-overlay)] z-40"
                                on:click=move |_| on_close.run(())
                            />
                            <div class="fixed top-32 right-0 bottom-0 w-80 max-w-[85vw] z-50 bg-card flex flex-col shadow-xl">
                                {panel_content()}
                            </div>
                            <ConfirmDialog
                                open=confirm_open_sig
                                title="Restore Version?"
                                message=confirm_msg
                                confirm_text="Restore"
                                destructive=false
                                on_confirm=on_confirm
                                on_cancel=on_cancel
                            />
                        </div>
                    }.into_any()
                } else {
                    // Desktop: Resizable inline sidebar
                    // React: `border-l border-border bg-card flex h-full overflow-hidden`
                    let width_style = move || format!("width: {}px", panel_width.get());

                    view! {
                        <div>
                            <div
                                class="border-l border-border bg-card flex h-full overflow-hidden"
                                style=width_style
                            >
                                // Resize Handle
                                // React: `flex items-center justify-center cursor-col-resize select-none px-1 -mr-2 relative z-10`
                                <div
                                    class="flex items-center justify-center cursor-col-resize select-none px-1 -mr-2 relative z-10"
                                    on:mousedown=handle_resize_start.clone()
                                    aria-label="Drag to resize"
                                >
                                    // React: `w-1 h-12 bg-border hover:bg-muted-foreground/50 rounded transition-colors`
                                    <div class="w-1 h-12 bg-border hover:bg-muted-foreground/50 rounded transition-colors" />
                                </div>

                                {panel_content()}
                            </div>
                            <ConfirmDialog
                                open=confirm_open_sig
                                title="Restore Version?"
                                message=confirm_msg
                                confirm_text="Restore"
                                destructive=false
                                on_confirm=on_confirm
                                on_cancel=on_cancel
                            />
                        </div>
                    }.into_any()
                }
            }}
        </Show>
    }
}
