// SPDX-License-Identifier: AGPL-3.0-or-later

//! Insert Dashboard Link Modal — matches `apps/frontend/src/components/InsertDashboardLinkModal.jsx` exactly.
//!
//! A simplified modal for selecting an existing dashboard and inserting a markdown
//! link to it. Unlike `SaveDashboardModal`, there is no "create new" option — only
//! a scrollable list of existing dashboards to pick from.
//!
//! On insert, generates markdown `[{title}](/dashboard/{id})` and passes it to the
//! `on_insert` callback.

use std::sync::Arc;

use leptos::prelude::*;

use crate::components::alert::{Alert, AlertVariant};
use crate::components::modal::{Modal, ModalSize};
use crate::components::spinner::Spinner;
use crate::server_fns::dashboards::{list_dashboards, DashboardListItem};

// ─── Constants ──────────────────────────────────────────────────────────────

/// Button base classes — copied from `button.rs` BASE constant.
const BTN_BASE: &str = "inline-flex items-center justify-center gap-2 whitespace-nowrap rounded-md text-sm font-medium transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:pointer-events-none disabled:opacity-50 [&_svg]:pointer-events-none [&_svg]:size-4 [&_svg]:shrink-0";

/// Default button variant classes.
const BTN_DEFAULT: &str = "bg-primary text-primary-foreground shadow hover:bg-primary/90";

/// Outline button variant classes.
const BTN_OUTLINE: &str = "border border-input bg-background text-foreground shadow-sm hover:bg-accent hover:text-accent-foreground";

/// Default button size classes.
const BTN_SIZE: &str = "h-9 px-4 py-2";

// ─── Date formatting ────────────────────────────────────────────────────────

/// Matches the React `formatDate` function in InsertDashboardLinkModal.jsx:
/// "Today", "Yesterday", "N days ago", then short date format.
fn format_date(iso: &str) -> String {
    let Ok(dt) = chrono::DateTime::parse_from_rfc3339(iso) else {
        return iso.to_string();
    };
    let now = chrono::Utc::now();
    let diff = now.signed_duration_since(dt);
    let days = diff.num_days();

    if days < 1 {
        return "Today".to_string();
    }
    if days == 1 {
        return "Yesterday".to_string();
    }
    if days < 7 {
        return format!("{days} days ago");
    }

    // Short date: "Mar 22" or "Mar 22, 2025" if different year
    let dt_utc = dt.with_timezone(&chrono::Utc);
    let now_year = now.format("%Y").to_string();
    let dt_year = dt_utc.format("%Y").to_string();

    if dt_year == now_year {
        dt_utc.format("%b %-d").to_string()
    } else {
        dt_utc.format("%b %-d, %Y").to_string()
    }
}

// ─── SVG Icons ──────────────────────────────────────────────────────────────

/// Dashboard icon — React: grid/layout SVG path from InsertDashboardLinkModal.jsx line 132
fn dashboard_icon(class: &str) -> impl IntoView {
    let class = class.to_string();
    view! {
        <svg class=class fill="none" stroke="currentColor" viewBox="0 0 24 24">
            <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M9 17V7m0 10a2 2 0 01-2 2H5a2 2 0 01-2-2V7a2 2 0 012-2h2a2 2 0 012 2m0 10a2 2 0 002 2h2a2 2 0 002-2M9 7a2 2 0 012-2h2a2 2 0 012 2m0 10V7m0 10a2 2 0 002 2h2a2 2 0 002-2V7a2 2 0 00-2-2h-2a2 2 0 00-2 2" />
        </svg>
    }
}

/// Checkmark circle icon — React: filled circle with checkmark, viewBox="0 0 20 20"
fn check_circle_icon(class: &str) -> impl IntoView {
    let class = class.to_string();
    view! {
        <svg class=class fill="currentColor" viewBox="0 0 20 20">
            <path fill-rule="evenodd" d="M10 18a8 8 0 100-16 8 8 0 000 16zm3.707-9.293a1 1 0 00-1.414-1.414L9 10.586 7.707 9.293a1 1 0 00-1.414 1.414l2 2a1 1 0 001.414 0l4-4z" clip-rule="evenodd" />
        </svg>
    }
}

// ─── Component ──────────────────────────────────────────────────────────────

