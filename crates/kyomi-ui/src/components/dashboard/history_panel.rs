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
use phosphor_leptos::{Icon, IconWeight};

use crate::components::{
    Button, ButtonSize, ButtonVariant, ConfirmDialog, EmptyState, RightPanel, Spinner, Tooltip,
};
use crate::server_fns::dashboards::{
    diff_versions, get_version, list_versions, restore_version,
    DiffLine, VersionDiff, VersionDetail,
};

// ─── Constants ──────────────────────────────────────────────────────────────

const MIN_WIDTH: f64 = 320.0;
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
    // Filter to only show changed lines with up to 3 lines of surrounding context
    let filtered = context_filter(&diff_lines, 3);

    view! {
        <div class="font-mono text-xs bg-card border border-border rounded-lg overflow-x-auto">
            <pre class="p-4">
                {filtered.into_iter().map(|line| {
                    let (class, prefix) = match line.line_type.as_str() {
                        "add" => ("text-success-foreground bg-success/10 px-2 -mx-2", "+"),
                        "delete" => ("text-error-foreground bg-error/10 px-2 -mx-2", "-"),
                        "separator" => ("text-muted-foreground px-2 -mx-2 my-1 border-t border-border", ""),
                        _ => ("text-muted-foreground px-2 -mx-2", " "),
                    };
                    let text = if line.line_type == "separator" {
                        "···".to_string()
                    } else if line.content.is_empty() {
                        format!("{prefix} ")
                    } else {
                        format!("{prefix} {}", line.content)
                    };
                    view! {
                        <div class=class>{text}</div>
                    }
                }).collect_view()}
            </pre>
        </div>
    }
}

