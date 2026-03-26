// SPDX-License-Identifier: AGPL-3.0-or-later

//! Save Dashboard Modal — matches `apps/frontend/src/components/SaveDashboardModal.jsx` exactly.
//!
//! Two modes:
//! 1. "Create new dashboard" — title input, creates new dashboard with chart appended
//! 2. "Add to existing dashboard" — scrollable list of existing dashboards, appends chart
//!
//! Uses the same server functions as the React version:
//! - `list_dashboards` to load existing dashboards
//! - `create_dashboard` to create a new one
//! - `get_dashboard` + `update_dashboard` to append to an existing one

use std::sync::Arc;

use leptos::prelude::*;

use crate::components::alert::{Alert, AlertVariant};
use crate::components::input::INPUT_CLASS;
use crate::components::modal::{Modal, ModalSize};
use crate::components::spinner::Spinner;
use crate::server_fns::dashboards::{
    create_dashboard, get_dashboard, list_dashboards, update_dashboard, DashboardListItem,
};

use super::shared::{
    check_circle_icon, BTN_BASE, BTN_DEFAULT, BTN_OUTLINE, BTN_SIZE, DashboardListEntry,
};

// ─── SVG Icons (local to this module) ───────────────────────────────────────

/// Plus icon — React: `<path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M12 4v16m8-8H4" />`
fn plus_icon(class: &str) -> impl IntoView {
    let class = class.to_string();
    view! {
        <svg class=class fill="none" stroke="currentColor" viewBox="0 0 24 24">
            <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M12 4v16m8-8H4" />
        </svg>
    }
}

// ─── Component ──────────────────────────────────────────────────────────────