/// Insert Dashboard Link Modal — select an existing dashboard to insert a markdown link.
///
/// React reference: `apps/frontend/src/components/InsertDashboardLinkModal.jsx`
#[component]
pub fn InsertDashboardLinkModal(
    /// Whether the modal is open
    #[prop(into)]
    open: Signal<bool>,
    /// Callback to close the modal
    on_close: Callback<()>,
    /// Callback with the markdown link to insert: [Title](/dashboard/{id})
    on_insert: Callback<String>,
) -> impl IntoView {
    // ── State ────────────────────────────────────────────────────────────
    let (selected_dashboard_id, set_selected_dashboard_id) = signal(Option::<String>::None);
    let (error, set_error) = signal(Option::<String>::None);

    // ── Load dashboards when modal opens ─────────────────────────────────
    // React: useEffect on isOpen → loadDashboards()
    let dashboards_resource = Resource::new(
        move || open.get(),
        move |is_open| async move {
            if !is_open {
                return Ok(Vec::<DashboardListItem>::new());
            }
            list_dashboards(None, Some("recent".to_string()), Some(50)).await
        },
    );

    // Reset state when modal opens
    // React: useEffect sets selectedDashboard=null, error=null
    Effect::new(move || {
        if open.get() {
            set_selected_dashboard_id.set(None);
            set_error.set(None);
        }
    });

    // ── Insert handler ──────────────────────────────────────────────────
    // React: handleInsert — generates markdown link and calls onSelect, then onClose
    let handle_insert = Callback::new(move |()| {
        let Some(dashboard_id) = selected_dashboard_id.get_untracked() else {
            return;
        };

        // We need the title for the markdown link. Read it from the resource.
        let dashboards = dashboards_resource
            .get()
            .and_then(|r| r.ok())
            .unwrap_or_default();

        let title = dashboards
            .iter()
            .find(|d| d.dashboard_id == dashboard_id)
            .map(|d| {
                if d.title.is_empty() {
                    "Untitled Dashboard".to_string()
                } else {
                    d.title.clone()
                }
            })
            .unwrap_or_else(|| "Untitled Dashboard".to_string());

        let markdown_link = format!("[{title}](/dashboard/{dashboard_id})");
        on_insert.run(markdown_link);
        on_close.run(());
    });

    // ── Handlers ─────────────────────────────────────────────────────────

    let handle_select = move |dashboard_id: String| {
        set_selected_dashboard_id.set(Some(dashboard_id));
        set_error.set(None);
    };

    // ── Footer ───────────────────────────────────────────────────────────
    // React footer: Cancel + Insert Link button
    let cancel_class = format!("{BTN_BASE} {BTN_OUTLINE} {BTN_SIZE}");
    let insert_class = format!("{BTN_BASE} {BTN_DEFAULT} {BTN_SIZE}");

    let cancel_class_clone = cancel_class.clone();
    let insert_class_clone = insert_class.clone();

    let footer_view: ChildrenFn = Arc::new(move || {
        let cancel_class = cancel_class_clone.clone();
        let insert_class = insert_class_clone.clone();

        let is_insert_disabled = selected_dashboard_id.get().is_none();

        view! {
            <button
                class=cancel_class
                on:click=move |_| on_close.run(())
            >
                "Cancel"
            </button>
            <button
                class=insert_class
                on:click=move |_| handle_insert.run(())
                disabled=is_insert_disabled
            >
                "Insert Link"
            </button>
        }
        .into_any()
    });

    // ── View ─────────────────────────────────────────────────────────────
    view! {
        <Modal
            show=open
            on_close=on_close
            title="Insert Dashboard Link"
            size=ModalSize::Lg
            footer=footer_view
        >
            // React: subtitle
            <p class="text-sm text-muted-foreground mb-4">
                "Select a dashboard to insert a link to"
            </p>

            // React: error alert
            {move || {
                error.get().map(|err| view! {
                    <Alert variant=AlertVariant::Error class="mb-4">
                        {err}
                    </Alert>
                })
            }}

            // React: body — flex-1 overflow-y-auto
            <div class="flex-1 overflow-y-auto">
                <Suspense fallback=move || view! {
                    <div class="flex items-center justify-center py-12">
                        <div class="flex items-center gap-2 text-muted-foreground">
                            <Spinner />
                            "Loading dashboards..."
                        </div>
                    </div>
                }>
                    {move || {
                        dashboards_resource.get().map(|result| {
                            let dashboards = result.unwrap_or_default();
                            let dashboards_len = dashboards.len();
                            let dashboards_list = dashboards.clone();

                            view! {
                                // React: empty state
                                {if dashboards_len == 0 {
                                    Some(view! {
                                        <div class="text-center py-8">
                                            <p class="text-sm text-muted-foreground">
                                                "No dashboards found"
                                            </p>
                                            <p class="text-xs text-muted-foreground/70 mt-1">
                                                "Create a dashboard first to link to it"
                                            </p>
                                        </div>
                                    })
                                } else {
                                    None
                                }}

                                // React: dashboard list — space-y-2
                                {if dashboards_len > 0 {
                                    Some(view! {
                                        <div class="space-y-2">
                                            <For
                                                each=move || dashboards_list.clone()
                                                key=|d| d.dashboard_id.clone()
                                                let:dashboard
                                            >
                                                <DashboardListEntry
                                                    dashboard=dashboard
                                                    selected_dashboard_id=selected_dashboard_id
                                                    on_select=handle_select.clone()
                                                />
                                            </For>
                                        </div>
                                    })
                                } else {
                                    None
                                }}
                            }
                        })
                    }}
                </Suspense>
            </div>
        </Modal>
    }
}

