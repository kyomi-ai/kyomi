// SPDX-License-Identifier: AGPL-3.0-or-later

//! Dashboard viewer page — read-only view of a single dashboard.
//!
//! Route: `/dashboard/:id`
//!
//! Fetches the dashboard by ID from the URL params, displays a toolbar
//! with navigation and action buttons, and renders the content using
//! `MarkdownRenderer`.

use leptos::prelude::*;
use leptos_router::hooks::use_params_map;

use crate::components::dashboard::MarkdownRenderer;
use crate::components::Spinner;
use crate::server_fns::dashboards::get_dashboard;

/// Read-only dashboard viewer page.
///
/// Extracts `id` from the URL path params, fetches the dashboard detail
/// via `get_dashboard`, and renders the content with `MarkdownRenderer`.
#[component]
pub fn DashboardViewerPage() -> impl IntoView {
    let params = use_params_map();
    let dashboard_id = Memo::new(move |_| {
        params.get().get("id").unwrap_or_default()
    });

    // Fetch dashboard detail, keyed on dashboard_id
    let dashboard_resource = Resource::new(
        move || dashboard_id.get(),
        move |id| get_dashboard(id),
    );

    view! {
        <Suspense fallback=move || view! {
            <div class="flex h-full items-center justify-center bg-muted">
                <Spinner class="h-8 w-8 text-muted-foreground" />
            </div>
        }>
            {move || {
                dashboard_resource.get().map(|result| {
                    match result {
                        Err(e) => {
                            view! {
                                <div class="flex h-full items-center justify-center bg-muted">
                                    <div class="text-center">
                                        <h2 class="text-2xl font-bold text-foreground mb-4">
                                            "Dashboard Not Found"
                                        </h2>
                                        <p class="text-muted-foreground mb-6">
                                            {e.to_string()}
                                        </p>
                                        <a
                                            href="/dashboards"
                                            class="px-6 py-3 text-white bg-primary hover:bg-primary/90 rounded-lg transition-colors inline-block"
                                        >
                                            "Back to Dashboards"
                                        </a>
                                    </div>
                                </div>
                            }.into_any()
                        }
                        Ok(dashboard) => {
                            let title = dashboard.title.clone();
                            let content = dashboard.content.clone();
                            let edit_href = format!("/dashboard/{}/edit", dashboard.dashboard_id);
                            let created_at = dashboard.created_at.clone();
                            let updated_at = dashboard.updated_at.clone();

                            view! {
                                <div class="flex flex-col h-full bg-muted overflow-hidden" style:flex-direction="column">
                                    // Toolbar
                                    <div class="h-16 bg-card border-b border-border px-4 md:px-6 flex-shrink-0 flex items-center justify-between">
                                        // Left: back button + title
                                        <div class="flex items-center gap-4 flex-1 min-w-0">
                                            <a
                                                href="/dashboards"
                                                class="p-2 text-muted-foreground hover:text-foreground hover:bg-accent rounded-lg transition-colors flex-shrink-0"
                                                aria-label="Back to dashboards"
                                            >
                                                <svg class="w-5 h-5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                                                    <path
                                                        stroke-linecap="round"
                                                        stroke-linejoin="round"
                                                        stroke-width="2"
                                                        d="M15 19l-7-7 7-7"
                                                    />
                                                </svg>
                                            </a>
                                            <h1 class="text-2xl font-bold text-foreground truncate">
                                                {title}
                                            </h1>
                                        </div>

                                        // Right: action buttons
                                        <div class="flex items-center gap-1 xl:gap-2 flex-shrink-0">
                                            // History button (placeholder)
                                            <button
                                                class="hidden md:flex items-center gap-2 px-2 xl:px-4 py-2 text-sm font-medium text-foreground bg-card border border-border hover:bg-accent rounded-lg transition-colors"
                                                aria-label="View version history"
                                            >
                                                <svg class="w-4 h-4 flex-shrink-0" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                                                    <path
                                                        stroke-linecap="round"
                                                        stroke-linejoin="round"
                                                        stroke-width="2"
                                                        d="M12 8v4l3 3m6-3a9 9 0 11-18 0 9 9 0 0118 0z"
                                                    />
                                                </svg>
                                                <span class="hidden xl:inline whitespace-nowrap">"History"</span>
                                            </button>

                                            // Edit button
                                            <a
                                                href=edit_href
                                                class="flex items-center gap-2 px-2 xl:px-4 py-2 text-sm font-medium text-white bg-primary hover:bg-primary/90 rounded-lg transition-colors"
                                            >
                                                <svg class="w-4 h-4 flex-shrink-0" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                                                    <path
                                                        stroke-linecap="round"
                                                        stroke-linejoin="round"
                                                        stroke-width="2"
                                                        d="M11 5H6a2 2 0 00-2 2v11a2 2 0 002 2h11a2 2 0 002-2v-5m-1.414-9.414a2 2 0 112.828 2.828L11.828 15H9v-2.828l8.586-8.586z"
                                                    />
                                                </svg>
                                                <span class="hidden xl:inline whitespace-nowrap">"Edit Dashboard"</span>
                                            </a>
                                        </div>
                                    </div>

                                    // Content area
                                    <div class="flex-1 overflow-y-auto p-4 md:p-6 bg-muted">
                                        <div class="bg-card rounded-lg border border-border shadow-sm min-h-full">
                                            <div class="p-4 md:p-6">
                                                {if content.trim().is_empty() {
                                                    let edit_href_empty = format!("/dashboard/{}/edit", dashboard.dashboard_id);
                                                    view! {
                                                        <div class="w-full text-center py-16">
                                                            <svg class="w-24 h-24 mx-auto text-muted-foreground mb-6" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                                                                <path
                                                                    stroke-linecap="round"
                                                                    stroke-linejoin="round"
                                                                    stroke-width="1.5"
                                                                    d="M9 12h6m-6 4h6m2 5H7a2 2 0 01-2-2V5a2 2 0 012-2h5.586a1 1 0 01.707.293l5.414 5.414a1 1 0 01.293.707V19a2 2 0 01-2 2z"
                                                                />
                                                            </svg>
                                                            <h3 class="text-xl font-semibold text-foreground mb-2">
                                                                "This dashboard is empty"
                                                            </h3>
                                                            <p class="text-muted-foreground mb-6">
                                                                "Click \"Edit Dashboard\" to add content and charts"
                                                            </p>
                                                            <a
                                                                href=edit_href_empty
                                                                class="inline-flex items-center gap-2 px-6 py-3 text-white bg-primary hover:bg-primary/90 rounded-lg transition-colors"
                                                            >
                                                                <svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                                                                    <path
                                                                        stroke-linecap="round"
                                                                        stroke-linejoin="round"
                                                                        stroke-width="2"
                                                                        d="M11 5H6a2 2 0 00-2 2v11a2 2 0 002 2h11a2 2 0 002-2v-5m-1.414-9.414a2 2 0 112.828 2.828L11.828 15H9v-2.828l8.586-8.586z"
                                                                    />
                                                                </svg>
                                                                "Edit Dashboard"
                                                            </a>
                                                        </div>
                                                    }.into_any()
                                                } else {
                                                    let content_for_signal = content.clone();
                                                    view! {
                                                        <div class="max-w-4xl mx-auto">
                                                            <MarkdownRenderer content=Signal::derive(move || content_for_signal.clone()) />
                                                        </div>
                                                    }.into_any()
                                                }}
                                            </div>
                                        </div>
                                    </div>

                                    // Footer with metadata
                                    <div class="bg-card border-t border-border px-4 md:px-6 py-3 flex-shrink-0">
                                        <div class="flex items-center justify-between text-xs text-muted-foreground">
                                            <div class="flex items-center gap-4">
                                                <div class="flex items-center gap-1">
                                                    <svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                                                        <path
                                                            stroke-linecap="round"
                                                            stroke-linejoin="round"
                                                            stroke-width="2"
                                                            d="M12 8v4l3 3m6-3a9 9 0 11-18 0 9 9 0 0118 0z"
                                                        />
                                                    </svg>
                                                    "Created " {created_at}
                                                </div>
                                                <div class="flex items-center gap-1">
                                                    <svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                                                        <path
                                                            stroke-linecap="round"
                                                            stroke-linejoin="round"
                                                            stroke-width="2"
                                                            d="M11 5H6a2 2 0 00-2 2v11a2 2 0 002 2h11a2 2 0 002-2v-5m-1.414-9.414a2 2 0 112.828 2.828L11.828 15H9v-2.828l8.586-8.586z"
                                                        />
                                                    </svg>
                                                    "Last updated " {updated_at}
                                                </div>
                                            </div>
                                        </div>
                                    </div>
                                </div>
                            }.into_any()
                        }
                    }
                })
            }}
        </Suspense>
    }
}