/// Save Dashboard Modal — create a new dashboard or add chart to an existing one.
///
/// React reference: `apps/frontend/src/components/SaveDashboardModal.jsx`
#[component]
pub fn SaveDashboardModal(
    /// Whether the modal is open
    #[prop(into)]
    open: Signal<bool>,
    /// The chart YAML to save
    chart_yaml: String,
    /// Callback to close the modal
    on_close: Callback<()>,
    /// Callback after successful save (passes dashboard_id)
    on_saved: Callback<String>,
) -> impl IntoView {
    // ── State ────────────────────────────────────────────────────────────
    let (is_creating_new, set_is_creating_new) = signal(false);
    let (new_dashboard_title, set_new_dashboard_title) = signal(String::new());
    let (selected_dashboard_id, set_selected_dashboard_id) = signal(Option::<String>::None);
    let (is_saving, set_is_saving) = signal(false);
    let (error, set_error) = signal(Option::<String>::None);

    // Clone chart_yaml for use in closures
    let chart_yaml_stored = StoredValue::new(chart_yaml);

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
    // React: useEffect sets title='', selectedId=null, isCreatingNew=false, error=null
    Effect::new(move || {
        if open.get() {
            set_new_dashboard_title.set(String::new());
            set_selected_dashboard_id.set(None);
            set_is_creating_new.set(false);
            set_error.set(None);
        }
    });

    // ── Unified save handler ─────────────────────────────────────────────
    // Uses a single closure to avoid type-mismatch between two different closures.
    // Reads `is_creating_new` to decide which path to take.
    let handle_save = Callback::new(move |()| {
        let creating = is_creating_new.get_untracked();

        if creating {
            let title = new_dashboard_title.get_untracked();
            if title.trim().is_empty() {
                return;
            }
            let chart_yaml = chart_yaml_stored.get_value();

            set_is_saving.set(true);
            set_error.set(None);

            leptos::task::spawn_local(async move {
                match create_dashboard(title.trim().to_string(), Some(chart_yaml)).await {
                    Ok(dashboard_id) => {
                        set_new_dashboard_title.set(String::new());
                        set_is_creating_new.set(false);
                        on_saved.run(dashboard_id);
                        on_close.run(());
                    }
                    Err(e) => {
                        set_error.set(Some(e.to_string()));
                    }
                }
                set_is_saving.set(false);
            });
        } else {
            let Some(dashboard_id) = selected_dashboard_id.get_untracked() else {
                return;
            };
            let chart_yaml = chart_yaml_stored.get_value();

            set_is_saving.set(true);
            set_error.set(None);

            leptos::task::spawn_local(async move {
                // First fetch existing dashboard content, then append chart
                match get_dashboard(dashboard_id.clone()).await {
                    Ok(dashboard) => {
                        let new_content = if dashboard.content.is_empty() {
                            chart_yaml
                        } else {
                            format!("{}\n\n{}", dashboard.content, chart_yaml)
                        };

                        match update_dashboard(
                            dashboard_id.clone(),
                            None,
                            Some(new_content),
                            Some("Added chart".to_string()),
                        )
                        .await
                        {
                            Ok(()) => {
                                set_selected_dashboard_id.set(None);
                                on_saved.run(dashboard_id);
                                on_close.run(());
                            }
                            Err(e) => {
                                set_error.set(Some(e.to_string()));
                            }
                        }
                    }
                    Err(e) => {
                        set_error.set(Some(format!("Failed to load dashboard: {e}")));
                    }
                }
                set_is_saving.set(false);
            });
        }
    });

    // ── Handlers ─────────────────────────────────────────────────────────

    // React: handleSelectNew
    let handle_select_new = Callback::new(move |()| {
        set_is_creating_new.set(true);
        set_selected_dashboard_id.set(None);
        set_error.set(None);
    });

    // React: handleSelectExisting
    let handle_select_existing = move |dashboard_id: String| {
        set_selected_dashboard_id.set(Some(dashboard_id));
        set_is_creating_new.set(false);
        set_error.set(None);
    };

    // ── Footer ───────────────────────────────────────────────────────────
    // React footer: Cancel + Save button (text varies by mode)
    let cancel_class = format!("{BTN_BASE} {BTN_OUTLINE} {BTN_SIZE}");
    let save_class = format!("{BTN_BASE} {BTN_DEFAULT} {BTN_SIZE}");

    let cancel_class_clone = cancel_class.clone();
    let save_class_clone = save_class.clone();

    let footer_view: ChildrenFn = Arc::new(move || {
        let cancel_class = cancel_class_clone.clone();
        let save_class = save_class_clone.clone();

        let is_creating = is_creating_new.get();
        let is_save_disabled = is_saving.get()
            || (is_creating && new_dashboard_title.get().trim().is_empty())
            || (!is_creating && selected_dashboard_id.get().is_none());

        let save_text = if is_saving.get() {
            "Saving..."
        } else if is_creating {
            "Create Dashboard"
        } else {
            "Add to Dashboard"
        };

        view! {
            <button
                class=cancel_class
                on:click=move |_| on_close.run(())
                disabled=is_saving.get()
            >
                "Cancel"
            </button>
            <button
                class=save_class
                on:click=move |_| handle_save.run(())
                disabled=is_save_disabled
            >
                {save_text}
            </button>
        }
        .into_any()
    });

    // ── View ─────────────────────────────────────────────────────────────
    view! {
        <Modal
            show=open
            on_close=on_close
            title="Save to Dashboard"
            size=ModalSize::Lg
            footer=footer_view
        >
            // React: subtitle
            <p class="text-sm text-muted-foreground mb-4">
                "Create a new dashboard or add to an existing one"
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
                                <div class="space-y-2">
                                    // ── Create New Dashboard Option ──
                                    <CreateNewOption
                                        is_creating_new=is_creating_new
                                        new_dashboard_title=new_dashboard_title
                                        set_new_dashboard_title=set_new_dashboard_title
                                        is_saving=is_saving
                                        on_select=handle_select_new
                                        on_save=handle_save
                                        on_close=on_close
                                    />

                                    // ── Divider ──
                                    // React: divider with "or add to existing" text
                                    {if dashboards_len > 0 {
                                        Some(view! {
                                            <div class="relative py-2">
                                                <div class="absolute inset-0 flex items-center">
                                                    <div class="w-full border-t border-border"></div>
                                                </div>
                                                <div class="relative flex justify-center">
                                                    <span class="px-3 bg-background text-sm text-muted-foreground">
                                                        "or add to existing"
                                                    </span>
                                                </div>
                                            </div>
                                        })
                                    } else {
                                        None
                                    }}

                                    // ── Existing Dashboards List ──
                                    <For
                                        each=move || dashboards_list.clone()
                                        key=|d| d.dashboard_id.clone()
                                        let:dashboard
                                    >
                                        <DashboardListEntry
                                            dashboard=dashboard
                                            selected_dashboard_id=selected_dashboard_id
                                            on_select=handle_select_existing
                                        />
                                    </For>

                                    // ── Empty state ──
                                    // React: "No existing dashboards yet"
                                    {move || {
                                        if dashboards_len == 0 && !is_creating_new.get() {
                                            Some(view! {
                                                <div class="text-center py-8">
                                                    <p class="text-sm text-muted-foreground">
                                                        "No existing dashboards yet"
                                                    </p>
                                                </div>
                                            })
                                        } else {
                                            None
                                        }
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

// ─── Sub-components ─────────────────────────────────────────────────────────

/// The "Create New Dashboard" card option.
/// React: The first `<div onClick={handleSelectNew}>` block (lines 165-218).
#[component]
fn CreateNewOption(
    #[prop(into)]
    is_creating_new: Signal<bool>,
    #[prop(into)]
    new_dashboard_title: Signal<String>,
    set_new_dashboard_title: WriteSignal<String>,
    #[prop(into)]
    is_saving: Signal<bool>,
    /// Called when the card is clicked
    on_select: Callback<()>,
    /// Called when Enter is pressed in the title input
    on_save: Callback<()>,
    /// Called when Escape is pressed
    on_close: Callback<()>,
) -> impl IntoView {
    view! {
        <div
            on:click=move |_| on_select.run(())
            class=move || {
                if is_creating_new.get() {
                    "border-2 rounded-lg p-4 transition-all border-primary bg-primary/10 cursor-pointer"
                } else {
                    "border-2 rounded-lg p-4 transition-all border-border hover:border-input hover:bg-accent cursor-pointer"
                }
            }
        >
            <div class="flex items-center gap-3">
                // Icon container
                <div class=move || {
                    if is_creating_new.get() {
                        "flex-shrink-0 w-10 h-10 rounded-lg flex items-center justify-center bg-primary"
                    } else {
                        "flex-shrink-0 w-10 h-10 rounded-lg flex items-center justify-center bg-accent"
                    }
                }>
                    {move || {
                        let class = if is_creating_new.get() {
                            "w-5 h-5 text-primary-foreground"
                        } else {
                            "w-5 h-5 text-muted-foreground"
                        };
                        plus_icon(class)
                    }}
                </div>

                // Text
                <div class="flex-1">
                    <h3 class="text-base font-medium text-foreground">
                        "Create New Dashboard"
                    </h3>
                    <p class="text-sm mt-0.5 text-muted-foreground">
                        "Start fresh with a new dashboard"
                    </p>
                </div>

                // Checkmark when selected
                {move || {
                    if is_creating_new.get() {
                        Some(check_circle_icon("w-5 h-5 text-primary flex-shrink-0"))
                    } else {
                        None
                    }
                }}
            </div>

            // Title input — shown when creating mode is active
            // React: "mt-4 pl-13" with input field
            {move || {
                if is_creating_new.get() {
                    Some(view! {
                        <div class="mt-4 pl-13">
                            <input
                                type="text"
                                prop:value=move || new_dashboard_title.get()
                                on:input=move |ev| {
                                    set_new_dashboard_title.set(event_target_value(&ev));
                                }
                                on:keydown=move |ev: web_sys::KeyboardEvent| {
                                    if ev.key() == "Enter"
                                        && !ev.shift_key()
                                        && !new_dashboard_title.get_untracked().trim().is_empty()
                                    {
                                        ev.prevent_default();
                                        on_save.run(());
                                    } else if ev.key() == "Escape" {
                                        on_close.run(());
                                    }
                                }
                                on:click=|ev: web_sys::MouseEvent| ev.stop_propagation()
                                placeholder="Enter dashboard title..."
                                class=INPUT_CLASS
                                autofocus=true
                                disabled=move || is_saving.get()
                            />
                        </div>
                    })
                } else {
                    None
                }
            }}
        </div>
    }
}