// ─── Sub-components ─────────────────────────────────────────────────────────

/// A single dashboard entry in the list.
/// React: The `dashboards.map((dashboard) => ...)` block (lines 115-150).
#[component]
fn DashboardListEntry(
    dashboard: DashboardListItem,
    #[prop(into)]
    selected_dashboard_id: Signal<Option<String>>,
    /// Called with the dashboard_id when clicked
    on_select: impl Fn(String) + 'static + Clone,
) -> impl IntoView {
    let id = dashboard.dashboard_id.clone();
    let id_for_click = id.clone();
    let id_for_class = id.clone();
    let id_for_icon_class = id.clone();
    let id_for_icon_text = id.clone();
    let id_for_check = id.clone();
    let title = dashboard.title.clone();
    let created_at = format_date(&dashboard.created_at);
    let on_select_clone = on_select.clone();

    view! {
        <div
            on:click=move |_| on_select_clone(id_for_click.clone())
            class=move || {
                let selected = selected_dashboard_id.get().as_deref() == Some(&*id_for_class);
                if selected {
                    "border-2 rounded-lg p-4 cursor-pointer transition-all border-primary bg-primary/10"
                } else {
                    "border-2 rounded-lg p-4 cursor-pointer transition-all border-border hover:border-input hover:bg-accent"
                }
            }
        >
            <div class="flex items-center gap-3">
                // Icon container
                <div class=move || {
                    let selected = selected_dashboard_id.get().as_deref() == Some(&*id_for_icon_class);
                    if selected {
                        "flex-shrink-0 w-10 h-10 rounded-lg flex items-center justify-center bg-primary"
                    } else {
                        "flex-shrink-0 w-10 h-10 rounded-lg flex items-center justify-center bg-accent"
                    }
                }>
                    {move || {
                        let selected = selected_dashboard_id.get().as_deref() == Some(&*id_for_icon_text);
                        let class = if selected {
                            "w-5 h-5 text-white"
                        } else {
                            "w-5 h-5 text-muted-foreground"
                        };
                        dashboard_icon(class)
                    }}
                </div>

                // Title + date
                <div class="flex-1 min-w-0">
                    <h3 class="text-base font-medium text-foreground truncate">
                        {if title.is_empty() { "Untitled Dashboard".to_string() } else { title.clone() }}
                    </h3>
                    <p class="text-sm text-muted-foreground mt-0.5">
                        {created_at.clone()}
                    </p>
                </div>

                // Checkmark when selected
                {move || {
                    let selected = selected_dashboard_id.get().as_deref() == Some(&*id_for_check);
                    if selected {
                        Some(check_circle_icon("w-5 h-5 text-primary flex-shrink-0"))
                    } else {
                        None
                    }
                }}
            </div>
        </div>
    }
}