/// Filter diff lines to show only changes with surrounding context lines.
/// Inserts separator markers ("···") between non-contiguous hunks.
fn context_filter(lines: &[DiffLine], context: usize) -> Vec<DiffLine> {
    if lines.is_empty() {
        return Vec::new();
    }

    // Mark which lines are within `context` distance of a change
    let mut show = vec![false; lines.len()];
    for (i, line) in lines.iter().enumerate() {
        if line.line_type == "add" || line.line_type == "delete" {
            let start = i.saturating_sub(context);
            let end = (i + context + 1).min(lines.len());
            for flag in &mut show[start..end] {
                *flag = true;
            }
        }
    }

    let mut result = Vec::new();
    let mut last_shown = false;

    for (i, line) in lines.iter().enumerate() {
        if show[i] {
            if !last_shown && !result.is_empty() {
                // Insert separator between non-contiguous hunks
                result.push(DiffLine {
                    line_type: "separator".to_string(),
                    content: String::new(),
                });
            }
            result.push(line.clone());
            last_shown = true;
        } else {
            last_shown = false;
        }
    }

    result
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

    // ── Panel width (desktop resize) ────────────────────────────────────
    let panel_width = RwSignal::new(DEFAULT_WIDTH);

    // ── Version list state ──────────────────────────────────────────────
    let (version_counter, set_version_counter) = signal(0u32);

    // Resource that fetches versions when panel opens or counter bumps.
    // Only re-fetch when open transitions to true — don't clear data on close
    // so the content stays visible during the slide-out animation.
    let (fetch_trigger, set_fetch_trigger) = signal(0u32);
    Effect::new(move |_| {
        if open.get() {
            set_fetch_trigger.update(|n| *n += 1);
        }
    });

    let versions_resource = Resource::new(
        move || (fetch_trigger.get(), version_counter.get()),
        move |(_trigger, _counter)| {
            let did = dashboard_id.get_value();
            async move {
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
        if !open.get() && let Some(cb) = on_preview {
            cb.run(None);
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
                    set_previewing.try_set(Some(detail));
                    set_show_diff.try_set(false);
                }
                Err(e) => {
                    set_error.try_set(Some(format!("Failed to load version: {e}")));
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
                    set_diff_data.try_set(Some(diff));
                    set_show_diff.try_set(true);
                    set_previewing.try_set(None);
                    if let Some(cb) = on_preview { cb.run(None); }
                }
                Err(e) => {
                    set_error.try_set(Some(format!("Failed to load diff: {e}")));
                }
            }
            set_is_diff_loading.try_set(false);
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
                    set_previewing.try_set(None);
                    set_show_diff.try_set(false);
                    if let Some(cb) = on_preview { cb.run(None); }
                    on_restore.run(());
                    // Refetch versions
                    set_version_counter.try_update(|c| *c += 1);
                }
                Err(e) => {
                    set_error.try_set(Some(format!("Failed to restore version: {e}")));
                }
            }
            set_is_restoring.try_set(false);
        });
    };

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

    // ── Panel body builder ──────────────────────────────────────────────
    // Header + close button + resize + mobile overlay are all handled by
    // <RightPanel>. This closure returns only the inner content.
    let panel_body = move || {
        let current_error = error.get();

        view! {
            <div class="h-full">
                // Error banner + version list / diff view
                    // Error banner (shown above content when there is an error)
                    {current_error.clone().map(|err| view! {
                        <div class="p-4 text-center">
                            <p class="text-error-foreground mb-2">{err}</p>
                            <Button
                                variant=ButtonVariant::Outline
                                size=ButtonSize::Sm
                                on:click=move |_| {
                                    set_error.set(None);
                                    set_version_counter.update(|c| *c += 1);
                                }
                            >
                                "Retry"
                            </Button>
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
                                                <Button
                                                    variant=ButtonVariant::Link
                                                    size=ButtonSize::Sm
                                                    on:click=move |_| set_show_diff.set(false)
                                                >
                                                    "Back to list"
                                                </Button>
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
                                                        <Button
                                                            variant=ButtonVariant::Outline
                                                            size=ButtonSize::Sm
                                                            on:click=move |_| set_version_counter.update(|c| *c += 1)
                                                        >
                                                            "Retry"
                                                        </Button>
                                                    </div>
                                                }.into_any(),
                                                Ok(result) if result.versions.is_empty() => view! {
                                                    <EmptyState
                                                        icon=std::sync::Arc::new(|| view! { <Icon icon=phosphor_leptos::CLOCK weight=IconWeight::Duotone size="64px" /> }.into_any())
                                                        title="No version history yet"
                                                        description="Versions are created when you save changes"
                                                        class="p-4 border-0"
                                                    />
                                                }.into_any(),
                                                Ok(result) => {
                                                    // current_version = live dashboard content (version_number = max + 1)
                                                    // versions = historical snapshots, newest first
                                                    let current_version = Some(result.current_version);
                                                    let historical_versions = result.versions;

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
                                                                        <Button
                                                                            variant=ButtonVariant::Outline
                                                                            size=ButtonSize::Sm
                                                                            class="text-warning-foreground border-warning-border hover:bg-warning"
                                                                            on:click=move |_| handle_exit_preview()
                                                                        >
                                                                            "Exit Preview"
                                                                        </Button>
                                                                    </div>
                                                                </div>
                                                            })}

                                                            // Current (latest) version
                                                            {current_version.clone().map(|cv| {
                                                                let cv_ver = cv.version_number;
                                                                let cv_created_at = cv.created_at.clone();
                                                                let cv_change_summary = cv.change_summary.clone();
                                                                let cv_content = cv.content.clone();
                                                                let cv_title = cv.title.clone();
                                                                let first_historical = historical_versions.first().map(|h| h.version_number);

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
                                                                        on:click={
                                                                            let cv_content = cv_content.clone();
                                                                            let cv_title = cv_title.clone();
                                                                            let cv_created_at = cv_created_at.clone();
                                                                            move |_| {
                                                                                let is_current_previewing = previewing.get_untracked()
                                                                                    .map(|pv| pv.version_number == cv_ver)
                                                                                    .unwrap_or(false);
                                                                                if is_current_previewing {
                                                                                    handle_exit_preview();
                                                                                } else {
                                                                                    // Current version: use content directly (no server fetch needed)
                                                                                    let detail = VersionDetail {
                                                                                        version_number: cv_ver,
                                                                                        title: cv_title.clone(),
                                                                                        content: cv_content.clone(),
                                                                                        change_summary: None,
                                                                                        byte_size: None,
                                                                                        created_at: cv_created_at.clone(),
                                                                                        created_by_name: None,
                                                                                    };
                                                                                    if let Some(cb) = on_preview { cb.run(Some(detail.content.clone())); }
                                                                                    set_previewing.set(Some(detail));
                                                                                    set_show_diff.set(false);
                                                                                }
                                                                            }
                                                                        }
                                                                    >
                                                                        <div class="flex items-start justify-between">
                                                                            <div class="flex-1 min-w-0">
                                                                                <div class="flex items-center gap-2">
                                                                                    <span class="text-sm font-medium text-foreground">
                                                                                        {format!("Version {cv_ver}")}
                                                                                    </span>
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
                                                                            let handle_view_diff_clone = handle_view_diff;
                                                                            view! {
                                                                                <div
                                                                                    class="flex items-center gap-2 mt-2"
                                                                                    on:click=move |ev| ev.stop_propagation()
                                                                                >
                                                                                    <Tooltip content="Compare with previous">
                                                                                        <Button
                                                                                            variant=ButtonVariant::GhostMuted
                                                                                            size=ButtonSize::IconSm
                                                                                            aria_label="Compare with previous"
                                                                                            disabled=is_diff_loading
                                                                                            on:click=move |_| handle_view_diff_clone(prev_ver, cv_ver)
                                                                                        >
                                                                                            <Icon icon=phosphor_leptos::FILE_ARROW_DOWN size="16px" />
                                                                                        </Button>
                                                                                    </Tooltip>
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
                                                                let handle_preview_version_clone = handle_preview_version;
                                                                let handle_view_diff_clone = handle_view_diff;
                                                                let handle_view_diff_clone2 = handle_view_diff;

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
                                                                                <Tooltip content="Compare with previous">
                                                                                    <Button
                                                                                        variant=ButtonVariant::GhostMuted
                                                                                        size=ButtonSize::IconSm
                                                                                        aria_label="Compare with previous"
                                                                                        disabled=is_diff_loading
                                                                                        on:click=move |_| handle_view_diff_clone(pv, ver_num)
                                                                                    >
                                                                                        <Icon icon=phosphor_leptos::FILE_ARROW_DOWN size="16px" />
                                                                                    </Button>
                                                                                </Tooltip>
                                                                            })}

                                                                            // Compare with current version
                                                                            {current_ver_num.map(|cv| view! {
                                                                                <Tooltip content="Compare with current">
                                                                                    <Button
                                                                                        variant=ButtonVariant::GhostMuted
                                                                                        size=ButtonSize::IconSm
                                                                                        aria_label="Compare with current"
                                                                                        disabled=is_diff_loading
                                                                                        on:click=move |_| handle_view_diff_clone2(ver_num, cv)
                                                                                    >
                                                                                        <Icon icon=phosphor_leptos::FILE_ARROW_UP size="16px" />
                                                                                    </Button>
                                                                                </Tooltip>
                                                                            })}

                                                                            // Restore button
                                                                            <Tooltip content="Restore this version">
                                                                                <Button
                                                                                    variant=ButtonVariant::GhostMuted
                                                                                    size=ButtonSize::IconSm
                                                                                    aria_label="Restore this version"
                                                                                    disabled=is_restoring
                                                                                    on:click=move |_| set_confirm_version.set(Some(ver_num))
                                                                                >
                                                                                    {move || if is_restoring.get() && confirm_version.get_untracked().is_none() {
                                                                                        view! { <Spinner class="text-primary" /> }.into_any()
                                                                                    } else {
                                                                                        view! { <Icon icon=phosphor_leptos::ARROW_U_UP_LEFT size="16px" /> }.into_any()
                                                                                    }}
                                                                                </Button>
                                                                            </Tooltip>
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
        }
    };

    // ── Render ───────────────────────────────────────────────────────────

    view! {
        <RightPanel
            open=open
            on_close=on_close
            width=panel_width
            min_width=MIN_WIDTH
            max_width=MAX_WIDTH
            title="Version History".to_string()
            close_label="Close history".to_string()
        >
            {panel_body()}
        </RightPanel>
        // Wrapped in a reactive closure so the confirm-dialog message re-renders
        // when `confirm_version` changes; without this, `confirm_message()` is
        // called once at init and the dialog shows an empty string.
        {move || {
            let message = confirm_message();
            view! {
                <ConfirmDialog
                    open=confirm_open
                    title="Restore Version?"
                    message=message
                    confirm_text="Restore"
                    destructive=false
                    on_confirm=on_confirm
                    on_cancel=on_cancel
                />
            }
        }}
    }
}
