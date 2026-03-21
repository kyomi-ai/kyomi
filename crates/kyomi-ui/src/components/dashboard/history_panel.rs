// SPDX-License-Identifier: AGPL-3.0-or-later

//! Version history slide-out panel for dashboards.
//!
//! Shows a list of saved versions with the ability to restore any
//! previous version. Slides in from the right with a backdrop overlay.
//! Matches the React `DashboardHistoryPanel.jsx` component.

use leptos::prelude::*;

use crate::components::Spinner;
use crate::server_fns::dashboards::{list_versions, restore_version, VersionSummary};

/// Slide-out panel showing dashboard version history.
///
/// Fetches versions when opened and provides restore functionality.
/// The parent is responsible for refetching the dashboard after a
/// restore via the `on_restore` callback.
#[component]
pub fn HistoryPanel(
    /// Dashboard ID to show history for.
    dashboard_id: String,
    /// Whether the panel is open.
    #[prop(into)]
    open: Signal<bool>,
    /// Callback to close the panel.
    on_close: Callback<()>,
    /// Callback when a version is restored (viewer should refetch).
    on_restore: Callback<()>,
) -> impl IntoView {
    let dashboard_id = StoredValue::new(dashboard_id);

    // Fetch versions whenever the panel opens.
    let versions = Resource::new(
        move || open.get(),
        move |is_open| {
            let did = dashboard_id.get_value();
            async move {
                if !is_open {
                    return Ok(Vec::new());
                }
                list_versions(did).await
            }
        },
    );

    // Track which version is currently being restored (to disable buttons).
    let (restoring, set_restoring) = signal(Option::<i32>::None);

    // Confirmation dialog state.
    let (confirm_version, set_confirm_version) = signal(Option::<i32>::None);

    // Restore handler — called after user confirms.
    let handle_restore = move |ver_num: i32| {
        let did = dashboard_id.get_value();
        set_restoring.set(Some(ver_num));
        set_confirm_version.set(None);
        leptos::task::spawn_local(async move {
            match restore_version(did, ver_num).await {
                Ok(()) => {
                    on_restore.run(());
                    // Refetch versions to reflect the new state.
                    versions.refetch();
                }
                Err(e) => {
                    // Log the error — the UI will show the stale list which is fine.
                    leptos::logging::error!("Failed to restore version: {e}");
                }
            }
            set_restoring.set(None);
        });
    };

    // Relative time formatter matching the React formatDate function.
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
        // Fall back to a short date
        dt.format("%b %-d, %Y").to_string()
    }

    view! {
        <Show when=move || open.get()>
            // Backdrop overlay — click to close
            <div
                class="fixed inset-0 z-40 bg-black/50"
                on:click=move |_| on_close.run(())
            />

            // Slide-in panel
            <div class="fixed inset-y-0 right-0 z-50 w-96 max-w-[85vw] bg-card border-l border-border shadow-xl flex flex-col">
                // Header
                <div class="flex items-center justify-between px-4 py-3 border-b border-border bg-muted flex-shrink-0">
                    <div class="flex items-center gap-2">
                        // Clock icon (Heroicons outline)
                        <svg class="w-5 h-5 text-primary" xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 24 24" stroke-width="1.5" stroke="currentColor">
                            <path stroke-linecap="round" stroke-linejoin="round" d="M12 6v6h4.5m4.5 0a9 9 0 1 1-18 0 9 9 0 0 1 18 0Z" />
                        </svg>
                        <span class="font-medium text-foreground">"Version History"</span>
                    </div>
                    <button
                        class="p-1 text-muted-foreground hover:text-foreground rounded-md hover:bg-accent"
                        aria-label="Close history"
                        on:click=move |_| on_close.run(())
                    >
                        // X icon (Heroicons outline)
                        <svg class="w-5 h-5" xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 24 24" stroke-width="1.5" stroke="currentColor">
                            <path stroke-linecap="round" stroke-linejoin="round" d="M6 18 18 6M6 6l12 12" />
                        </svg>
                    </button>
                </div>

                // Content area
                <div class="flex-1 overflow-y-auto">
                    <Suspense fallback=move || view! {
                        <div class="flex items-center justify-center py-12">
                            <Spinner class="text-muted-foreground" />
                        </div>
                    }>
                        {move || {
                            versions.get().map(|result| match result {
                                Err(e) => view! {
                                    <div class="p-4 text-center">
                                        <p class="text-error-foreground mb-2">{e.to_string()}</p>
                                        <button
                                            class="inline-flex items-center justify-center rounded-md text-sm font-medium border border-input bg-background text-foreground shadow-sm hover:bg-accent hover:text-accent-foreground h-8 px-3"
                                            on:click=move |_| versions.refetch()
                                        >
                                            "Retry"
                                        </button>
                                    </div>
                                }.into_any(),
                                Ok(items) if items.is_empty() => view! {
                                    <div class="p-6 text-center">
                                        // Clock icon for empty state
                                        <svg class="w-12 h-12 mx-auto text-muted-foreground/50 mb-3" xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 24 24" stroke-width="1.5" stroke="currentColor">
                                            <path stroke-linecap="round" stroke-linejoin="round" d="M12 6v6h4.5m4.5 0a9 9 0 1 1-18 0 9 9 0 0 1 18 0Z" />
                                        </svg>
                                        <p class="text-muted-foreground text-sm">"No version history yet"</p>
                                        <p class="text-muted-foreground text-xs mt-1">
                                            "Versions are created when you save changes"
                                        </p>
                                    </div>
                                }.into_any(),
                                Ok(items) => view! {
                                    <div class="divide-y divide-border/50">
                                        <For
                                            each=move || items.clone()
                                            key=|v| v.version_number
                                            children=move |version: VersionSummary| {
                                                let ver_num = version.version_number;
                                                let is_restoring = move || restoring.get() == Some(ver_num);
                                                let any_restoring = move || restoring.get().is_some();

                                                view! {
                                                    <div class="px-4 py-3 hover:bg-accent transition-colors">
                                                        <div class="flex items-start justify-between">
                                                            <div class="flex-1 min-w-0">
                                                                <div class="flex items-center gap-2">
                                                                    // Version number badge
                                                                    <span class="px-1.5 py-0.5 bg-primary/10 text-primary text-xs rounded font-medium">
                                                                        {format!("v{ver_num}")}
                                                                    </span>
                                                                    <span class="text-sm font-medium text-foreground truncate">
                                                                        {version.title.clone()}
                                                                    </span>
                                                                </div>
                                                                // Change summary
                                                                {version.change_summary.clone().map(|summary| view! {
                                                                    <p class="text-xs text-muted-foreground mt-1">{summary}</p>
                                                                })}
                                                                // Timestamp and author
                                                                <div class="flex items-center gap-2 mt-1">
                                                                    <span class="text-xs text-muted-foreground/70">
                                                                        {format_relative_time(&version.created_at)}
                                                                    </span>
                                                                    {version.created_by_name.clone().map(|name| view! {
                                                                        <span class="text-xs text-muted-foreground/70">
                                                                            {format!("by {name}")}
                                                                        </span>
                                                                    })}
                                                                </div>
                                                            </div>
                                                        </div>

                                                        // Restore button
                                                        <div class="flex items-center gap-2 mt-2">
                                                            <button
                                                                class="inline-flex items-center gap-1.5 p-1.5 text-xs text-muted-foreground hover:text-primary hover:bg-primary/10 rounded transition-colors disabled:opacity-50 disabled:cursor-not-allowed"
                                                                disabled=any_restoring
                                                                on:click=move |_| set_confirm_version.set(Some(ver_num))
                                                            >
                                                                {move || if is_restoring() {
                                                                    view! { <Spinner class="text-primary" /> }.into_any()
                                                                } else {
                                                                    // ArrowUturnLeft icon (Heroicons outline)
                                                                    view! {
                                                                        <svg class="w-4 h-4" xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 24 24" stroke-width="1.5" stroke="currentColor">
                                                                            <path stroke-linecap="round" stroke-linejoin="round" d="M9 15 3 9m0 0 6-6M3 9h12a6 6 0 0 1 0 12h-3" />
                                                                        </svg>
                                                                    }.into_any()
                                                                }}
                                                                "Restore"
                                                            </button>
                                                        </div>
                                                    </div>
                                                }
                                            }
                                        />
                                    </div>
                                }.into_any(),
                            })
                        }}
                    </Suspense>
                </div>
            </div>

            // Confirm dialog for restore
            {move || confirm_version.get().map(|ver_num| view! {
                <div
                    class="fixed inset-0 z-[60] bg-black/50 flex items-center justify-center"
                    on:click=move |_| set_confirm_version.set(None)
                >
                    <div
                        class="bg-card border border-border rounded-xl shadow-xl max-w-md w-full mx-4 p-6"
                        on:click=|ev| ev.stop_propagation()
                    >
                        <h3 class="text-lg font-semibold text-foreground mb-2">
                            "Restore Version?"
                        </h3>
                        <p class="text-sm text-muted-foreground mb-6">
                            {format!("Restore to version {ver_num}? This will create a new version with that content.")}
                        </p>
                        <div class="flex justify-end gap-3">
                            <button
                                class="inline-flex items-center justify-center gap-2 whitespace-nowrap rounded-md text-sm font-medium transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring h-9 px-4 py-2 border border-input bg-background text-foreground shadow-sm hover:bg-accent hover:text-accent-foreground"
                                on:click=move |_| set_confirm_version.set(None)
                            >
                                "Cancel"
                            </button>
                            <button
                                class="inline-flex items-center justify-center gap-2 whitespace-nowrap rounded-md text-sm font-medium transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring h-9 px-4 py-2 bg-primary text-primary-foreground shadow hover:bg-primary/90"
                                on:click=move |_| handle_restore(ver_num)
                            >
                                "Restore"
                            </button>
                        </div>
                    </div>
                </div>
            })}
        </Show>
    }
}
