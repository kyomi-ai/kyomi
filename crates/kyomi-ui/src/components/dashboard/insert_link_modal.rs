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
use crate::components::ModalListSkeleton;
use crate::server_fns::dashboards::{list_dashboards, DashboardListItem};

use super::shared::{BTN_BASE, BTN_DEFAULT, BTN_OUTLINE, BTN_SIZE, DashboardListEntry};

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
                disabled=move || selected_dashboard_id.get().is_none()
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

            // React: body — flex-1 overflow-y-auto.
            // Fixed height (matches `save_dashboard_modal.rs`, which shares this
            // list via `DashboardListEntry`) so the skeleton→loaded swap doesn't
            // shift the modal's height (KYO-233).
            <div class="flex-1 overflow-y-auto h-[420px]">
                <Suspense fallback=move || view! { <ModalListSkeleton /> }>
                    {move || {
                        dashboards_resource.get().map(|result| {
                            let dashboards = match result {
                                Ok(list) => list,
                                Err(e) => {
                                    set_error.set(Some(format!("Failed to load dashboards: {e}")));
                                    vec![]
                                }
                            };
                            let dashboards_len = dashboards.len();
                            let dashboards_list = dashboards.clone();

                            view! {
                                <div class="animate-fade-in">
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
                                                        on_select=handle_select
                                                    />
                                                </For>
                                            </div>
                                        })
                                    } else {
                                        None
                                    }}
                                </div>
                            }
                        })
                    }}
                </Suspense>
            </div>
        </Modal>
    }
}

