// SPDX-License-Identifier: AGPL-3.0-or-later

//! Data Sources settings page — list, toggle, and delete datasources.
//!
//! Replaces the list view from `apps/frontend/src/components/settings/DatasourceSettings.jsx`.
//!
//! The DatasourceModal (create/edit) is NOT ported yet. The "Add Data Source"
//! and "Settings" buttons link to the React app at `/settings/datasources`.
//! This is intentional — we don't build incomplete UI.

use leptos::prelude::*;
use leptos_icons::Icon;

use crate::components::{
    Badge, BadgeVariant, Card, ConfirmDialog, Skeleton, Switch,
};
use crate::server_fns::datasources::*;

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Credential status badge text, or None if no badge needed.
fn credential_badge(ds: &DatasourceInfo) -> Option<(&'static str, BadgeVariant)> {
    match ds.credential_status.as_str() {
        "missing" => Some(("Needs Setup", BadgeVariant::Warning)),
        "expired" => Some(("Expired", BadgeVariant::Warning)),
        _ => None,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Main Page
// ─────────────────────────────────────────────────────────────────────────────

/// Data Sources settings page content.
#[component]
pub fn DatasourcesPage() -> impl IntoView {
    let datasources_resource = Resource::new(|| (), |_| list_datasources());

    view! {
        <Suspense fallback=move || view! { <DatasourcesLoadingSkeleton/> }>
            {move || Suspend::new(async move {
                match datasources_resource.await {
                    Ok(datasources) => {
                        view! {
                            <DatasourcesContent
                                initial_datasources=datasources
                                _datasources_resource=datasources_resource
                            />
                        }.into_any()
                    }
                    Err(e) => view! {
                        <div class="p-4 sm:p-6 space-y-6">
                            <div>
                                <h2 class="text-xl font-bold text-foreground">"Datasources"</h2>
                                <p class="text-sm text-muted-foreground">
                                    "Manage database connections"
                                </p>
                            </div>
                            <Card>
                                <div class="p-6">
                                    <p class="text-error-foreground">
                                        {format!("Failed to load datasources: {e}")}
                                    </p>
                                </div>
                            </Card>
                        </div>
                    }.into_any(),
                }
            })}
        </Suspense>
    }
}

/// Loading skeleton shown while data is being fetched.
///
/// Matches the React loading state: `<Skeleton className="h-8 w-64" />` + `<Skeleton className="h-24 w-full" />`
#[component]
fn DatasourcesLoadingSkeleton() -> impl IntoView {
    view! {
        <div class="p-6 space-y-4" style:display="block">
            <Skeleton class="h-8 w-64"/>
            <Skeleton class="h-24 w-full"/>
        </div>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Content (with data loaded)
// ─────────────────────────────────────────────────────────────────────────────

/// Main content rendered after data is loaded.
#[component]
fn DatasourcesContent(
    initial_datasources: Vec<DatasourceInfo>,
    _datasources_resource: Resource<Result<Vec<DatasourceInfo>, ServerFnError>>,
) -> impl IntoView {
    let (datasources, set_datasources) = signal(initial_datasources);

    // ── Delete state ────────────────────────────────────────────────────
    let (delete_dialog_open, set_delete_dialog_open) = signal(false);
    let (datasource_to_delete, set_datasource_to_delete) =
        signal::<Option<DatasourceInfo>>(None);

    let on_delete_confirm = Callback::new(move |()| {
        set_delete_dialog_open.set(false);
        let ds = datasource_to_delete.get_untracked();
        if let Some(ds) = ds {
            let ds_id = ds.id.clone();
            leptos::task::spawn_local(async move {
                match delete_datasource(ds_id).await {
                    Ok(()) => {
                        // Remove from local list
                        set_datasources.update(|list| {
                            list.retain(|d| d.id != ds.id);
                        });
                    }
                    Err(e) => {
                        // Log error — toast system will be added later
                        leptos::logging::error!("Failed to delete datasource: {e}");
                    }
                }
                set_datasource_to_delete.set(None);
            });
        }
    });

    let on_delete_cancel = Callback::new(move |()| {
        set_delete_dialog_open.set(false);
        set_datasource_to_delete.set(None);
    });

    // ── Delete confirmation dialog message ──────────────────────────────
    let delete_title = "Delete Datasource?".to_string();
    let delete_message = move || {
        datasource_to_delete
            .get()
            .map(|ds| {
                format!(
                    "Are you sure you want to delete \"{}\"? This cannot be undone.",
                    ds.name
                )
            })
            .unwrap_or_default()
    };

    view! {
        <div class="p-4 sm:p-6 space-y-6">
            // Header
            <div class="flex items-center justify-between">
                <div>
                    <h2 class="text-xl font-bold text-foreground">"Datasources"</h2>
                    <p class="text-sm text-muted-foreground">
                        "Manage database connections"
                    </p>
                </div>
                // "Add Datasource" links to React app since modal is not ported
                <a href="/settings/datasources" class="inline-flex items-center justify-center gap-2 whitespace-nowrap rounded-md text-sm font-medium transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring h-9 px-4 py-2 bg-primary text-primary-foreground shadow hover:bg-primary/90">
                    <span class="h-4 w-4 inline-flex items-center justify-center">
                        <Icon icon=icondata_lu::LuPlus/>
                    </span>
                    "Add Datasource"
                </a>
            </div>

            // Datasources List
            <Card>
                <div class="p-0">
                    <Show
                        when=move || !datasources.get().is_empty()
                        fallback=move || view! {
                            <div class="text-center py-12">
                                <span class="mx-auto h-12 w-12 text-muted-foreground flex items-center justify-center">
                                    <Icon icon=icondata_lu::LuDatabase/>
                                </span>
                                <p class="mt-4 text-sm text-muted-foreground">
                                    "No datasources configured"
                                </p>
                                <a href="/settings/datasources" class="mt-4 inline-flex items-center justify-center gap-2 whitespace-nowrap rounded-md text-sm font-medium transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring h-9 px-4 py-2 bg-primary text-primary-foreground shadow hover:bg-primary/90">
                                    <span class="h-4 w-4 inline-flex items-center justify-center">
                                        <Icon icon=icondata_lu::LuPlus/>
                                    </span>
                                    "Add Datasource"
                                </a>
                            </div>
                        }
                    >
                        <div class="divide-y divide-border">
                            <For
                                each=move || datasources.get()
                                key=|ds| ds.id.clone()
                                let:ds
                            >
                                <DatasourceRow
                                    ds=ds
                                    set_datasources=set_datasources
                                    set_delete_dialog_open=set_delete_dialog_open
                                    set_datasource_to_delete=set_datasource_to_delete
                                />
                            </For>
                        </div>
                    </Show>
                </div>
            </Card>
        </div>

        // Delete Confirmation Dialog
        <ConfirmDialog
            open=Signal::from(delete_dialog_open)
            title=delete_title
            message=delete_message()
            confirm_text="Delete"
            on_confirm=on_delete_confirm
            on_cancel=on_delete_cancel
        />
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Datasource Row
// ─────────────────────────────────────────────────────────────────────────────

/// A single datasource row in the list.
///
/// Matches the React `datasources.map((ds) => ...)` render block.
#[component]
fn DatasourceRow(
    ds: DatasourceInfo,
    set_datasources: WriteSignal<Vec<DatasourceInfo>>,
    set_delete_dialog_open: WriteSignal<bool>,
    set_datasource_to_delete: WriteSignal<Option<DatasourceInfo>>,
) -> impl IntoView {
    // ── Toggle state ────────────────────────────────────────────────────
    let ds_for_toggle = ds.clone();
    let (local_enabled, set_local_enabled) = signal(ds.user_enabled);
    let (is_toggling, set_is_toggling) = signal(false);

    let can_enable = ds.can_enable;
    let switch_disabled = !can_enable || is_toggling.get_untracked();

    let on_toggle = Callback::new(move |new_val: bool| {
        let ds_id = ds_for_toggle.id.clone();
        set_local_enabled.set(new_val);
        set_is_toggling.set(true);

        leptos::task::spawn_local(async move {
            match toggle_datasource(ds_id.clone(), new_val).await {
                Ok(()) => {
                    // Update the datasource list with new enabled state
                    set_datasources.update(|list| {
                        if let Some(d) = list.iter_mut().find(|d| d.id == ds_id) {
                            d.user_enabled = new_val;
                        }
                    });
                }
                Err(e) => {
                    // Revert optimistic update
                    set_local_enabled.set(!new_val);
                    leptos::logging::error!("Failed to toggle datasource: {e}");
                }
            }
            set_is_toggling.set(false);
        });
    });

    // ── Delete handler ──────────────────────────────────────────────────
    let ds_for_delete = ds.clone();
    let on_delete_click = move |_: leptos::ev::MouseEvent| {
        set_datasource_to_delete.set(Some(ds_for_delete.clone()));
        set_delete_dialog_open.set(true);
    };

    // ── Credential badge ────────────────────────────────────────────────
    let cred_badge = credential_badge(&ds);

    // ── Catalog attention ───────────────────────────────────────────────
    let show_catalog_warning = ds.can_enable && ds.needs_catalog_attention;

    // ── Toggle label ────────────────────────────────────────────────────
    let toggle_label = move || {
        if local_enabled.get() {
            "Enabled"
        } else {
            "Disabled"
        }
    };

    view! {
        <div class="flex flex-col sm:flex-row sm:items-center sm:justify-between p-4 gap-3 hover:bg-muted/50">
            // Left side: name, type badge, status badges
            <div class="flex items-center gap-3 min-w-0">
                // Datasource icon (using a database icon as placeholder)
                <span class="h-6 w-6 shrink-0 text-muted-foreground inline-flex items-center justify-center">
                    <Icon icon=icondata_lu::LuDatabase/>
                </span>
                <div class="min-w-0">
                    <div class="flex flex-wrap items-center gap-1.5 sm:gap-2">
                        <span class="font-medium truncate">{ds.name.clone()}</span>
                        <Badge variant=BadgeVariant::Outline>
                            {ds.type_display_name.clone()}
                        </Badge>
                        {ds.is_sample.then(|| view! {
                            <Badge variant=BadgeVariant::Secondary class="text-xs">
                                "Sample"
                            </Badge>
                        })}
                        {cred_badge.map(|(text, variant)| view! {
                            <Badge variant=variant class="text-xs">
                                {text}
                            </Badge>
                        })}
                        {show_catalog_warning.then(|| view! {
                            <span class="h-4 w-4 text-warning-foreground inline-flex items-center justify-center" title="Catalog needs attention">
                                <Icon icon=icondata_lu::LuTriangleAlert/>
                            </span>
                        })}
                    </div>
                    {(!ds.slug.is_empty()).then(|| view! {
                        <p class="text-xs text-muted-foreground font-mono truncate">
                            {ds.slug.clone()}
                        </p>
                    })}
                </div>
            </div>

            // Right side: toggle, settings, delete
            <div class="flex items-center gap-2 sm:gap-3 flex-wrap sm:flex-nowrap">
                // User enable/disable toggle
                <div class="flex items-center gap-2">
                    <span class="text-xs text-muted-foreground hidden sm:inline">
                        {toggle_label}
                    </span>
                    <Switch
                        checked=Signal::from(local_enabled)
                        on_change=on_toggle
                        disabled=switch_disabled
                        class=if !can_enable { "opacity-50 cursor-not-allowed".to_string() } else { String::new() }
                    />
                </div>

                // Settings button — links to React app
                <a
                    href=format!("/settings/datasources?open={}", ds.slug)
                    class="inline-flex items-center justify-center gap-2 whitespace-nowrap rounded-md text-sm font-medium transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring h-8 rounded-md px-3 text-xs border border-input bg-background text-foreground shadow-sm hover:bg-accent hover:text-accent-foreground"
                >
                    <span class="h-4 w-4 inline-flex items-center justify-center">
                        <Icon icon=icondata_lu::LuSettings/>
                    </span>
                    <span class="hidden sm:inline">"Settings"</span>
                </a>

                // Delete button
                <button
                    class="inline-flex items-center justify-center gap-2 whitespace-nowrap rounded-md text-sm font-medium transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring h-8 rounded-md px-3 text-xs text-foreground hover:bg-accent hover:text-accent-foreground"
                    on:click=on_delete_click
                >
                    <span class="h-4 w-4 text-error-foreground inline-flex items-center justify-center">
                        <Icon icon=icondata_lu::LuTrash2/>
                    </span>
                </button>
            </div>
        </div>
    }
}
