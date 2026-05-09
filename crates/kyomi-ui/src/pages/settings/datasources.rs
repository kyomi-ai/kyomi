// SPDX-License-Identifier: AGPL-3.0-or-later

//! Data Sources settings page — list, toggle, delete, create, and edit datasources.
//!
//! Replaces `apps/frontend/src/components/settings/DatasourceSettings.jsx` and
//! `apps/frontend/src/components/settings/DatasourceModal.jsx`.

use leptos::prelude::*;
use phosphor_leptos::{Icon, IconWeight};
use crate::components::{
    Alert, AlertDescription, AlertTitle, AlertVariant, Badge, BadgeVariant, Button, ButtonLink,
    ButtonSize, ButtonVariant, Card, ConfirmDialog, EmptyState, Modal, ModalSize, Skeleton, Switch,
};
use crate::components::DynSelect;
use crate::pages::connect_setup::CONNECT_TYPES;
use crate::pages::settings::connect_deployment::{
    CopyButton, DeploymentCommands, DeploymentTabStrip, build_deployment_commands, default_port,
};
use crate::pages::settings::connect_status_panel::ConnectStatusPanel;
use crate::query_cache::{use_query, QueryCache};
use crate::server_fns::connect::create_connect_datasource;
use crate::server_fns::datasources::*;
use crate::server_fns::sql_editor::refresh_catalog;
use crate::server_fns::onboarding::{
    check_sample_datasource_available, create_sample_datasource,
};

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

/// Generate a slug from a datasource name — matches React `generateSlug`.
fn generate_slug(name: &str) -> String {
    name.to_lowercase()
        .replace(|c: char| c.is_whitespace() || c == '_', "-")
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

/// Returns the `connection_config` key that holds the catalog scope list
/// for a given datasource type.
fn catalog_config_key_for_type(ds_type: &str) -> &'static str {
    match ds_type {
        "bigquery" => "catalog_projects",
        "clickhouse" | "mysql" | "snowflake" => "catalog_databases",
        "databricks" => "catalog_catalogs",
        _ => "catalog_schemas",
    }
}

/// Returns the human-readable label for the catalog scope items for a given
/// datasource type.
fn catalog_item_label_for_type(ds_type: &str) -> &'static str {
    match ds_type {
        "bigquery" => "projects",
        "clickhouse" | "mysql" | "snowflake" => "databases",
        "databricks" => "catalogs",
        _ => "schemas",
    }
}

/// Returns the key within the `DiscoverResourcesResult.resources` map that
/// holds the items relevant to catalog scope selection for a given datasource
/// type.  Matches the pairs emitted by `discover_datasource_resources` in
/// `server_fns/datasources.rs`.
fn discovery_resource_key_for_type(ds_type: &str) -> &'static str {
    match ds_type {
        // postgres / redshift / sqlserver / synapse: catalog scope = schemas
        "postgres" | "redshift" | "sqlserver" | "synapse" => "schemas",
        // databricks: catalog scope = catalogs
        "databricks" => "catalogs",
        // bigquery: no discovery support (text input only — requires auth flow)
        // clickhouse / mysql / snowflake: catalog scope = databases
        _ => "databases",
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Main Page
// ─────────────────────────────────────────────────────────────────────────────

/// Data Sources settings page content.
#[component]
pub fn DatasourcesPage() -> impl IntoView {
    // Layout-level QueryCache — cached across navigation and invalidated by
    // `datasource_update` WS events so other tabs (and other workspace
    // members) see create/update/delete mutations without a manual refresh.
    let datasources_signal =
        use_query("datasources", || (), |_: ()| list_datasources());

    view! {
        {move || {
            match datasources_signal.get() {
                None => view! { <DatasourcesLoadingSkeleton/> }.into_any(),
                Some(Ok(datasources)) => view! {
                    <DatasourcesContent
                        initial_datasources=datasources
                        datasources_signal=datasources_signal
                    />
                }.into_any(),
                Some(Err(e)) => view! {
                    <div class="p-4 sm:p-6 space-y-6">
                        <div>
                            <h2 class="text-xl font-display text-foreground">"Datasources"</h2>
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
        }}
    }
}

/// Loading skeleton shown while data is being fetched.
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
    datasources_signal: Signal<Option<Result<Vec<DatasourceInfo>, ServerFnError>>>,
) -> impl IntoView {
    let (datasources, set_datasources) = signal(initial_datasources);
    let query_cache = expect_context::<QueryCache>();

    // ── Modal state ──────────────────────────────────────────────────────
    // None = closed, Some(None) = create mode, Some(Some(id)) = edit mode
    let (modal_datasource_id, set_modal_datasource_id) =
        signal::<Option<Option<String>>>(None);

    let modal_open = Signal::derive(move || modal_datasource_id.get().is_some());

    let on_modal_close = Callback::new(move |()| {
        set_modal_datasource_id.set(None);
    });

    let on_datasource_saved = Callback::new(move |_result: DatasourceResult| {
        set_modal_datasource_id.set(None);
        // Refresh the datasource list — the cached signal will refetch in
        // the background and reseed `datasources` via the Effect below.
        query_cache.invalidate("datasources");
    });

    // When the shared cache refreshes (WS invalidation or explicit
    // invalidate), sync the local optimistic list.
    Effect::new(move |_| {
        if let Some(Ok(list)) = datasources_signal.get() {
            set_datasources.set(list);
        }
    });

    // ── Delete state ────────────────────────────────────────────────────
    let (delete_dialog_open, set_delete_dialog_open) = signal(false);
    let (datasource_to_delete, set_datasource_to_delete) =
        signal::<Option<DatasourceInfo>>(None);

    let delete_ds_action = Action::new(|ds_id: &String| {
        let ds_id = ds_id.clone();
        async move { delete_datasource(ds_id).await }
    });

    Effect::new(move |_| {
        if let Some(result) = delete_ds_action.value().get() {
            match result {
                Ok(()) => {
                    if let Some(ds) = datasource_to_delete.get_untracked() {
                        set_datasources.update(|list| {
                            list.retain(|d| d.id != ds.id);
                        });
                    }
                    set_datasource_to_delete.set(None);
                }
                Err(e) => {
                    leptos::logging::error!("Failed to delete datasource: {e}");
                    set_datasource_to_delete.set(None);
                }
            }
        }
    });

    let on_delete_confirm = Callback::new(move |()| {
        set_delete_dialog_open.set(false);
        if let Some(ds) = datasource_to_delete.get_untracked() {
            delete_ds_action.dispatch(ds.id.clone());
        }
    });

    let on_delete_cancel = Callback::new(move |()| {
        set_delete_dialog_open.set(false);
        set_datasource_to_delete.set(None);
    });

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
                    <h2 class="text-xl font-display text-foreground">"Datasources"</h2>
                    <p class="text-sm text-muted-foreground">
                        "Manage database connections"
                    </p>
                </div>
                // Header CTA — only shown when at least one datasource exists.
                // Empty state renders its own prominent CTA below (see `EmptyState`),
                // so double-showing the button creates a duplicate "Add Datasource" CTA.
                <Show when=move || !datasources.get().is_empty()>
                    <Button
                        on:click=move |_| set_modal_datasource_id.set(Some(None))
                    >
                        <span class="h-4 w-4 inline-flex items-center justify-center">
                            <Icon icon=phosphor_leptos::PLUS/>
                        </span>
                        "Add Datasource"
                    </Button>
                </Show>
            </div>

            // Datasources List
            <Card>
                <div class="p-0">
                    <Show
                        when=move || !datasources.get().is_empty()
                        fallback=move || view! {
                            <EmptyState
                                icon=std::sync::Arc::new(|| view! {
                                    <Icon icon=phosphor_leptos::DATABASE weight=IconWeight::Duotone size="64px"/>
                                }.into_any())
                                title="No datasources configured"
                                description="Connect a data source to start querying your data"
                                action=std::sync::Arc::new(move || view! {
                                    <Button on:click=move |_| set_modal_datasource_id.set(Some(None))>
                                        <span class="h-4 w-4 inline-flex items-center justify-center">
                                            <Icon icon=phosphor_leptos::PLUS/>
                                        </span>
                                        "Add Datasource"
                                    </Button>
                                }.into_any())
                            />
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
                                    set_modal_datasource_id=set_modal_datasource_id
                                />
                            </For>
                        </div>
                    </Show>
                </div>
            </Card>
        </div>

        // Datasource Modal (create and edit)
        <DatasourceModal
            open=modal_open
            datasource_id=Signal::derive(move || {
                modal_datasource_id.get().flatten()
            })
            on_close=on_modal_close
            on_saved=on_datasource_saved
        />

        // Delete Confirmation Dialog
        {move || view! {
            <ConfirmDialog
                open=Signal::from(delete_dialog_open)
                title=delete_title.clone()
                message=delete_message()
                confirm_text="Delete"
                on_confirm=on_delete_confirm
                on_cancel=on_delete_cancel
            />
        }}
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Datasource Row
// ─────────────────────────────────────────────────────────────────────────────

/// A single datasource row in the list.
#[component]
fn DatasourceRow(
    ds: DatasourceInfo,
    set_datasources: WriteSignal<Vec<DatasourceInfo>>,
    set_delete_dialog_open: WriteSignal<bool>,
    set_datasource_to_delete: WriteSignal<Option<DatasourceInfo>>,
    set_modal_datasource_id: WriteSignal<Option<Option<String>>>,
) -> impl IntoView {
    // ── Toggle state ────────────────────────────────────────────────────
    let ds_for_toggle = ds.clone();
    let (local_enabled, set_local_enabled) = signal(ds.user_enabled);

    let can_enable = ds.can_enable;

    let toggle_action = Action::new(|(ds_id, new_val): &(String, bool)| {
        let ds_id = ds_id.clone();
        let new_val = *new_val;
        async move { toggle_datasource(ds_id, new_val).await.map(|()| new_val) }
    });

    let ds_id_for_effect = ds_for_toggle.id.clone();
    Effect::new(move |_| {
        if let Some(result) = toggle_action.value().get() {
            match result {
                Ok(new_val) => {
                    let id = ds_id_for_effect.clone();
                    set_datasources.update(|list| {
                        if let Some(d) = list.iter_mut().find(|d| d.id == id) {
                            d.user_enabled = new_val;
                        }
                    });
                }
                Err(e) => {
                    set_local_enabled.update(|v| *v = !*v);
                    leptos::logging::error!("Failed to toggle datasource: {e}");
                }
            }
        }
    });

    let switch_disabled = !can_enable;

    let on_toggle = Callback::new(move |new_val: bool| {
        if toggle_action.pending().get_untracked() {
            return;
        }
        let ds_id = ds_for_toggle.id.clone();
        set_local_enabled.set(new_val);
        toggle_action.dispatch((ds_id, new_val));
    });

    // ── Delete handler ──────────────────────────────────────────────────
    let ds_for_delete = ds.clone();
    let on_delete_click = move |_: leptos::ev::MouseEvent| {
        set_datasource_to_delete.set(Some(ds_for_delete.clone()));
        set_delete_dialog_open.set(true);
    };

    // ── Settings handler ─────────────────────────────────────────────────
    let ds_id_for_settings = ds.id.clone();
    let on_settings_click = move |_: leptos::ev::MouseEvent| {
        set_modal_datasource_id.set(Some(Some(ds_id_for_settings.clone())));
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
        <div class="flex flex-col sm:flex-row sm:items-center sm:justify-between p-4 gap-3 hover:bg-muted/50 transition-colors">
            // Left side: name, type badge, status badges
            <div class="flex items-center gap-3 min-w-0">
                <span class="h-6 w-6 shrink-0 text-muted-foreground inline-flex items-center justify-center">
                    <Icon icon=phosphor_leptos::DATABASE/>
                </span>
                <div class="min-w-0">
                    <div class="flex flex-wrap items-center gap-1.5 sm:gap-2">
                        <span class="font-medium truncate">{ds.name.clone()}</span>
                        <span aria-hidden="true" class="text-muted-foreground">"·"</span>
                        <span class="text-sm text-muted-foreground">{ds.type_display_name.clone()}</span>
                        {(ds.connection_type == "connect").then(|| view! {
                            <Badge variant=BadgeVariant::Secondary class="text-xs">
                                "Connect"
                            </Badge>
                        })}
                        {ds.is_sample.then(|| view! {
                            <Badge variant=BadgeVariant::Secondary class="text-xs">
                                "Sample"
                            </Badge>
                        })}
                        {ds.is_analytics.then(|| view! {
                            <Badge variant=BadgeVariant::Secondary class="text-xs">
                                <span class="inline-flex items-center gap-1">
                                    <Icon icon=phosphor_leptos::PULSE size="12px"/>
                                    "Analytics"
                                </span>
                            </Badge>
                        })}
                        {cred_badge.map(|(text, variant)| view! {
                            <Badge variant=variant class="text-xs">
                                {text}
                            </Badge>
                        })}
                        {show_catalog_warning.then(|| view! {
                            <span class="h-4 w-4 text-warning-foreground inline-flex items-center justify-center" title="Catalog needs attention">
                                <Icon icon=phosphor_leptos::WARNING/>
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

                // Settings button — opens modal (or analytics settings link for analytics datasources)
                {if ds.is_analytics {
                    view! {
                        <ButtonLink
                            href="/settings/analytics"
                            variant=ButtonVariant::Outline
                            size=ButtonSize::Sm
                        >
                            <span class="h-4 w-4 sm:mr-1 inline-flex items-center justify-center">
                                <Icon icon=phosphor_leptos::PULSE/>
                            </span>
                            <span class="hidden sm:inline">"Analytics Settings"</span>
                        </ButtonLink>
                    }.into_any()
                } else {
                    view! {
                        <Button
                            variant=ButtonVariant::Outline
                            size=ButtonSize::Sm
                            on:click=on_settings_click
                        >
                            <span class="h-4 w-4 sm:mr-1 inline-flex items-center justify-center">
                                <Icon icon=phosphor_leptos::GEAR/>
                            </span>
                            <span class="hidden sm:inline">"Settings"</span>
                        </Button>
                    }.into_any()
                }}

                // Delete button — hidden for analytics datasources (lifecycle-managed by analytics site CRUD)
                {(!ds.is_analytics).then(|| view! {
                    <Button
                        variant=ButtonVariant::GhostDestructive
                        size=ButtonSize::Icon
                        on:click=on_delete_click
                    >
                        <Icon icon=phosphor_leptos::TRASH/>
                    </Button>
                })}
            </div>
        </div>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Datasource Modal
// ─────────────────────────────────────────────────────────────────────────────

/// CSS class for text input fields — matches the React modal input style.
const MODAL_INPUT_CLASS: &str =
    "w-full px-3 py-2 border border-input rounded-md bg-background text-sm text-foreground focus:outline-none focus:ring-1 focus:ring-ring";

/// Tab button active class.
const TAB_ACTIVE: &str =
    "px-4 py-2 text-sm font-medium border-b-2 -mb-px transition-colors border-primary text-primary";

/// Tab button inactive class.
const TAB_INACTIVE: &str =
    "px-4 py-2 text-sm font-medium border-b-2 -mb-px transition-colors border-transparent text-muted-foreground hover:text-foreground";

/// Tab button disabled class.
const TAB_DISABLED: &str =
    "px-4 py-2 text-sm font-medium border-b-2 -mb-px transition-colors border-transparent text-muted-foreground opacity-50 cursor-not-allowed";

/// Datasource types for the create-mode type selector.
/// Matches the values from `get_datasource_types()`.
const PROVIDER_TYPES: &[(&str, &str)] = &[
    ("bigquery", "BigQuery"),
    ("postgres", "PostgreSQL"),
    ("mysql", "MySQL"),
    ("clickhouse", "ClickHouse"),
    ("snowflake", "Snowflake"),
    ("databricks", "Databricks"),
    ("sqlserver", "SQL Server"),
];

/// Modal state for create/edit mode.
///
/// Matches `apps/frontend/src/components/settings/DatasourceModal.jsx`.
#[component]
pub fn DatasourceModal(
    /// Whether the modal is open.
    open: Signal<bool>,
    /// None = create mode, Some(id) = edit mode.
    datasource_id: Signal<Option<String>>,
    /// Called when modal is closed.
    on_close: Callback<()>,
    /// Called when a datasource was successfully saved.
    on_saved: Callback<DatasourceResult>,
) -> impl IntoView {
    // ── Form state ───────────────────────────────────────────────────────
    let (name, set_name) = signal(String::new());
    let (slug, set_slug) = signal(String::new());
    let (slug_manually_edited, set_slug_manually_edited) = signal(false);
    let (ds_type, set_ds_type) = signal("bigquery".to_string());

    // connection_config fields (stored individually so Leptos reactivity works)
    // host, port, ssl_mode, database, schema, warehouse, account, role,
    // catalog, server_hostname, http_path, secure, encrypt, trust_server_certificate
    // bigquery: auth_mode, oauth_client_id, oauth_client_secret, service_account_json
    // snowflake: auth_mode
    let (cfg_host, set_cfg_host) = signal(String::new());
    let (cfg_port, set_cfg_port) = signal(String::new());
    let (cfg_ssl_mode, set_cfg_ssl_mode) = signal("require".to_string());
    let (cfg_database, set_cfg_database) = signal(String::new());
    let (cfg_schema, set_cfg_schema) = signal(String::new());
    let (cfg_warehouse, set_cfg_warehouse) = signal(String::new());
    let (cfg_account, set_cfg_account) = signal(String::new());
    let (cfg_role, set_cfg_role) = signal(String::new());
    let (cfg_catalog, set_cfg_catalog) = signal(String::new());
    let (cfg_server_hostname, set_cfg_server_hostname) = signal(String::new());
    let (cfg_http_path, set_cfg_http_path) = signal(String::new());
    let (cfg_secure, set_cfg_secure) = signal(false);
    let (cfg_encrypt, set_cfg_encrypt) = signal(true);
    let (cfg_trust_cert, set_cfg_trust_cert) = signal(false);
    let (cfg_shared_credentials, set_cfg_shared_credentials) = signal(false);

    // BigQuery-specific
    let (bq_auth_mode, set_bq_auth_mode) = signal("kyomi_oauth".to_string());
    let (cfg_oauth_client_id, set_cfg_oauth_client_id) = signal(String::new());
    let (cfg_oauth_client_secret, set_cfg_oauth_client_secret) = signal(String::new());
    let (cfg_service_account_json, set_cfg_service_account_json) = signal(String::new());
    let (service_account_email, set_service_account_email) = signal(String::new());

    // Snowflake-specific
    let (sf_auth_mode, set_sf_auth_mode) = signal("password".to_string());

    // Credentials form
    let (cred_username, set_cred_username) = signal(String::new());
    let (cred_password, set_cred_password) = signal(String::new());
    let (cred_access_token, set_cred_access_token) = signal(String::new());
    let (cred_private_key, set_cred_private_key) = signal(String::new());
    let (cred_billing_project, set_cred_billing_project) = signal(String::new());
    let (cred_default_project, set_cred_default_project) = signal(String::new());

    // ── Tab state ────────────────────────────────────────────────────────
    // "connection" or "catalog"
    let (active_tab, set_active_tab) = signal("connection".to_string());

    // ── Operation state ──────────────────────────────────────────────────
    let (test_result, set_test_result) = signal::<Option<TestConnectionResult>>(None);
    let (settings_loading, set_settings_loading) = signal(false);
    let (error_msg, set_error_msg) = signal::<Option<String>>(None);

    // ── Connect datasource state ─────────────────────────────────────────
    // `connection_type` is `"direct"` for standard provider datasources and
    // `"connect"` for Kyomi Connect agent datasources. In edit mode we branch
    // on this to swap the connection/auth form for `ConnectStatusPanel`.
    // Used by BOTH modes: loaded from `settings.connection_type` in edit mode,
    // driven by the create-mode Direct / Kyomi Connect toggle in create mode.
    let (connection_type, set_connection_type) = signal::<String>("direct".to_string());

    // ── Create-mode Connect flow state ───────────────────────────────────
    // `connect_token`: populated after `create_connect_datasource` succeeds;
    // presence of a token swaps the modal body to the post-create view
    // (token display + deployment tabs + Done button).
    // `connect_created_name` / `connect_created_type`: snapshot of the
    // datasource we just created (so we can render its name in the post-
    // create header and its default port in the deployment commands).
    // `creating_connect`: in-flight flag for the create server_fn — disables
    // the submit button and swaps its label for "Creating...".
    // `active_deploy_tab`: which of the four deployment tabs is active in
    // the post-create view. Default `"linux"` (matches `ConnectStatusPanel`).
    let (connect_token, set_connect_token) = signal::<Option<String>>(None);
    let (connect_created_name, set_connect_created_name) = signal::<String>(String::new());
    let (connect_created_type, set_connect_created_type) = signal::<String>(String::new());
    let (creating_connect, set_creating_connect) = signal(false);
    let (active_deploy_tab, set_active_deploy_tab) = signal::<String>("linux".to_string());

    // ── Sample datasource state ──────────────────────────────────────────
    // `is_sample`: true when editing an existing sample datasource (loaded from
    // connection_config.is_sample). Gates the read-only sample view.
    // `sample_available`: sample ClickHouse is configured on the server.
    // `sample_already_added`: current workspace already has a sample datasource.
    // `creating_sample`: in-flight POST for quick-add.
    let (is_sample, set_is_sample) = signal(false);
    let (sample_available, set_sample_available) = signal(false);
    let (sample_already_added, set_sample_already_added) = signal(false);
    let (creating_sample, set_creating_sample) = signal(false);

    // ── Discovery state ──────────────────────────────────────────────────
    // "idle", "loading", "success", "error"
    let (discovery_status, set_discovery_status) = signal("idle".to_string());
    let (discovery_error, set_discovery_error) = signal::<Option<String>>(None);
    let (discovered_databases, set_discovered_databases) = signal::<Vec<String>>(vec![]);
    let (discovered_schemas, set_discovered_schemas) = signal::<Vec<String>>(vec![]);
    let (discovered_warehouses, set_discovered_warehouses) = signal::<Vec<String>>(vec![]);
    let (discovered_catalogs, set_discovered_catalogs) = signal::<Vec<String>>(vec![]);

    // ── Catalog tab state (edit mode) ────────────────────────────────────
    // Selected catalog scope items (projects / databases / schemas / catalogs).
    // Stored at modal level so `build_connection_config` can include them when
    // saving from the Connection tab after the user configures them on the
    // Catalog tab.
    let (catalog_selected, set_catalog_selected) = signal::<Vec<String>>(vec![]);
    // BigQuery-specific: whether to include public datasets in catalog indexing.
    let (bq_include_public, set_bq_include_public) = signal(false);

    // ── Reset form ───────────────────────────────────────────────────────
    let reset_form = move || {
        set_name.set(String::new());
        set_slug.set(String::new());
        set_slug_manually_edited.set(false);
        set_ds_type.set("bigquery".to_string());
        set_cfg_host.set(String::new());
        set_cfg_port.set(String::new());
        set_cfg_ssl_mode.set("require".to_string());
        set_cfg_database.set(String::new());
        set_cfg_schema.set(String::new());
        set_cfg_warehouse.set(String::new());
        set_cfg_account.set(String::new());
        set_cfg_role.set(String::new());
        set_cfg_catalog.set(String::new());
        set_cfg_server_hostname.set(String::new());
        set_cfg_http_path.set(String::new());
        set_cfg_secure.set(false);
        set_cfg_encrypt.set(true);
        set_cfg_trust_cert.set(false);
        set_cfg_shared_credentials.set(false);
        set_bq_auth_mode.set("kyomi_oauth".to_string());
        set_cfg_oauth_client_id.set(String::new());
        set_cfg_oauth_client_secret.set(String::new());
        set_cfg_service_account_json.set(String::new());
        set_service_account_email.set(String::new());
        set_sf_auth_mode.set("password".to_string());
        set_cred_username.set(String::new());
        set_cred_password.set(String::new());
        set_cred_access_token.set(String::new());
        set_cred_private_key.set(String::new());
        set_cred_billing_project.set(String::new());
        set_cred_default_project.set(String::new());
        set_active_tab.set("connection".to_string());
        set_test_result.set(None);
        set_error_msg.set(None);
        set_discovery_status.set("idle".to_string());
        set_discovery_error.set(None);
        set_discovered_databases.set(vec![]);
        set_discovered_schemas.set(vec![]);
        set_discovered_warehouses.set(vec![]);
        set_discovered_catalogs.set(vec![]);
        set_is_sample.set(false);
        set_connection_type.set("direct".to_string());
        set_connect_token.set(None);
        set_connect_created_name.set(String::new());
        set_connect_created_type.set(String::new());
        set_creating_connect.set(false);
        set_active_deploy_tab.set("linux".to_string());
        // Don't reset sample_available / sample_already_added here — those are
        // refreshed by an async effect when the modal opens in create mode.
        set_creating_sample.set(false);
        set_catalog_selected.set(vec![]);
        set_bq_include_public.set(false);
    };

    // ── Load settings when switching to edit mode ─────────────────────────
    Effect::new(move |_| {
        if !open.get() {
            return;
        }
        let id = datasource_id.get();
        match id {
            None => {
                // Create mode — reset form
                reset_form();
            }
            Some(ds_id) => {
                // Edit mode — load settings
                set_settings_loading.set(true);
                set_active_tab.set("connection".to_string());
                set_test_result.set(None);
                set_error_msg.set(None);
                set_discovery_status.set("idle".to_string());

                leptos::task::spawn_local(async move {
                    match get_datasource_settings(ds_id).await {
                        Ok(settings) => {
                            set_name.try_set(settings.name.clone());
                            set_slug.try_set(settings.slug.clone());
                            set_ds_type.try_set(settings.datasource_type.clone());
                            set_connection_type.try_set(settings.connection_type.clone());

                            // Load connection_config fields
                            let cfg = &settings.connection_config;
                            let str_val = |key: &str| -> String {
                                cfg.get(key)
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("")
                                    .to_string()
                            };
                            let bool_val = |key: &str| -> bool {
                                cfg.get(key)
                                    .and_then(|v| v.as_bool())
                                    .unwrap_or(false)
                            };
                            set_cfg_host.try_set(str_val("host"));
                            set_cfg_port.try_set(
                                cfg.get("port")
                                    .and_then(|v| v.as_i64())
                                    .map(|n| n.to_string())
                                    .unwrap_or_default(),
                            );
                            set_cfg_ssl_mode.try_set(
                                if str_val("ssl_mode").is_empty() {
                                    "require".to_string()
                                } else {
                                    str_val("ssl_mode")
                                }
                            );
                            set_cfg_database.try_set(str_val("database"));
                            set_cfg_schema.try_set(str_val("schema"));
                            set_cfg_warehouse.try_set(str_val("warehouse"));
                            set_cfg_account.try_set(str_val("account"));
                            set_cfg_role.try_set(str_val("role"));
                            set_cfg_catalog.try_set(str_val("catalog"));
                            set_cfg_server_hostname.try_set(str_val("server_hostname"));
                            set_cfg_http_path.try_set(str_val("http_path"));
                            set_cfg_secure.try_set(bool_val("secure"));
                            set_cfg_encrypt.try_set(
                                cfg.get("encrypt")
                                    .and_then(|v| v.as_bool())
                                    .unwrap_or(true),
                            );
                            set_cfg_trust_cert.try_set(bool_val("trust_server_certificate"));
                            set_is_sample.try_set(bool_val("is_sample"));
                            set_cfg_shared_credentials.try_set(settings.shared_credentials);
                            set_cfg_oauth_client_id.try_set(str_val("oauth_client_id"));
                            set_cfg_oauth_client_secret.try_set(str_val("oauth_client_secret"));

                            // BigQuery auth mode
                            if let Some(ref auth_mode) = settings.auth_mode {
                                set_bq_auth_mode.try_set(auth_mode.clone());
                                set_sf_auth_mode.try_set(auth_mode.clone());
                            }

                            // Service account
                            if let Some(ref email) = settings.service_account_email {
                                set_service_account_email.try_set(email.clone());
                            }

                            // Load catalog scope selections from connection_config
                            let catalog_key =
                                catalog_config_key_for_type(&settings.datasource_type);
                            let selected_items: Vec<String> = cfg
                                .get(catalog_key)
                                .and_then(|v| v.as_array())
                                .map(|arr| {
                                    arr.iter()
                                        .filter_map(|v| v.as_str().map(|s| s.to_string()))
                                        .collect()
                                })
                                .unwrap_or_default();
                            set_catalog_selected.try_set(selected_items);
                            let include_public = cfg
                                .get("include_public_datasets")
                                .and_then(|v| v.as_bool())
                                .unwrap_or(false);
                            set_bq_include_public.try_set(include_public);

                            // Load user settings (masked credentials)
                            let user = &settings.user_settings;
                            let user_str = |key: &str| -> String {
                                user.get(key)
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("")
                                    .to_string()
                            };
                            set_cred_billing_project.try_set(user_str("billing_project"));
                            set_cred_default_project.try_set(user_str("default_project"));
                            // Note: passwords are not pre-filled (security)
                        }
                        Err(e) => {
                            set_error_msg.try_set(Some(format!("Failed to load settings: {e}")));
                        }
                    }
                    set_settings_loading.try_set(false);
                });
            }
        }
    });

    // ── Sample availability check (create mode) ─────────────────────────
    // Matches the React `checkSample` effect in DatasourceModal.jsx — fetches
    // `/api/v1/datasources/sample/available` when the modal opens in create mode.
    Effect::new(move |_| {
        if !open.get() {
            return;
        }
        if datasource_id.get().is_some() {
            // Edit mode — sample tile is not shown.
            return;
        }
        leptos::task::spawn_local(async move {
            match check_sample_datasource_available().await {
                Ok(result) => {
                    set_sample_available.try_set(result.configured && result.is_admin);
                    set_sample_already_added.try_set(result.already_added);
                }
                Err(_) => {
                    set_sample_available.try_set(false);
                    set_sample_already_added.try_set(false);
                }
            }
        });
    });

    // ── Sample quick-add handler ─────────────────────────────────────────
    // Matches React `handleCreateSample` — creates the sample datasource and
    // closes the modal, then refreshes the datasource list via on_saved.
    let do_create_sample = move || {
        set_creating_sample.set(true);
        set_error_msg.set(None);
        leptos::task::spawn_local(async move {
            match create_sample_datasource().await {
                Ok(()) => {
                    // Signal success — DatasourcesContent refreshes the list.
                    on_saved.run(DatasourceResult {
                        id: String::new(),
                        slug: "acme-analytics-sample".to_string(),
                        name: "Acme Analytics (Sample)".to_string(),
                        datasource_type: "clickhouse".to_string(),
                    });
                }
                Err(e) => {
                    let msg = e.to_string();
                    if msg.contains("already") {
                        set_sample_already_added.try_set(true);
                    }
                    set_error_msg.try_set(Some(msg));
                }
            }
            set_creating_sample.try_set(false);
        });
    };

    // ── Name change handler ──────────────────────────────────────────────
    let handle_name_change = move |new_name: String| {
        if !slug_manually_edited.get_untracked() {
            set_slug.set(generate_slug(&new_name));
        }
        set_name.set(new_name);
    };

    // ── Build connection_config JSON ─────────────────────────────────────
    let build_connection_config = move || -> serde_json::Value {
        let t = ds_type.get_untracked();
        let mut map = serde_json::Map::new();

        match t.as_str() {
            "postgres" | "mysql" | "redshift" => {
                if !cfg_host.get_untracked().is_empty() {
                    map.insert("host".to_string(), serde_json::json!(cfg_host.get_untracked()));
                }
                if let Ok(port) = cfg_port.get_untracked().parse::<i64>() {
                    map.insert("port".to_string(), serde_json::json!(port));
                }
                map.insert("ssl_mode".to_string(), serde_json::json!(cfg_ssl_mode.get_untracked()));
                if !cfg_database.get_untracked().is_empty() {
                    map.insert("database".to_string(), serde_json::json!(cfg_database.get_untracked()));
                }
                if !cfg_schema.get_untracked().is_empty() {
                    map.insert("schema".to_string(), serde_json::json!(cfg_schema.get_untracked()));
                }
            }
            "clickhouse" => {
                if !cfg_host.get_untracked().is_empty() {
                    map.insert("host".to_string(), serde_json::json!(cfg_host.get_untracked()));
                }
                if let Ok(port) = cfg_port.get_untracked().parse::<i64>() {
                    map.insert("port".to_string(), serde_json::json!(port));
                }
                map.insert("secure".to_string(), serde_json::json!(cfg_secure.get_untracked()));
                if !cfg_database.get_untracked().is_empty() {
                    map.insert("database".to_string(), serde_json::json!(cfg_database.get_untracked()));
                }
            }
            "snowflake" => {
                if !cfg_account.get_untracked().is_empty() {
                    map.insert("account".to_string(), serde_json::json!(cfg_account.get_untracked()));
                }
                map.insert("auth_mode".to_string(), serde_json::json!(sf_auth_mode.get_untracked()));
                if !cfg_warehouse.get_untracked().is_empty() {
                    map.insert("warehouse".to_string(), serde_json::json!(cfg_warehouse.get_untracked()));
                }
                if !cfg_database.get_untracked().is_empty() {
                    map.insert("database".to_string(), serde_json::json!(cfg_database.get_untracked()));
                }
                if !cfg_schema.get_untracked().is_empty() {
                    map.insert("schema".to_string(), serde_json::json!(cfg_schema.get_untracked()));
                }
                if !cfg_role.get_untracked().is_empty() {
                    map.insert("role".to_string(), serde_json::json!(cfg_role.get_untracked()));
                }
                if !cfg_oauth_client_id.get_untracked().is_empty() {
                    map.insert("oauth_client_id".to_string(), serde_json::json!(cfg_oauth_client_id.get_untracked()));
                }
                if !cfg_oauth_client_secret.get_untracked().is_empty() {
                    map.insert("oauth_client_secret".to_string(), serde_json::json!(cfg_oauth_client_secret.get_untracked()));
                }
            }
            "databricks" => {
                if !cfg_server_hostname.get_untracked().is_empty() {
                    map.insert("server_hostname".to_string(), serde_json::json!(cfg_server_hostname.get_untracked()));
                }
                if !cfg_http_path.get_untracked().is_empty() {
                    map.insert("http_path".to_string(), serde_json::json!(cfg_http_path.get_untracked()));
                }
                if !cfg_catalog.get_untracked().is_empty() {
                    map.insert("catalog".to_string(), serde_json::json!(cfg_catalog.get_untracked()));
                }
                if !cfg_schema.get_untracked().is_empty() {
                    map.insert("schema".to_string(), serde_json::json!(cfg_schema.get_untracked()));
                }
            }
            "sqlserver" | "synapse" => {
                if !cfg_host.get_untracked().is_empty() {
                    map.insert("host".to_string(), serde_json::json!(cfg_host.get_untracked()));
                }
                if let Ok(port) = cfg_port.get_untracked().parse::<i64>() {
                    map.insert("port".to_string(), serde_json::json!(port));
                }
                map.insert("encrypt".to_string(), serde_json::json!(cfg_encrypt.get_untracked()));
                map.insert("trust_server_certificate".to_string(), serde_json::json!(cfg_trust_cert.get_untracked()));
                if !cfg_database.get_untracked().is_empty() {
                    map.insert("database".to_string(), serde_json::json!(cfg_database.get_untracked()));
                }
                if !cfg_schema.get_untracked().is_empty() {
                    map.insert("schema".to_string(), serde_json::json!(cfg_schema.get_untracked()));
                }
            }
            "bigquery" => {
                map.insert("auth_mode".to_string(), serde_json::json!(bq_auth_mode.get_untracked()));
                if !cfg_oauth_client_id.get_untracked().is_empty() {
                    map.insert("oauth_client_id".to_string(), serde_json::json!(cfg_oauth_client_id.get_untracked()));
                }
                if !cfg_oauth_client_secret.get_untracked().is_empty() {
                    map.insert("oauth_client_secret".to_string(), serde_json::json!(cfg_oauth_client_secret.get_untracked()));
                }
                if !cfg_service_account_json.get_untracked().is_empty() {
                    map.insert("service_account_json".to_string(), serde_json::json!(cfg_service_account_json.get_untracked()));
                }
            }
            _ => {}
        }

        if cfg_shared_credentials.get_untracked() {
            map.insert("shared_credentials".to_string(), serde_json::json!(true));
        }

        // Catalog scope — only written in edit mode (non-empty selection).
        // In create mode these signals are always empty (reset_form clears them).
        let selected = catalog_selected.get_untracked();
        if !selected.is_empty() {
            let key = catalog_config_key_for_type(&t);
            map.insert(key.to_string(), serde_json::json!(selected));
        }
        if bq_include_public.get_untracked() {
            map.insert(
                "include_public_datasets".to_string(),
                serde_json::json!(true),
            );
        }

        serde_json::Value::Object(map)
    };

    // ── Build credentials JSON ────────────────────────────────────────────
    let build_credentials = move || -> serde_json::Value {
        let t = ds_type.get_untracked();
        let mut map = serde_json::Map::new();

        match t.as_str() {
            "databricks" => {
                if !cred_access_token.get_untracked().is_empty() {
                    map.insert("access_token".to_string(), serde_json::json!(cred_access_token.get_untracked()));
                }
            }
            "bigquery" => {
                let bq_mode = bq_auth_mode.get_untracked();
                match bq_mode.as_str() {
                    "kyomi_oauth" | "enterprise_oauth" => {
                        if !cred_billing_project.get_untracked().is_empty() {
                            map.insert("billing_project".to_string(), serde_json::json!(cred_billing_project.get_untracked()));
                        }
                        if !cred_default_project.get_untracked().is_empty() {
                            map.insert("default_project".to_string(), serde_json::json!(cred_default_project.get_untracked()));
                        }
                    }
                    _ => {}
                }
            }
            _ => {
                if !cred_username.get_untracked().is_empty() {
                    map.insert("username".to_string(), serde_json::json!(cred_username.get_untracked()));
                }
                if !cred_password.get_untracked().is_empty() {
                    map.insert("password".to_string(), serde_json::json!(cred_password.get_untracked()));
                }
                if !cred_private_key.get_untracked().is_empty() {
                    map.insert("private_key".to_string(), serde_json::json!(cred_private_key.get_untracked()));
                }
            }
        }

        serde_json::Value::Object(map)
    };

    // ── Test & Discover ──────────────────────────────────────────────────
    // Input: (ds_type, conn_cfg, creds, ds_id, slug_val)
    type TestDiscoverInput = (String, serde_json::Value, serde_json::Value, Option<String>, String);

    let test_action = Action::new(|input: &TestDiscoverInput| {
        let (ds_type_val, conn_cfg, creds, ds_id, slug_val) = input.clone();
        async move {
            // In edit mode, use the existing datasource ID for OAuth credential lookup.
            let ds_slug = ds_id
                .as_deref()
                .map(|_| slug_val)
                .filter(|s| !s.is_empty());
            discover_datasource_resources(ds_type_val, conn_cfg, creds, ds_slug).await
        }
    });

    Effect::new(move |_| {
        if let Some(result) = test_action.value().get() {
            match result {
                Ok(r) => {
                    if r.success {
                        set_test_result.set(Some(TestConnectionResult {
                            success: true,
                            message: "Connected successfully".to_string(),
                        }));
                        set_discovery_status.set("success".to_string());

                        if let Some(dbs) = r.resources.get("databases") {
                            set_discovered_databases.set(dbs.clone());
                        }
                        if let Some(schemas) = r.resources.get("schemas") {
                            set_discovered_schemas.set(schemas.clone());
                        }
                        if let Some(wh) = r.resources.get("warehouses") {
                            set_discovered_warehouses.set(wh.clone());
                        }
                        if let Some(cats) = r.resources.get("catalogs") {
                            set_discovered_catalogs.set(cats.clone());
                        }
                    } else {
                        set_test_result.set(Some(TestConnectionResult {
                            success: false,
                            message: r.message.clone(),
                        }));
                        set_discovery_status.set("error".to_string());
                        set_discovery_error.set(Some(r.message));
                    }
                }
                Err(e) => {
                    let msg = e.to_string();
                    set_test_result.set(Some(TestConnectionResult {
                        success: false,
                        message: msg.clone(),
                    }));
                    set_discovery_status.set("error".to_string());
                    set_discovery_error.set(Some(msg));
                }
            }
        }
    });

    let do_test_and_discover = move || {
        set_test_result.set(None);
        set_discovery_status.set("loading".to_string());
        set_discovery_error.set(None);
        set_discovered_databases.set(vec![]);
        set_discovered_schemas.set(vec![]);
        set_discovered_warehouses.set(vec![]);
        set_discovered_catalogs.set(vec![]);

        let ds_type_val = ds_type.get_untracked();
        let conn_cfg = build_connection_config();
        let creds = build_credentials();
        let ds_id = datasource_id.get_untracked();
        let slug_val = slug.get_untracked();

        test_action.dispatch((ds_type_val, conn_cfg, creds, ds_id, slug_val));
    };

    // ── Save ─────────────────────────────────────────────────────────────
    // Input: (ds_id, name, slug, conn_cfg, creds, ds_type)
    type SaveInput = (Option<String>, String, String, serde_json::Value, serde_json::Value, String);

    let save_action = Action::new(|input: &SaveInput| {
        let (ds_id, name_val, slug_val, conn_cfg, creds, ds_type_val) = input.clone();
        async move {
            match ds_id {
                None => {
                    // Create mode
                    create_datasource_modal(name_val, slug_val, ds_type_val, conn_cfg, creds).await
                }
                Some(id) => {
                    // Edit mode — save connection settings first
                    let update_result = update_datasource_settings(id.clone(), name_val, slug_val, conn_cfg).await;
                    match update_result {
                        Ok(r) => {
                            // Save credentials if any were entered
                            let creds_obj = creds.as_object().map(|o| !o.is_empty()).unwrap_or(false);
                            if creds_obj {
                                let _ = save_datasource_credentials(id, creds).await;
                            }
                            Ok(r)
                        }
                        Err(e) => Err(e),
                    }
                }
            }
        }
    });

    Effect::new(move |_| {
        if let Some(result) = save_action.value().get() {
            match result {
                Ok(saved) => {
                    on_saved.run(saved);
                }
                Err(e) => {
                    set_error_msg.set(Some(e.to_string()));
                }
            }
        }
    });

    let do_save = move || {
        let ds_id = datasource_id.get_untracked();
        let name_val = name.get_untracked();
        let slug_val = slug.get_untracked();
        let conn_cfg = build_connection_config();
        let creds = build_credentials();
        let ds_type_val = ds_type.get_untracked();

        if name_val.is_empty() {
            set_error_msg.set(Some("Name is required".to_string()));
            return;
        }

        set_error_msg.set(None);
        save_action.dispatch((ds_id, name_val, slug_val, conn_cfg, creds, ds_type_val));
    };

    // ── Create-mode: create a Connect datasource ─────────────────────────
    // Called from the create-mode footer when `connection_type == "connect"`.
    // On success, stashes the returned token + datasource name/type so the
    // modal body swaps to the post-create deployment view. The datasource
    // list refresh happens later, when the user clicks "Done" (which calls
    // on_saved, same as the direct create path's success callback).
    let do_create_connect = move || {
        let name_val = name.get_untracked().trim().to_string();
        let slug_val = slug.get_untracked();
        let ds_type_val = ds_type.get_untracked();

        if name_val.is_empty() {
            set_error_msg.set(Some("Name is required".to_string()));
            return;
        }

        set_creating_connect.set(true);
        set_error_msg.set(None);

        let slug_opt = if slug_val.trim().is_empty() {
            None
        } else {
            Some(slug_val)
        };

        leptos::task::spawn_local(async move {
            match create_connect_datasource(name_val.clone(), slug_opt, ds_type_val.clone()).await {
                Ok(result) => {
                    set_connect_created_name.try_set(name_val);
                    set_connect_created_type.try_set(ds_type_val);
                    set_connect_token.try_set(Some(result.connect_token));
                }
                Err(e) => {
                    set_error_msg.try_set(Some(e.to_string()));
                }
            }
            set_creating_connect.try_set(false);
        });
    };

    // ── Derived: is create mode ──────────────────────────────────────────
    let is_create_mode = Signal::derive(move || datasource_id.get().is_none());

    // ── Derived: is this a Kyomi Connect datasource? ─────────────────────
    // True in edit mode when the loaded datasource has `connection_type ==
    // "connect"`, and true in create mode when the user has picked "Kyomi
    // Connect" on the top-of-modal toggle.
    let is_connect = Signal::derive(move || connection_type.get() == "connect");

    // ── Derived: have we finished creating a Connect datasource? ─────────
    // When true, the modal body swaps to the post-create view (token +
    // deployment tabs + Done button). Cleared on modal close via reset_form.
    let connect_create_complete = Signal::derive(move || connect_token.get().is_some());

    // ── Modal title ──────────────────────────────────────────────────────
    let modal_title = Signal::derive(move || {
        if is_create_mode.get() {
            "Add Datasource".to_string()
        } else {
            format!("{} Settings", name.get())
        }
    });

    // ── Discovery section: show post-test fields ──────────────────────────
    let discovery_succeeded = Signal::derive(move || {
        discovery_status.get() == "success"
    });

    // Footer must be Arc<dyn Fn() -> AnyView + Send + Sync> (Leptos ChildrenFn).
    let do_save_for_footer = do_save;
    let footer: std::sync::Arc<dyn Fn() -> leptos::prelude::AnyView + Send + Sync> =
        std::sync::Arc::new(move || {
            let do_save = do_save_for_footer;
            view! {
                <Show
                    when=move || is_create_mode.get()
                    fallback=move || {
                        let do_save = do_save;
                        let is_saving = save_action.pending().get();
                        let sample = is_sample.get();
                        let connect = is_connect.get();
                        // Connect + sample both render as read-only in the
                        // body, so neither has a Save action. Label the
                        // dismiss button "Close" in both cases to signal the
                        // read-only nature.
                        let read_only = sample || connect;
                        view! {
                            // Edit mode footer.
                            //
                            // The sample datasource is intentionally read-only:
                            // its connection settings are managed by the server
                            // via `SAMPLE_CLICKHOUSE_*` env vars, so there is
                            // nothing for the user to save. Connect datasources
                            // are also read-only here — connection settings
                            // live on the remote agent, not in this modal. We
                            // surface a single "Close" button (no "Save") to
                            // make the read-only nature explicit, matching the
                            // React reference in `apps/frontend/src/components/
                            // settings/datasources/DatasourceModal.jsx` for
                            // both the sample and `connection_type === 'connect'`
                            // branches.
                            <Button
                                variant=ButtonVariant::Outline
                                on:click=move |_| on_close.run(())
                            >
                                {if read_only { "Close" } else { "Cancel" }}
                            </Button>
                            <Show when=move || !is_sample.get() && !is_connect.get()>
                                <Button
                                    disabled=is_saving
                                    on:click=move |_| do_save()
                                >
                                    {move || if save_action.pending().get() { "Saving..." } else { "Save" }}
                                </Button>
                            </Show>
                        }.into_any()
                    }
                >
                    // Create mode footer.
                    //
                    // Three distinct sub-modes, each with its own footer:
                    //   1. Direct create: Cancel + Next/Create (existing flow).
                    //   2. Connect create, pre-submit: Cancel + "Create Connect
                    //      Datasource" which calls `do_create_connect`.
                    //   3. Connect create, post-submit (token in hand): a
                    //      single "Done" button which calls `on_saved` —
                    //      same callback the direct-create success path uses,
                    //      so the modal closes and the datasource list
                    //      refreshes.
                    {move || {
                        let do_save = do_save;
                        let do_create_connect = do_create_connect;
                        // Post-create view (Connect create finished) — single Done button.
                        if connect_create_complete.get() {
                            return view! {
                                <Button
                                    on:click=move |_| {
                                        on_saved.run(DatasourceResult {
                                            id: String::new(),
                                            slug: String::new(),
                                            name: connect_created_name.get_untracked(),
                                            datasource_type: connect_created_type.get_untracked(),
                                        });
                                    }
                                >
                                    "Done"
                                </Button>
                            }.into_any();
                        }

                        // Pre-submit Connect create — single "Create Connect
                        // Datasource" action; no tab navigation.
                        if is_connect.get() {
                            return view! {
                                <Button
                                    variant=ButtonVariant::Outline
                                    on:click=move |_| on_close.run(())
                                >
                                    "Cancel"
                                </Button>
                                <Button
                                    disabled=Signal::derive(move || creating_connect.get() || name.get().trim().is_empty())
                                    on:click=move |_| do_create_connect()
                                >
                                    {move || if creating_connect.get() {
                                        "Creating..."
                                    } else {
                                        "Create Connect Datasource"
                                    }}
                                </Button>
                            }.into_any();
                        }

                        // Direct create footer (original behavior).
                        let is_connection_tab = active_tab.get() == "connection";
                        let can_next = test_result.get().map(|r| r.success).unwrap_or(false) && !name.get().is_empty();
                        let is_saving = save_action.pending().get();
                        view! {
                            <Button
                                variant=ButtonVariant::Outline
                                on:click=move |_| on_close.run(())
                            >
                                "Cancel"
                            </Button>
                            {if is_connection_tab {
                                view! {
                                    <Button
                                        disabled=!can_next
                                        on:click=move |_| set_active_tab.set("catalog".to_string())
                                    >
                                        "Next"
                                    </Button>
                                }.into_any()
                            } else {
                                view! {
                                    <Button
                                        disabled=is_saving
                                        on:click=move |_| do_save()
                                    >
                                        {move || if save_action.pending().get() { "Creating..." } else { "Create" }}
                                    </Button>
                                }.into_any()
                            }}
                        }.into_any()
                    }}
                </Show>
            }.into_any()
        });

    view! {
        <Modal
            show=open
            on_close=on_close
            title=modal_title
            size=ModalSize::Lg
            footer=footer.clone()
        >

            {move || {
                let do_test_and_discover = do_test_and_discover;
                view! {
                    // Error message
                    {move || error_msg.get().map(|msg| view! {
                        <Alert variant=AlertVariant::Error class="mb-4">
                            <AlertDescription>{msg}</AlertDescription>
                        </Alert>
                    })}

                    // Loading state (edit mode)
                    <Show when=move || settings_loading.get()>
                        <div class="flex items-center justify-center min-h-[400px]">
                            <span class="text-sm text-muted-foreground">"Loading settings..."</span>
                        </div>
                    </Show>

                    <Show when=move || !settings_loading.get()>
                        // ── Connection Method toggle (create mode only) ──
                        //
                        // Large two-column card selector. Follows the
                        // DESIGN.md "radio card" pattern used for selected /
                        // unselected choice cards: `rounded-lg border` with
                        // selected state switching to `border-primary
                        // bg-primary/5`. Hidden once the user has clicked
                        // "Create Connect Datasource" successfully — the
                        // post-create view below takes over the modal body.
                        <Show when=move || is_create_mode.get() && !connect_create_complete.get()>
                            <div class="mb-5">
                                <label class="block text-sm font-medium text-foreground mb-2">
                                    "Connection Method"
                                </label>
                                <div class="grid grid-cols-1 sm:grid-cols-2 gap-3">
                                    <button
                                        type="button"
                                        on:click=move |_| {
                                            set_connection_type.set("direct".to_string());
                                            set_error_msg.set(None);
                                        }
                                        class=move || {
                                            let selected = connection_type.get() == "direct";
                                            if selected {
                                                "p-4 rounded-lg border text-left transition-colors border-primary/20 bg-primary/10 text-primary"
                                            } else {
                                                "p-4 rounded-lg border text-left transition-colors border-border hover:border-muted-foreground/30 text-foreground"
                                            }
                                        }
                                    >
                                        <div class="font-medium text-sm">
                                            "Direct Connection"
                                        </div>
                                        <div class="text-xs text-muted-foreground mt-1">
                                            "Connect directly from Kyomi to your database."
                                        </div>
                                    </button>
                                    <button
                                        type="button"
                                        on:click=move |_| {
                                            set_connection_type.set("connect".to_string());
                                            set_error_msg.set(None);
                                            // If the user had picked a type
                                            // that isn't Connect-compatible
                                            // (e.g. BigQuery), snap to the
                                            // first Connect-compatible type
                                            // so the simplified form below
                                            // doesn't render with a disabled
                                            // placeholder value.
                                            let current = ds_type.get_untracked();
                                            let compatible = CONNECT_TYPES
                                                .iter()
                                                .any(|(v, _)| *v == current);
                                            if !compatible
                                                && let Some((v, _)) = CONNECT_TYPES.first()
                                            {
                                                set_ds_type.set((*v).to_string());
                                            }
                                        }
                                        class=move || {
                                            let selected = connection_type.get() == "connect";
                                            if selected {
                                                "p-4 rounded-lg border text-left transition-colors border-primary/20 bg-primary/10 text-primary"
                                            } else {
                                                "p-4 rounded-lg border text-left transition-colors border-border hover:border-muted-foreground/30 text-foreground"
                                            }
                                        }
                                    >
                                        <div class="font-medium text-sm">
                                            "Kyomi Connect"
                                        </div>
                                        <div class="text-xs text-muted-foreground mt-1">
                                            "Self-hosted agent — works through firewalls."
                                        </div>
                                    </button>
                                </div>
                            </div>
                        </Show>

                        // ── Post-create Connect view ──
                        //
                        // Rendered when the user has successfully created a
                        // Connect datasource in this session. Replaces the
                        // whole content area (tab bar + connection form) with
                        // a one-time token display + the four deployment
                        // command tabs. Footer provides a single "Done"
                        // button which closes the modal and refreshes the
                        // datasource list.
                        <Show when=move || connect_create_complete.get() && is_create_mode.get()>
                            <ConnectCreateSuccessView
                                token=Signal::derive(move || {
                                    connect_token
                                        .get()
                                        .expect("connect_token is set before the success view renders")
                                })
                                datasource_name=Signal::derive(move || connect_created_name.get())
                                datasource_type=Signal::derive(move || connect_created_type.get())
                                active_tab=active_deploy_tab
                                set_active_tab=set_active_deploy_tab
                            />
                        </Show>

                        // Edit-mode tab bar — Connection | Catalog.
                        // Hidden for sample and Connect datasources (both are
                        // read-only in this modal; no catalog management applies).
                        <Show when=move || {
                            !is_create_mode.get()
                                && !is_sample.get()
                                && !is_connect.get()
                        }>
                            <div class="flex border-b border-border mb-4">
                                <button
                                    class=move || if active_tab.get() == "connection" { TAB_ACTIVE } else { TAB_INACTIVE }
                                    on:click=move |_| set_active_tab.set("connection".to_string())
                                >
                                    "Connection"
                                </button>
                                <button
                                    class=move || if active_tab.get() == "catalog" { TAB_ACTIVE } else { TAB_INACTIVE }
                                    on:click=move |_| set_active_tab.set("catalog".to_string())
                                >
                                    "Catalog"
                                </button>
                            </div>
                        </Show>

                        // Tab bar — hidden in Connect create-mode (the simplified
                        // Connect form has no separate Catalog step; the catalog
                        // is indexed automatically after agent registration) and
                        // hidden after a successful Connect create (the post-
                        // create view owns the whole body).
                        <Show when=move || {
                            is_create_mode.get()
                                && !is_connect.get()
                                && !connect_create_complete.get()
                        }>
                            <div class="flex border-b border-border mb-4">
                                <button
                                    class=move || if active_tab.get() == "connection" { TAB_ACTIVE } else { TAB_INACTIVE }
                                    on:click=move |_| set_active_tab.set("connection".to_string())
                                >
                                    "Connection"
                                </button>
                                <button
                                    class=move || {
                                        let can_go = test_result.get().map(|r| r.success).unwrap_or(false);
                                        if active_tab.get() == "catalog" { TAB_ACTIVE }
                                        else if can_go { TAB_INACTIVE }
                                        else { TAB_DISABLED }
                                    }
                                    disabled=move || !test_result.get().map(|r| r.success).unwrap_or(false)
                                    on:click=move |_| {
                                        if test_result.get_untracked().map(|r| r.success).unwrap_or(false) {
                                            set_active_tab.set("catalog".to_string());
                                        }
                                    }
                                >
                                    "Catalog"
                                </button>
                            </div>
                        </Show>

                        // ── Connect create-mode simplified form ──
                        //
                        // When the user picked "Kyomi Connect" above, we
                        // show a compact form: name + restricted type
                        // selector + a short info panel. No credentials,
                        // no test-connection, no catalog tab — the agent
                        // owns the connection. The submit button lives in
                        // the footer.
                        <Show when=move || {
                            is_create_mode.get()
                                && is_connect.get()
                                && !connect_create_complete.get()
                        }>
                            <ConnectCreateForm
                                name=name
                                set_name=set_name
                                slug=slug
                                set_slug=set_slug
                                slug_manually_edited=slug_manually_edited
                                set_slug_manually_edited=set_slug_manually_edited
                                ds_type=ds_type
                                set_ds_type=set_ds_type
                            />
                        </Show>

                        // Content area (min-height from React). Hidden in
                        // Connect create-mode (simplified form takes over)
                        // and post-create (deployment view takes over).
                        <Show when=move || {
                            !(connect_create_complete.get()
                                || is_create_mode.get() && is_connect.get())
                        }>
                        <div class="space-y-4 min-h-[400px]">

                            // ── CONNECTION TAB ──
                            <Show when=move || active_tab.get() == "connection">
                                <div class="space-y-4">

                                    // ── Sample datasource read-only view (edit mode) ──
                                    // Matches the React `isSampleDatasource` branch in
                                    // renderWorkspaceSettingsTab. Shows Name/Slug/Type as
                                    // non-editable text with an informational alert.
                                    <Show when=move || !is_create_mode.get() && is_sample.get()>
                                        <div class="space-y-4">
                                            <Alert>
                                                <AlertDescription>
                                                    "This is a sample datasource with pre-configured connection settings. Connection settings cannot be modified."
                                                </AlertDescription>
                                            </Alert>
                                            <div class="grid grid-cols-2 gap-4">
                                                <div>
                                                    <label class="block text-sm font-medium mb-1 text-muted-foreground">"Name"</label>
                                                    <p class="text-sm">{move || name.get()}</p>
                                                </div>
                                                <div>
                                                    <label class="block text-sm font-medium mb-1 text-muted-foreground">"Slug"</label>
                                                    <p class="text-sm font-mono">{move || slug.get()}</p>
                                                </div>
                                            </div>
                                            <div>
                                                <label class="block text-sm font-medium mb-1 text-muted-foreground">"Type"</label>
                                                <div class="flex items-center gap-2">
                                                    <span class="h-4 w-4 inline-flex items-center justify-center">
                                                        <Icon icon=phosphor_leptos::DATABASE/>
                                                    </span>
                                                    <span class="text-sm">
                                                        {move || {
                                                            let t = ds_type.get();
                                                            PROVIDER_TYPES
                                                                .iter()
                                                                .find(|(v, _)| *v == t)
                                                                .map(|(_, l)| (*l).to_string())
                                                                .unwrap_or(t)
                                                        }}
                                                    </span>
                                                </div>
                                            </div>
                                        </div>
                                    </Show>

                                    // ── Sample quick-add tile (create mode only) ──
                                    // Matches the React sample quick-add block at the top of
                                    // the connection tab. Only visible for admins when the
                                    // sample ClickHouse is configured and not already added.
                                    <Show when=move || {
                                        is_create_mode.get()
                                            && sample_available.get()
                                            && !sample_already_added.get()
                                    }>
                                        <div class="flex items-center justify-between p-3 border border-border rounded-lg bg-muted/30">
                                            <div class="flex items-center gap-3">
                                                <span class="h-5 w-5 inline-flex items-center justify-center text-muted-foreground">
                                                    <Icon icon=phosphor_leptos::DATABASE/>
                                                </span>
                                                <div>
                                                    <p class="text-sm font-medium">"Acme Analytics (Sample)"</p>
                                                    <p class="text-xs text-muted-foreground">
                                                        "Try Kyomi with demo data — no setup required"
                                                    </p>
                                                </div>
                                            </div>
                                            <Button
                                                variant=ButtonVariant::Outline
                                                size=ButtonSize::Sm
                                                disabled=creating_sample
                                                on:click=move |_| do_create_sample()
                                            >
                                                {move || if creating_sample.get() { "Adding..." } else { "Add Sample" }}
                                            </Button>
                                        </div>
                                    </Show>

                                    // Rest of the connection form — hidden when viewing a sample datasource.
                                    <Show when=move || is_create_mode.get() || !is_sample.get()>
                                    <div class="space-y-4">

                                    // Type selector (create mode only)
                                    <Show when=move || is_create_mode.get()>
                                        <div>
                                            <label class="block text-sm font-medium mb-1">"Type"</label>
                                            <DynSelect
                                                value=Signal::derive(move || ds_type.get())
                                                options=Signal::stored(PROVIDER_TYPES.iter().map(|(v, l)| (v.to_string(), l.to_string())).collect::<Vec<_>>())
                                                on_change=move |val: String| {
                                                    set_ds_type.set(val);
                                                    // Reset discovery when type changes
                                                    set_discovery_status.set("idle".to_string());
                                                    set_test_result.set(None);
                                                    set_discovered_databases.set(vec![]);
                                                    set_discovered_schemas.set(vec![]);
                                                    set_discovered_warehouses.set(vec![]);
                                                    set_discovered_catalogs.set(vec![]);
                                                }
                                            />
                                        </div>
                                    </Show>

                                    // Name & Slug (admin fields)
                                    <div class="grid grid-cols-1 sm:grid-cols-2 gap-4">
                                        <div>
                                            <label class="block text-sm font-medium mb-1">
                                                "Name "
                                                <span class="text-error-foreground">"*"</span>
                                            </label>
                                            <input
                                                type="text"
                                                class=MODAL_INPUT_CLASS
                                                placeholder="Production Database"
                                                prop:value=move || name.get()
                                                on:input=move |ev| {
                                                    handle_name_change(event_target_value(&ev));
                                                }
                                            />
                                        </div>
                                        <div>
                                            <label class="block text-sm font-medium mb-1">"Slug"</label>
                                            <input
                                                type="text"
                                                class=format!("{} font-mono", MODAL_INPUT_CLASS)
                                                placeholder="production-db"
                                                prop:value=move || slug.get()
                                                on:input=move |ev| {
                                                    set_slug_manually_edited.set(true);
                                                    let val = event_target_value(&ev)
                                                        .to_lowercase()
                                                        .chars()
                                                        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
                                                        .collect::<String>();
                                                    set_slug.set(val);
                                                }
                                            />
                                            <p class="text-xs text-muted-foreground mt-1">
                                                {move || if is_create_mode.get() {
                                                    "Auto-generated from name if left empty"
                                                } else {
                                                    "Used in ChartML specs and API calls"
                                                }}
                                            </p>
                                        </div>
                                    </div>

                                    // ── Kyomi Connect status panel ──
                                    // For Connect datasources, the agent owns
                                    // the connection config (host, port,
                                    // credentials), not this modal. Show the
                                    // status / rotate-token / disconnect UI
                                    // instead of the per-provider form.
                                    // create-mode is gated out because Connect
                                    // datasources are created via the dedicated
                                    // Connect Setup page.
                                    <Show
                                        when=move || is_connect.get() && !is_create_mode.get()
                                        // Use a fallback-free closure so the
                                        // panel is freshly mounted (and its
                                        // polling interval freshly spawned)
                                        // each time the user opens a Connect
                                        // datasource in the modal.
                                    >
                                        {move || {
                                            let ds_id = datasource_id
                                                .get()
                                                .unwrap_or_default();
                                            let ds_type_val = ds_type.get();
                                            view! {
                                                <ConnectStatusPanel
                                                    datasource_id=ds_id
                                                    datasource_type=ds_type_val
                                                />
                                            }
                                        }}
                                    </Show>

                                    // Everything from auth-mode selectors onward
                                    // is hidden for Connect datasources — the
                                    // ConnectStatusPanel above replaces them.
                                    <Show when=move || !is_connect.get()>
                                    <div class="space-y-4">

                                    // BigQuery auth mode selector
                                    <Show when=move || ds_type.get() == "bigquery">
                                        <BigQueryAuthModeSection
                                            bq_auth_mode=bq_auth_mode
                                            set_bq_auth_mode=set_bq_auth_mode
                                            cfg_oauth_client_id=cfg_oauth_client_id
                                            set_cfg_oauth_client_id=set_cfg_oauth_client_id
                                            cfg_oauth_client_secret=cfg_oauth_client_secret
                                            set_cfg_oauth_client_secret=set_cfg_oauth_client_secret
                                            cfg_service_account_json=cfg_service_account_json
                                            set_cfg_service_account_json=set_cfg_service_account_json
                                            service_account_email=service_account_email
                                            set_service_account_email=set_service_account_email
                                            slug=slug
                                            cred_billing_project=cred_billing_project
                                            set_cred_billing_project=set_cred_billing_project
                                            cred_default_project=cred_default_project
                                            set_cred_default_project=set_cred_default_project
                                        />
                                    </Show>

                                    // Snowflake auth mode selector
                                    <Show when=move || ds_type.get() == "snowflake">
                                        <SnowflakeAuthModeSection
                                            sf_auth_mode=sf_auth_mode
                                            set_sf_auth_mode=set_sf_auth_mode
                                        />
                                    </Show>

                                    // Connection fields (provider-specific)
                                    <ProviderConnectionFields
                                        signals=ConnectionFieldsSignals {
                                            ds_type,
                                            sf_auth_mode,
                                            cfg_host,
                                            set_cfg_host,
                                            cfg_port,
                                            set_cfg_port,
                                            cfg_ssl_mode,
                                            set_cfg_ssl_mode,
                                            cfg_database,
                                            set_cfg_database,
                                            cfg_account,
                                            set_cfg_account,
                                            cfg_server_hostname,
                                            set_cfg_server_hostname,
                                            cfg_http_path,
                                            set_cfg_http_path,
                                            cfg_secure,
                                            set_cfg_secure,
                                            cfg_encrypt,
                                            set_cfg_encrypt,
                                            cfg_trust_cert,
                                            set_cfg_trust_cert,
                                            cfg_oauth_client_id,
                                            set_cfg_oauth_client_id,
                                            cfg_oauth_client_secret,
                                            set_cfg_oauth_client_secret,
                                        }
                                    />

                                    // Credentials section (non-BigQuery / non-Snowflake-OAuth)
                                    <ProviderCredentialsFields
                                        signals=CredentialsFieldsSignals {
                                            ds_type,
                                            sf_auth_mode,
                                            bq_auth_mode,
                                            cred_username,
                                            set_cred_username,
                                            cred_password,
                                            set_cred_password,
                                            cred_access_token,
                                            set_cred_access_token,
                                            cred_private_key,
                                            set_cred_private_key,
                                            cfg_shared_credentials,
                                            set_cfg_shared_credentials,
                                        }
                                    />

                                    // Test & Discover button
                                    // Hidden for BigQuery (uses OAuth) and Snowflake OAuth mode
                                    <Show when=move || {
                                        let t = ds_type.get();
                                        let sf = sf_auth_mode.get();
                                        !(t == "bigquery" || (t == "snowflake" && sf == "oauth"))
                                    }>
                                        <div class="border-t border-border pt-4 mt-4">
                                            <div class="flex items-center gap-3">
                                                <button
                                                    type="button"
                                                    class="inline-flex items-center justify-center gap-2 whitespace-nowrap rounded-md text-sm font-medium transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring h-9 px-4 py-2 border border-input bg-background text-foreground shadow-sm hover:bg-secondary hover:text-accent-foreground disabled:pointer-events-none disabled:opacity-50"
                                                    disabled=move || test_action.pending().get()
                                                    on:click=move |_| do_test_and_discover()
                                                >
                                                    <span class="h-4 w-4 inline-flex items-center justify-center">
                                                        <Icon icon=phosphor_leptos::PLUG/>
                                                    </span>
                                                    {move || if test_action.pending().get() { "Discovering..." } else { "Test & Discover" }}
                                                </button>
                                                {move || test_result.get().map(|r| {
                                                    if r.success {
                                                        view! {
                                                            <div class="flex items-center gap-2 text-sm text-success-foreground">
                                                                <Icon icon=phosphor_leptos::CHECK attr:class="h-4 w-4"/>
                                                                "Connected"
                                                            </div>
                                                        }.into_any()
                                                    } else {
                                                        view! {
                                                            <div class="flex items-center gap-2 text-sm text-error-foreground">
                                                                <Icon icon=phosphor_leptos::X attr:class="h-4 w-4"/>
                                                                "Failed"
                                                            </div>
                                                        }.into_any()
                                                    }
                                                })}
                                            </div>
                                            {move || discovery_error.get().filter(|_| discovery_status.get() == "error").map(|msg| view! {
                                                <Alert variant=AlertVariant::Warning class="mt-3">
                                                    <AlertDescription>{msg}</AlertDescription>
                                                </Alert>
                                            })}
                                            <p class="text-xs text-muted-foreground mt-2">
                                                "Validate connection and discover available resources"
                                            </p>
                                        </div>
                                    </Show>

                                    // Discovery fields (shown after successful Test & Discover, or always in edit mode)
                                    <Show when=move || {
                                        let t = ds_type.get();
                                        let is_create = is_create_mode.get();
                                        t != "bigquery" && (!is_create || discovery_succeeded.get())
                                    }>
                                        <DiscoveryFields
                                            signals=DiscoveryFieldsSignals {
                                                ds_type,
                                                discovery_succeeded,
                                                discovered_databases,
                                                discovered_schemas,
                                                discovered_warehouses,
                                                discovered_catalogs,
                                                cfg_database,
                                                set_cfg_database,
                                                cfg_schema,
                                                set_cfg_schema,
                                                cfg_warehouse,
                                                set_cfg_warehouse,
                                                cfg_catalog,
                                                set_cfg_catalog,
                                                cfg_role,
                                                set_cfg_role,
                                            }
                                        />
                                    </Show>

                                    </div>
                                    </Show>

                                    </div>
                                    </Show>

                                </div>
                            </Show>

                            // ── CATALOG TAB (create mode only) ──
                            <Show when=move || active_tab.get() == "catalog" && is_create_mode.get()>
                                <div class="space-y-4">
                                    <div>
                                        <h4 class="text-sm font-medium mb-1">"Catalog Configuration"</h4>
                                        <p class="text-sm text-muted-foreground">
                                            "Your datasource will be indexed automatically after creation. You can configure catalog settings in the datasource settings later."
                                        </p>
                                    </div>
                                    <div class="p-4 border border-border rounded-lg bg-muted/30">
                                        <div class="flex items-center gap-2">
                                            <Icon icon=phosphor_leptos::CHECK attr:class="h-5 w-5 text-success-foreground"/>
                                            <span class="text-sm font-medium">"Connection verified"</span>
                                        </div>
                                        <p class="text-xs text-muted-foreground mt-1">
                                            "Click Create to add this datasource. The catalog will be indexed automatically."
                                        </p>
                                    </div>
                                </div>
                            </Show>

                            // ── CATALOG TAB (edit mode only) ──
                            <Show when=move || {
                                active_tab.get() == "catalog" && !is_create_mode.get()
                            }>
                                <EditModeCatalogTab
                                    datasource_id=Signal::derive(move || {
                                        datasource_id.get().unwrap_or_default()
                                    })
                                    datasource_slug=Signal::derive(move || slug.get())
                                    datasource_type=Signal::derive(move || ds_type.get())
                                    connection_config=Signal::derive(build_connection_config)
                                    credentials=Signal::derive(build_credentials)
                                    is_sample=is_sample
                                    catalog_selected=catalog_selected
                                    set_catalog_selected=set_catalog_selected
                                    bq_include_public=bq_include_public
                                    set_bq_include_public=set_bq_include_public
                                />
                            </Show>

                        </div>
                        </Show>
                    </Show>
                }
            }}
        </Modal>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Connect Create Form (create-mode, simplified)
// ─────────────────────────────────────────────────────────────────────────────

/// Simplified form rendered when the create-mode user selects "Kyomi Connect".
///
/// Shows only the fields needed to provision a Connect datasource: name +
/// slug + type (restricted to [`CONNECT_TYPES`]). Submission happens via
/// the modal footer (`do_create_connect`), not via an in-form button, so
/// the user's save gesture is consistent across Direct and Connect modes.
#[component]
fn ConnectCreateForm(
    name: ReadSignal<String>,
    set_name: WriteSignal<String>,
    slug: ReadSignal<String>,
    set_slug: WriteSignal<String>,
    slug_manually_edited: ReadSignal<bool>,
    set_slug_manually_edited: WriteSignal<bool>,
    ds_type: ReadSignal<String>,
    set_ds_type: WriteSignal<String>,
) -> impl IntoView {
    view! {
        <div class="space-y-4 min-h-[400px]">
            // Info panel — matches the React "How Kyomi Connect works" box
            // (same three-step sequence, same copy).
            <div class="rounded-lg border border-border bg-muted/30 p-4">
                <p class="text-sm text-foreground font-medium mb-2">
                    "How Kyomi Connect works"
                </p>
                <ol class="text-sm text-muted-foreground space-y-1 list-decimal list-inside">
                    <li>"Save this datasource to generate a secure token"</li>
                    <li>"Deploy the Kyomi Connect agent in your network"</li>
                    <li>"The agent connects outbound to Kyomi — no inbound access needed"</li>
                </ol>
            </div>

            // Name + Slug
            <div class="grid grid-cols-1 sm:grid-cols-2 gap-4">
                <div>
                    <label class="block text-sm font-medium mb-1">
                        "Name "
                        <span class="text-error-foreground">"*"</span>
                    </label>
                    <input
                        type="text"
                        class=MODAL_INPUT_CLASS
                        placeholder="Production Database"
                        prop:value=move || name.get()
                        on:input=move |ev| {
                            let new_name = event_target_value(&ev);
                            // Auto-generate slug until the user edits it
                            // manually — same rule the Direct form uses.
                            if !slug_manually_edited.get_untracked() {
                                set_slug.set(generate_slug(&new_name));
                            }
                            set_name.set(new_name);
                        }
                    />
                </div>
                <div>
                    <label class="block text-sm font-medium mb-1">"Slug"</label>
                    <input
                        type="text"
                        class=format!("{} font-mono", MODAL_INPUT_CLASS)
                        placeholder="production-db"
                        prop:value=move || slug.get()
                        on:input=move |ev| {
                            set_slug_manually_edited.set(true);
                            let val = event_target_value(&ev)
                                .to_lowercase()
                                .chars()
                                .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
                                .collect::<String>();
                            set_slug.set(val);
                        }
                    />
                    <p class="text-xs text-muted-foreground mt-1">
                        "Auto-generated from name if left empty"
                    </p>
                </div>
            </div>

            // Type selector — restricted to Connect-compatible types.
            // Drives the default port baked into the deployment commands
            // on the post-create view.
            <div>
                <label class="block text-sm font-medium mb-1">"Type"</label>
                <DynSelect
                    value=Signal::derive(move || ds_type.get())
                    options=Signal::stored(
                        CONNECT_TYPES
                            .iter()
                            .map(|(v, l)| ((*v).to_string(), (*l).to_string()))
                            .collect::<Vec<_>>()
                    )
                    on_change=move |val: String| set_ds_type.set(val)
                />
                <p class="text-xs text-muted-foreground mt-1">
                    "BigQuery, Snowflake, and Databricks use OAuth and don't need Kyomi Connect."
                </p>
            </div>
        </div>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Connect Create Success View (post-create token display + deployment tabs)
// ─────────────────────────────────────────────────────────────────────────────

/// Post-create view rendered after `create_connect_datasource` succeeds.
///
/// Shows a one-time-display warning Alert, the new Connect token in a
/// copy-ready mono block, and the four deployment-command tabs with the
/// real token substituted in. The "Done" button lives in the modal footer
/// and fires `on_saved` to close the modal + refresh the list.
///
/// Reuses [`build_deployment_commands`] (same function powering the
/// edit-mode `ConnectStatusPanel`) so command text stays byte-equivalent
/// across both flows.
#[component]
fn ConnectCreateSuccessView(
    token: Signal<String>,
    datasource_name: Signal<String>,
    datasource_type: Signal<String>,
    active_tab: ReadSignal<String>,
    set_active_tab: WriteSignal<String>,
) -> impl IntoView {
    // Derived: the four deployment commands, rebuilt whenever the token or
    // type changes (both are stable for the lifetime of this view, but the
    // memo keeps us honest if that ever stops being true).
    let deployment_commands: Memo<DeploymentCommands> = Memo::new(move |_| {
        let tok = token.get();
        let ty = datasource_type.get();
        build_deployment_commands(&ty, Some(&tok), None)
    });

    // Displayed port reflects the default for the selected type — matches
    // what the commands embed, so the user can correlate them.
    let displayed_port = Signal::derive(move || default_port(&datasource_type.get()));

    let active_command = move || -> String {
        let tab = active_tab.get();
        deployment_commands.with(|cmds| cmds.for_tab(&tab).to_string())
    };

    view! {
        <div class="space-y-5 min-h-[400px]">
            // Header — datasource name + short framing text.
            <div class="space-y-1">
                <h3 class="text-lg font-semibold text-foreground">
                    "Deploy Kyomi Connect"
                </h3>
                <p class="text-sm text-muted-foreground">
                    "Install the Connect agent to bridge "
                    <span class="font-medium text-foreground">
                        {move || datasource_name.get()}
                    </span>
                    " to Kyomi."
                </p>
            </div>

            // One-time-display warning — same copy as ConnectStatusPanel's
            // post-rotation alert so the "save this token" message reads
            // identically across the creation + rotation flows.
            <Alert variant=AlertVariant::Warning>
                <AlertTitle>"Save this token now"</AlertTitle>
                <AlertDescription>
                    "It will not be shown again. Store it somewhere safe — \
                     you'll need it to deploy the Kyomi Connect agent."
                </AlertDescription>
            </Alert>

            // Token display with copy-to-clipboard.
            <div class="space-y-1.5">
                <label class="block text-sm font-medium text-foreground">
                    "Connect Token"
                </label>
                <div class="flex items-center gap-2 rounded-md border border-border bg-muted/30 px-3 py-2">
                    <code class="flex-1 text-xs font-mono text-foreground break-all select-all">
                        {move || token.get()}
                    </code>
                    <CopyButton text=Signal::derive(move || token.get())/>
                </div>
            </div>

            // Deployment tabs — same module powers ConnectStatusPanel.
            <div class="space-y-2">
                <div class="flex items-center justify-between">
                    <h4 class="text-sm font-medium text-foreground">"Deployment Instructions"</h4>
                    <span class="text-xs text-muted-foreground font-mono">
                        {move || format!("default port: {}", displayed_port.get())}
                    </span>
                </div>
                <DeploymentTabStrip
                    active_tab=active_tab.into()
                    set_active_tab=set_active_tab
                />
                <div class="relative rounded-md border border-border bg-muted/30">
                    <pre class="p-4 pr-12 text-xs font-mono text-foreground overflow-x-auto whitespace-pre">
                        {active_command}
                    </pre>
                    <div class="absolute top-2 right-2">
                        <CopyButton text=Signal::derive(move || {
                            let tab = active_tab.get();
                            deployment_commands.with(|cmds| cmds.for_tab(&tab).to_string())
                        })/>
                    </div>
                </div>
            </div>
        </div>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// BigQuery Auth Mode Section
// ─────────────────────────────────────────────────────────────────────────────

#[component]
fn BigQueryAuthModeSection(
    bq_auth_mode: ReadSignal<String>,
    set_bq_auth_mode: WriteSignal<String>,
    cfg_oauth_client_id: ReadSignal<String>,
    set_cfg_oauth_client_id: WriteSignal<String>,
    cfg_oauth_client_secret: ReadSignal<String>,
    set_cfg_oauth_client_secret: WriteSignal<String>,
    cfg_service_account_json: ReadSignal<String>,
    set_cfg_service_account_json: WriteSignal<String>,
    service_account_email: ReadSignal<String>,
    set_service_account_email: WriteSignal<String>,
    slug: ReadSignal<String>,
    cred_billing_project: ReadSignal<String>,
    set_cred_billing_project: WriteSignal<String>,
    cred_default_project: ReadSignal<String>,
    set_cred_default_project: WriteSignal<String>,
) -> impl IntoView {
    // Parse service account email from JSON
    let handle_service_account_json = move |json_text: String| {
        set_cfg_service_account_json.set(json_text.clone());
        if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&json_text) {
            if let Some(email) = parsed.get("client_email").and_then(|v| v.as_str()) {
                set_service_account_email.set(email.to_string());
            } else {
                set_service_account_email.set(String::new());
            }
        } else if json_text.is_empty() {
            set_service_account_email.set(String::new());
        }
    };

    let slug_for_oauth = slug;

    view! {
        <div class="space-y-2 pb-4 border-b border-border">
            <label class="block text-sm font-medium">"Authentication Mode"</label>
            <DynSelect
                value=Signal::derive(move || bq_auth_mode.get())
                options=Signal::stored(vec![
                    ("kyomi_oauth".to_string(), "Kyomi OAuth (Recommended)".to_string()),
                    ("enterprise_oauth".to_string(), "Enterprise OAuth".to_string()),
                    ("service_account".to_string(), "Service Account".to_string()),
                ])
                on_change=move |val| set_bq_auth_mode.set(val)
            />
            <p class="text-xs text-muted-foreground">
                {move || match bq_auth_mode.get().as_str() {
                    "kyomi_oauth" => "Users authenticate with their Google accounts via Kyomi.",
                    "enterprise_oauth" => "Users authenticate with your organization's OAuth app.",
                    "service_account" => "All users share a service account for automated access.",
                    _ => "",
                }}
            </p>
        </div>

        // BigQuery Credentials section
        <div class="space-y-4 border-t border-border pt-4 mt-4">
            <h4 class="text-sm font-medium">"BigQuery Credentials"</h4>

            // Kyomi OAuth mode
            <Show when=move || bq_auth_mode.get() == "kyomi_oauth">
                <div class="space-y-3">
                    <p class="text-sm text-muted-foreground">
                        "Connect your Google account to access BigQuery projects."
                    </p>
                    <a
                        href="/api/v1/auth/google-oauth/connect"
                        target="_blank"
                        rel="noopener noreferrer"
                        class="inline-flex items-center justify-center gap-2 whitespace-nowrap rounded-md text-sm font-medium transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring h-9 px-4 py-2 border border-input bg-background text-foreground shadow-sm hover:bg-secondary hover:text-accent-foreground"
                    >
                        <Icon icon=phosphor_leptos::LINK attr:class="h-4 w-4"/>
                        "Connect Google Account"
                    </a>
                    <p class="text-xs text-muted-foreground">
                        "After connecting, you can set the billing and default project."
                    </p>
                    <div class="grid grid-cols-1 sm:grid-cols-2 gap-4">
                        <div>
                            <label class="block text-sm font-medium mb-1">"Billing Project"</label>
                            <input
                                type="text"
                                class=MODAL_INPUT_CLASS
                                placeholder="my-gcp-project"
                                prop:value=move || cred_billing_project.get()
                                on:input=move |ev| set_cred_billing_project.set(event_target_value(&ev))
                            />
                        </div>
                        <div>
                            <label class="block text-sm font-medium mb-1">"Default Project"</label>
                            <input
                                type="text"
                                class=MODAL_INPUT_CLASS
                                placeholder="my-gcp-project"
                                prop:value=move || cred_default_project.get()
                                on:input=move |ev| set_cred_default_project.set(event_target_value(&ev))
                            />
                        </div>
                    </div>
                </div>
            </Show>

            // Enterprise OAuth mode
            <Show when=move || bq_auth_mode.get() == "enterprise_oauth">
                <div class="space-y-4">
                    // Admin OAuth configuration
                    <div class="space-y-3 pb-4 border-b border-border">
                        <h4 class="text-sm font-medium">"OAuth Configuration"</h4>
                        <p class="text-xs text-muted-foreground">
                            "Configure your organization's Google Cloud OAuth app."
                        </p>
                        <div class="p-3 bg-muted/30 rounded-lg space-y-1">
                            <label class="block text-xs font-medium text-muted-foreground">
                                "Redirect URL (use when creating Google Cloud OAuth client)"
                            </label>
                        </div>
                        <div class="grid grid-cols-1 sm:grid-cols-2 gap-4">
                            <div>
                                <label class="block text-sm font-medium mb-1">"OAuth Client ID"</label>
                                <input
                                    type="text"
                                    class=MODAL_INPUT_CLASS
                                    placeholder="From Google Cloud Console"
                                    prop:value=move || cfg_oauth_client_id.get()
                                    on:input=move |ev| set_cfg_oauth_client_id.set(event_target_value(&ev))
                                />
                            </div>
                            <div>
                                <label class="block text-sm font-medium mb-1">"OAuth Client Secret"</label>
                                <input
                                    type="password"
                                    class=MODAL_INPUT_CLASS
                                    placeholder="OAuth client secret"
                                    prop:value=move || cfg_oauth_client_secret.get()
                                    on:input=move |ev| set_cfg_oauth_client_secret.set(event_target_value(&ev))
                                />
                            </div>
                        </div>
                    </div>
                    // User connection
                    <div class="space-y-3">
                        <h4 class="text-sm font-medium">"Your Connection"</h4>
                        {move || {
                            let slug_val = slug_for_oauth.get();
                            let oauth_url = format!("/api/v1/auth/oauth/bigquery-enterprise/connect?datasource_slug={}", slug_val);
                            view! {
                                <a
                                    href=oauth_url
                                    target="_blank"
                                    rel="noopener noreferrer"
                                    class="inline-flex items-center justify-center gap-2 whitespace-nowrap rounded-md text-sm font-medium transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring h-9 px-4 py-2 border border-input bg-background text-foreground shadow-sm hover:bg-secondary hover:text-accent-foreground"
                                >
                                    <Icon icon=phosphor_leptos::LINK attr:class="h-4 w-4"/>
                                    "Connect BigQuery"
                                </a>
                            }
                        }}
                    </div>
                </div>
            </Show>

            // Service Account mode
            <Show when=move || bq_auth_mode.get() == "service_account">
                <div class="space-y-4">
                    <p class="text-xs text-muted-foreground">
                        "Upload or paste your Google Cloud service account credentials JSON."
                    </p>
                    {move || service_account_email.get().is_empty().then(|| view! {
                        <div class="space-y-3">
                            <div>
                                <label class="block text-sm font-medium mb-1">"Service Account JSON"</label>
                                <textarea
                                    rows="6"
                                    class="w-full px-3 py-2 border border-input rounded-md bg-background text-sm font-mono focus:outline-none focus:ring-1 focus:ring-ring"
                                    placeholder=r#"{"type": "service_account", "client_email": "...", ...}"#
                                    prop:value=move || cfg_service_account_json.get()
                                    on:input=move |ev| {
                                        handle_service_account_json(event_target_value(&ev));
                                    }
                                />
                                <p class="text-xs text-muted-foreground mt-1">
                                    "Paste the contents of your service account JSON file"
                                </p>
                            </div>
                        </div>
                    })}
                    {move || {
                        let email = service_account_email.get();
                        (!email.is_empty()).then(move || view! {
                            <div class="flex items-center justify-between p-3 bg-muted/50 rounded-lg">
                                <div class="flex items-center gap-2">
                                    <Icon icon=phosphor_leptos::CHECK attr:class="h-4 w-4 text-success-foreground"/>
                                    <span class="text-sm text-foreground">
                                        {format!("Service Account: {email}")}
                                    </span>
                                </div>
                                <Button
                                    variant=ButtonVariant::Outline
                                    size=ButtonSize::Sm
                                    on:click=move |_| {
                                        set_service_account_email.set(String::new());
                                        set_cfg_service_account_json.set(String::new());
                                    }
                                >
                                    "Remove"
                                </Button>
                            </div>
                        })
                    }}
                </div>
            </Show>
        </div>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Snowflake Auth Mode Section
// ─────────────────────────────────────────────────────────────────────────────

#[component]
fn SnowflakeAuthModeSection(
    sf_auth_mode: ReadSignal<String>,
    set_sf_auth_mode: WriteSignal<String>,
) -> impl IntoView {
    view! {
        <div class="space-y-2 pb-4 border-b border-border">
            <label class="block text-sm font-medium">"Authentication Mode"</label>
            <DynSelect
                value=Signal::derive(move || sf_auth_mode.get())
                options=Signal::stored(vec![
                    ("password".to_string(), "Password".to_string()),
                    ("oauth".to_string(), "OAuth".to_string()),
                    ("keypair".to_string(), "Key-Pair".to_string()),
                ])
                on_change=move |val| set_sf_auth_mode.set(val)
            />
            <p class="text-xs text-muted-foreground">
                {move || match sf_auth_mode.get().as_str() {
                    "oauth" => "Users authenticate with their Snowflake accounts via OAuth.",
                    "password" => "Users authenticate with username and password.",
                    "keypair" => "Users authenticate using RSA key-pair.",
                    _ => "",
                }}
            </p>
        </div>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Provider Connection Fields
// ─────────────────────────────────────────────────────────────────────────────

/// Bundle of every signal `ProviderConnectionFields` needs.
/// Packed as a single prop to keep the component signature small;
/// signals are `Copy`, so cloning the struct is cheap.
#[derive(Clone, Copy)]
struct ConnectionFieldsSignals {
    ds_type: ReadSignal<String>,
    sf_auth_mode: ReadSignal<String>,
    cfg_host: ReadSignal<String>,
    set_cfg_host: WriteSignal<String>,
    cfg_port: ReadSignal<String>,
    set_cfg_port: WriteSignal<String>,
    cfg_ssl_mode: ReadSignal<String>,
    set_cfg_ssl_mode: WriteSignal<String>,
    cfg_database: ReadSignal<String>,
    set_cfg_database: WriteSignal<String>,
    cfg_account: ReadSignal<String>,
    set_cfg_account: WriteSignal<String>,
    cfg_server_hostname: ReadSignal<String>,
    set_cfg_server_hostname: WriteSignal<String>,
    cfg_http_path: ReadSignal<String>,
    set_cfg_http_path: WriteSignal<String>,
    cfg_secure: ReadSignal<bool>,
    set_cfg_secure: WriteSignal<bool>,
    cfg_encrypt: ReadSignal<bool>,
    set_cfg_encrypt: WriteSignal<bool>,
    cfg_trust_cert: ReadSignal<bool>,
    set_cfg_trust_cert: WriteSignal<bool>,
    cfg_oauth_client_id: ReadSignal<String>,
    set_cfg_oauth_client_id: WriteSignal<String>,
    cfg_oauth_client_secret: ReadSignal<String>,
    set_cfg_oauth_client_secret: WriteSignal<String>,
}

/// Renders the connection fields (host/port/ssl_mode/account/etc.) for the active provider.
/// Also renders the database text input so users can type a database name before Test & Discover.
/// After Test & Discover, DiscoveryFields replaces the text input with a dropdown of real db names.
#[component]
fn ProviderConnectionFields(signals: ConnectionFieldsSignals) -> impl IntoView {
    let ConnectionFieldsSignals {
        ds_type,
        sf_auth_mode,
        cfg_host,
        set_cfg_host,
        cfg_port,
        set_cfg_port,
        cfg_ssl_mode,
        set_cfg_ssl_mode,
        cfg_database,
        set_cfg_database,
        cfg_account,
        set_cfg_account,
        cfg_server_hostname,
        set_cfg_server_hostname,
        cfg_http_path,
        set_cfg_http_path,
        cfg_secure,
        set_cfg_secure,
        cfg_encrypt,
        set_cfg_encrypt,
        cfg_trust_cert,
        set_cfg_trust_cert,
        cfg_oauth_client_id,
        set_cfg_oauth_client_id,
        cfg_oauth_client_secret,
        set_cfg_oauth_client_secret,
    } = signals;
    view! {
        {move || {
            let t = ds_type.get();
            match t.as_str() {
                "postgres" | "mysql" | "redshift" => view! {
                    <div class="space-y-4">
                        <h4 class="text-sm font-medium">"Connection Settings"</h4>
                        <div class="grid grid-cols-1 sm:grid-cols-2 gap-4">
                            <div>
                                <label class="block text-sm font-medium mb-1">
                                    "Host " <span class="text-error-foreground">"*"</span>
                                </label>
                                <input type="text" class=MODAL_INPUT_CLASS
                                    placeholder="db.example.com"
                                    prop:value=move || cfg_host.get()
                                    on:input=move |ev| set_cfg_host.set(event_target_value(&ev))
                                />
                            </div>
                            <div>
                                <label class="block text-sm font-medium mb-1">"Port"</label>
                                <input type="number" class=MODAL_INPUT_CLASS
                                    placeholder=if t == "mysql" { "3306" } else { "5432" }
                                    prop:value=move || cfg_port.get()
                                    on:input=move |ev| set_cfg_port.set(event_target_value(&ev))
                                />
                            </div>
                        </div>
                        <div>
                            <label class="block text-sm font-medium mb-1">"SSL Mode"</label>
                            <DynSelect
                                value=Signal::derive(move || cfg_ssl_mode.get())
                                options=Signal::stored(vec![
                                    ("disable".to_string(), "Disable".to_string()),
                                    ("require".to_string(), "Require".to_string()),
                                    ("verify-ca".to_string(), "Verify CA".to_string()),
                                    ("verify-full".to_string(), "Verify Full".to_string()),
                                ])
                                on_change=move |val| set_cfg_ssl_mode.set(val)
                            />
                        </div>
                        <div>
                            <label class="block text-sm font-medium mb-1">"Database"</label>
                            <input type="text" class=MODAL_INPUT_CLASS
                                placeholder="mydb"
                                prop:value=move || cfg_database.get()
                                on:input=move |ev| set_cfg_database.set(event_target_value(&ev))
                            />
                        </div>
                    </div>
                }.into_any(),

                "clickhouse" => view! {
                    <div class="space-y-4">
                        <h4 class="text-sm font-medium">"Connection Settings"</h4>
                        <div class="grid grid-cols-1 sm:grid-cols-2 gap-4">
                            <div>
                                <label class="block text-sm font-medium mb-1">
                                    "Host " <span class="text-error-foreground">"*"</span>
                                </label>
                                <input type="text" class=MODAL_INPUT_CLASS
                                    placeholder="clickhouse.example.com"
                                    prop:value=move || cfg_host.get()
                                    on:input=move |ev| set_cfg_host.set(event_target_value(&ev))
                                />
                            </div>
                            <div>
                                <label class="block text-sm font-medium mb-1">"Port"</label>
                                <input type="number" class=MODAL_INPUT_CLASS
                                    placeholder="8123"
                                    prop:value=move || cfg_port.get()
                                    on:input=move |ev| set_cfg_port.set(event_target_value(&ev))
                                />
                            </div>
                        </div>
                        <label class="flex items-center gap-2 cursor-pointer">
                            <input
                                type="checkbox"
                                class="h-4 w-4 rounded-md border-input"
                                prop:checked=move || cfg_secure.get()
                                on:change=move |ev| {
                                    let checked = event_target_checked(&ev);
                                    set_cfg_secure.set(checked);
                                }
                            />
                            <span class="text-sm">"Secure (HTTPS)"</span>
                        </label>
                        <div>
                            <label class="block text-sm font-medium mb-1">"Database"</label>
                            <input type="text" class=MODAL_INPUT_CLASS
                                placeholder="default"
                                prop:value=move || cfg_database.get()
                                on:input=move |ev| set_cfg_database.set(event_target_value(&ev))
                            />
                        </div>
                    </div>
                }.into_any(),

                "snowflake" => view! {
                    <div class="space-y-4">
                        <h4 class="text-sm font-medium">"Connection Settings"</h4>
                        <div>
                            <label class="block text-sm font-medium mb-1">
                                "Account " <span class="text-error-foreground">"*"</span>
                            </label>
                            <input type="text" class=MODAL_INPUT_CLASS
                                placeholder="xy12345.us-east-1 or myorg-myaccount"
                                prop:value=move || cfg_account.get()
                                on:input=move |ev| set_cfg_account.set(event_target_value(&ev))
                            />
                            <p class="text-xs text-muted-foreground mt-1">
                                "Your Snowflake account identifier (found in your Snowflake URL)"
                            </p>
                        </div>
                        // Snowflake OAuth config (admin only, when oauth mode)
                        <Show when=move || sf_auth_mode.get() == "oauth">
                            <div class="space-y-3 pt-2 border-t border-border">
                                <h4 class="text-sm font-medium">"OAuth Configuration"</h4>
                                <div class="grid grid-cols-1 sm:grid-cols-2 gap-4">
                                    <div>
                                        <label class="block text-sm font-medium mb-1">"OAuth Client ID"</label>
                                        <input type="text" class=MODAL_INPUT_CLASS
                                            placeholder="From Snowflake OAuth integration"
                                            prop:value=move || cfg_oauth_client_id.get()
                                            on:input=move |ev| set_cfg_oauth_client_id.set(event_target_value(&ev))
                                        />
                                    </div>
                                    <div>
                                        <label class="block text-sm font-medium mb-1">"OAuth Client Secret"</label>
                                        <input type="password" class=MODAL_INPUT_CLASS
                                            placeholder="OAuth client secret"
                                            prop:value=move || cfg_oauth_client_secret.get()
                                            on:input=move |ev| set_cfg_oauth_client_secret.set(event_target_value(&ev))
                                        />
                                    </div>
                                </div>
                            </div>
                        </Show>
                    </div>
                }.into_any(),

                "databricks" => view! {
                    <div class="space-y-4">
                        <h4 class="text-sm font-medium">"Connection Settings"</h4>
                        <div>
                            <label class="block text-sm font-medium mb-1">
                                "Server Hostname " <span class="text-error-foreground">"*"</span>
                            </label>
                            <input type="text" class=MODAL_INPUT_CLASS
                                placeholder="dbc-xxxxxxxx-xxxx.cloud.databricks.com"
                                prop:value=move || cfg_server_hostname.get()
                                on:input=move |ev| set_cfg_server_hostname.set(event_target_value(&ev))
                            />
                        </div>
                        <div>
                            <label class="block text-sm font-medium mb-1">
                                "HTTP Path " <span class="text-error-foreground">"*"</span>
                            </label>
                            <input type="text" class=MODAL_INPUT_CLASS
                                placeholder="/sql/1.0/warehouses/xxxx"
                                prop:value=move || cfg_http_path.get()
                                on:input=move |ev| set_cfg_http_path.set(event_target_value(&ev))
                            />
                        </div>
                    </div>
                }.into_any(),

                "sqlserver" | "synapse" => view! {
                    <div class="space-y-4">
                        <h4 class="text-sm font-medium">"Connection Settings"</h4>
                        <div class="grid grid-cols-1 sm:grid-cols-2 gap-4">
                            <div>
                                <label class="block text-sm font-medium mb-1">
                                    "Host " <span class="text-error-foreground">"*"</span>
                                </label>
                                <input type="text" class=MODAL_INPUT_CLASS
                                    placeholder="sqlserver.example.com"
                                    prop:value=move || cfg_host.get()
                                    on:input=move |ev| set_cfg_host.set(event_target_value(&ev))
                                />
                            </div>
                            <div>
                                <label class="block text-sm font-medium mb-1">"Port"</label>
                                <input type="number" class=MODAL_INPUT_CLASS
                                    placeholder="1433"
                                    prop:value=move || cfg_port.get()
                                    on:input=move |ev| set_cfg_port.set(event_target_value(&ev))
                                />
                            </div>
                        </div>
                        <div>
                            <label class="block text-sm font-medium mb-1">"Database"</label>
                            <input type="text" class=MODAL_INPUT_CLASS
                                placeholder="master"
                                prop:value=move || cfg_database.get()
                                on:input=move |ev| set_cfg_database.set(event_target_value(&ev))
                            />
                        </div>
                        <div class="grid grid-cols-1 sm:grid-cols-2 gap-4">
                            <label class="flex items-center gap-2 cursor-pointer">
                                <input
                                    type="checkbox"
                                    class="h-4 w-4 rounded-md border-input"
                                    prop:checked=move || cfg_encrypt.get()
                                    on:change=move |ev| {
                                        set_cfg_encrypt.set(event_target_checked(&ev));
                                    }
                                />
                                <span class="text-sm">"Encrypt Connection"</span>
                            </label>
                            <label class="flex items-center gap-2 cursor-pointer">
                                <input
                                    type="checkbox"
                                    class="h-4 w-4 rounded-md border-input"
                                    prop:checked=move || cfg_trust_cert.get()
                                    on:change=move |ev| {
                                        set_cfg_trust_cert.set(event_target_checked(&ev));
                                    }
                                />
                                <span class="text-sm">"Trust Server Certificate"</span>
                            </label>
                        </div>
                    </div>
                }.into_any(),

                "bigquery" => view! {
                    // BigQuery has no connection fields — handled in BigQueryAuthModeSection
                    <div></div>
                }.into_any(),

                _ => view! {
                    <div class="space-y-4">
                        <h4 class="text-sm font-medium">"Connection Settings"</h4>
                        <div class="grid grid-cols-1 sm:grid-cols-2 gap-4">
                            <div>
                                <label class="block text-sm font-medium mb-1">"Host"</label>
                                <input type="text" class=MODAL_INPUT_CLASS
                                    placeholder="host.example.com"
                                    prop:value=move || cfg_host.get()
                                    on:input=move |ev| set_cfg_host.set(event_target_value(&ev))
                                />
                            </div>
                            <div>
                                <label class="block text-sm font-medium mb-1">"Port"</label>
                                <input type="number" class=MODAL_INPUT_CLASS
                                    placeholder="5432"
                                    prop:value=move || cfg_port.get()
                                    on:input=move |ev| set_cfg_port.set(event_target_value(&ev))
                                />
                            </div>
                        </div>
                    </div>
                }.into_any(),
            }
        }}
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Provider Credentials Fields
// ─────────────────────────────────────────────────────────────────────────────

/// Bundle of every signal `ProviderCredentialsFields` needs.
#[derive(Clone, Copy)]
struct CredentialsFieldsSignals {
    ds_type: ReadSignal<String>,
    sf_auth_mode: ReadSignal<String>,
    bq_auth_mode: ReadSignal<String>,
    cred_username: ReadSignal<String>,
    set_cred_username: WriteSignal<String>,
    cred_password: ReadSignal<String>,
    set_cred_password: WriteSignal<String>,
    cred_access_token: ReadSignal<String>,
    set_cred_access_token: WriteSignal<String>,
    cred_private_key: ReadSignal<String>,
    set_cred_private_key: WriteSignal<String>,
    cfg_shared_credentials: ReadSignal<bool>,
    set_cfg_shared_credentials: WriteSignal<bool>,
}

/// Renders credentials fields for the active provider.
/// Skipped for BigQuery (handled in BigQueryAuthModeSection) and Snowflake OAuth.
#[component]
fn ProviderCredentialsFields(signals: CredentialsFieldsSignals) -> impl IntoView {
    let CredentialsFieldsSignals {
        ds_type,
        sf_auth_mode,
        bq_auth_mode,
        cred_username,
        set_cred_username,
        cred_password,
        set_cred_password,
        cred_access_token,
        set_cred_access_token,
        cred_private_key,
        set_cred_private_key,
        cfg_shared_credentials,
        set_cfg_shared_credentials,
    } = signals;
    view! {
        {move || {
            let t = ds_type.get();
            let sf = sf_auth_mode.get();
            let _bq = bq_auth_mode.get();

            // BigQuery is handled entirely in BigQueryAuthModeSection
            if t == "bigquery" {
                return view! { <div></div> }.into_any();
            }

            // Snowflake OAuth — no password fields shown
            if t == "snowflake" && sf == "oauth" {
                return view! { <div></div> }.into_any();
            }

            view! {
                <div class="space-y-4 border-t border-border pt-4 mt-4">
                    <div class="flex items-center justify-between">
                        <h4 class="text-sm font-medium">"Credentials"</h4>
                        // Shared credentials toggle
                        <label class="flex items-center gap-2 cursor-pointer text-xs text-muted-foreground">
                            <input
                                type="checkbox"
                                class="h-4 w-4 rounded-md border-input"
                                prop:checked=move || cfg_shared_credentials.get()
                                on:change=move |ev| {
                                    set_cfg_shared_credentials.set(event_target_checked(&ev));
                                }
                            />
                            "Shared credentials (all users)"
                        </label>
                    </div>

                    <Show when=move || !cfg_shared_credentials.get()>
                        {move || {
                            let t2 = ds_type.get();
                            let sf2 = sf_auth_mode.get();

                            if t2 == "databricks" {
                                return view! {
                                    <div>
                                        <label class="block text-sm font-medium mb-1">
                                            "Personal Access Token " <span class="text-error-foreground">"*"</span>
                                        </label>
                                        <input type="password" class=MODAL_INPUT_CLASS
                                            placeholder="dapi..."
                                            prop:value=move || cred_access_token.get()
                                            on:input=move |ev| set_cred_access_token.set(event_target_value(&ev))
                                        />
                                        <p class="text-xs text-muted-foreground mt-1">
                                            "Credentials are encrypted at rest"
                                        </p>
                                    </div>
                                }.into_any();
                            }

                            if t2 == "snowflake" && sf2 == "keypair" {
                                return view! {
                                    <div class="space-y-3">
                                        <div class="grid grid-cols-1 sm:grid-cols-2 gap-4">
                                            <div>
                                                <label class="block text-sm font-medium mb-1">
                                                    "Username " <span class="text-error-foreground">"*"</span>
                                                </label>
                                                <input type="text" class=MODAL_INPUT_CLASS
                                                    placeholder="your_username"
                                                    prop:value=move || cred_username.get()
                                                    on:input=move |ev| set_cred_username.set(event_target_value(&ev))
                                                />
                                            </div>
                                            <div>
                                                <label class="block text-sm font-medium mb-1">"Private Key Passphrase"</label>
                                                <input type="password" class=MODAL_INPUT_CLASS
                                                    placeholder="If encrypted"
                                                    prop:value=move || cred_password.get()
                                                    on:input=move |ev| set_cred_password.set(event_target_value(&ev))
                                                />
                                            </div>
                                        </div>
                                        <div>
                                            <label class="block text-sm font-medium mb-1">
                                                "Private Key (PEM) " <span class="text-error-foreground">"*"</span>
                                            </label>
                                            <textarea
                                                rows="4"
                                                class="w-full px-3 py-2 border border-input rounded-md bg-background text-sm font-mono focus:outline-none focus:ring-1 focus:ring-ring"
                                                placeholder="-----BEGIN PRIVATE KEY-----\n...\n-----END PRIVATE KEY-----"
                                                prop:value=move || cred_private_key.get()
                                                on:input=move |ev| set_cred_private_key.set(event_target_value(&ev))
                                            />
                                            <p class="text-xs text-muted-foreground mt-1">
                                                "Credentials are encrypted at rest"
                                            </p>
                                        </div>
                                    </div>
                                }.into_any();
                            }

                            // Default: username + password
                            view! {
                                <div class="grid grid-cols-1 sm:grid-cols-2 gap-4">
                                    <div>
                                        <label class="block text-sm font-medium mb-1">
                                            "Username " <span class="text-error-foreground">"*"</span>
                                        </label>
                                        <input type="text" class=MODAL_INPUT_CLASS
                                            placeholder="Database username"
                                            prop:value=move || cred_username.get()
                                            on:input=move |ev| set_cred_username.set(event_target_value(&ev))
                                        />
                                    </div>
                                    <div>
                                        <label class="block text-sm font-medium mb-1">
                                            "Password " <span class="text-error-foreground">"*"</span>
                                        </label>
                                        <input type="password" class=MODAL_INPUT_CLASS
                                            placeholder="••••••••"
                                            prop:value=move || cred_password.get()
                                            on:input=move |ev| set_cred_password.set(event_target_value(&ev))
                                        />
                                        <p class="text-xs text-muted-foreground mt-1">
                                            "Credentials are encrypted at rest"
                                        </p>
                                    </div>
                                </div>
                            }.into_any()
                        }}
                    </Show>

                    <Show when=move || cfg_shared_credentials.get()>
                        <div class="flex items-center gap-2 p-3 bg-muted/50 rounded-lg">
                            <Icon icon=phosphor_leptos::LOCK attr:class="h-4 w-4 text-muted-foreground"/>
                            <span class="text-sm text-muted-foreground">
                                "Shared credentials — all users connect with the same account"
                            </span>
                        </div>
                    </Show>
                </div>
            }.into_any()
        }}
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Discovery Fields
// ─────────────────────────────────────────────────────────────────────────────

/// Renders the discovery fields (database/schema/warehouse/catalog) for the active provider.
/// Bundle of every signal `DiscoveryFields` needs.
#[derive(Clone, Copy)]
struct DiscoveryFieldsSignals {
    ds_type: ReadSignal<String>,
    discovery_succeeded: Signal<bool>,
    discovered_databases: ReadSignal<Vec<String>>,
    discovered_schemas: ReadSignal<Vec<String>>,
    discovered_warehouses: ReadSignal<Vec<String>>,
    discovered_catalogs: ReadSignal<Vec<String>>,
    cfg_database: ReadSignal<String>,
    set_cfg_database: WriteSignal<String>,
    cfg_schema: ReadSignal<String>,
    set_cfg_schema: WriteSignal<String>,
    cfg_warehouse: ReadSignal<String>,
    set_cfg_warehouse: WriteSignal<String>,
    cfg_catalog: ReadSignal<String>,
    set_cfg_catalog: WriteSignal<String>,
    cfg_role: ReadSignal<String>,
    set_cfg_role: WriteSignal<String>,
}

/// In create mode after successful Test & Discover, shows dropdowns.
/// In edit mode, shows text inputs pre-filled from saved config.
#[component]
fn DiscoveryFields(signals: DiscoveryFieldsSignals) -> impl IntoView {
    let DiscoveryFieldsSignals {
        ds_type,
        discovery_succeeded,
        discovered_databases,
        discovered_schemas,
        discovered_warehouses,
        discovered_catalogs,
        cfg_database,
        set_cfg_database,
        cfg_schema,
        set_cfg_schema,
        cfg_warehouse,
        set_cfg_warehouse,
        cfg_catalog,
        set_cfg_catalog,
        cfg_role,
        set_cfg_role,
    } = signals;
    view! {
        <div class="border-t border-border pt-4 mt-4">
            <h4 class="text-sm font-medium mb-3">
                {move || if discovery_succeeded.get() { "Select Resources" } else { "Resource Configuration" }}
            </h4>

            {move || {
                let t = ds_type.get();
                let succeeded = discovery_succeeded.get();

                match t.as_str() {
                    "postgres" | "redshift" => view! {
                        <div class="grid grid-cols-1 sm:grid-cols-2 gap-4">
                            <div>
                                <label class="block text-sm font-medium mb-1">
                                    "Database " <span class="text-error-foreground">"*"</span>
                                </label>
                                {if succeeded && !discovered_databases.get().is_empty() {
                                    view! {
                                        <DynSelect
                                            value=Signal::derive(move || cfg_database.get())
                                            options=Signal::derive(move || {
                                                discovered_databases.get().into_iter()
                                                    .map(|db| (db.clone(), db))
                                                    .collect()
                                            })
                                            on_change=move |val| set_cfg_database.set(val)
                                            placeholder="Select database..."
                                        />
                                    }.into_any()
                                } else {
                                    view! {
                                        <input type="text" class=MODAL_INPUT_CLASS
                                            placeholder="mydb"
                                            prop:value=move || cfg_database.get()
                                            on:input=move |ev| set_cfg_database.set(event_target_value(&ev))
                                        />
                                    }.into_any()
                                }}
                            </div>
                            <div>
                                <label class="block text-sm font-medium mb-1">"Default Schema"</label>
                                {if succeeded && !discovered_schemas.get().is_empty() {
                                    view! {
                                        <DynSelect
                                            value=Signal::derive(move || cfg_schema.get())
                                            options=Signal::derive(move || {
                                                discovered_schemas.get().into_iter()
                                                    .map(|s| (s.clone(), s))
                                                    .collect()
                                            })
                                            on_change=move |val| set_cfg_schema.set(val)
                                            placeholder="Select schema..."
                                        />
                                    }.into_any()
                                } else {
                                    view! {
                                        <input type="text" class=MODAL_INPUT_CLASS
                                            placeholder="public"
                                            prop:value=move || cfg_schema.get()
                                            on:input=move |ev| set_cfg_schema.set(event_target_value(&ev))
                                        />
                                    }.into_any()
                                }}
                                <p class="text-xs text-muted-foreground mt-1">
                                    "Default schema for queries (usually \"public\")"
                                </p>
                            </div>
                        </div>
                    }.into_any(),

                    "clickhouse" | "mysql" => view! {
                        <div>
                            <label class="block text-sm font-medium mb-1">
                                "Default Database " <span class="text-error-foreground">"*"</span>
                            </label>
                            {if succeeded && !discovered_databases.get().is_empty() {
                                view! {
                                    <DynSelect
                                        value=Signal::derive(move || cfg_database.get())
                                        options=Signal::derive(move || {
                                            discovered_databases.get().into_iter()
                                                .map(|db| (db.clone(), db))
                                                .collect()
                                        })
                                        on_change=move |val| set_cfg_database.set(val)
                                        placeholder="Select database..."
                                    />
                                }.into_any()
                            } else {
                                view! {
                                    <input type="text" class=MODAL_INPUT_CLASS
                                        placeholder="default"
                                        prop:value=move || cfg_database.get()
                                        on:input=move |ev| set_cfg_database.set(event_target_value(&ev))
                                    />
                                }.into_any()
                            }}
                            <p class="text-xs text-muted-foreground mt-1">"Default database for queries"</p>
                        </div>
                    }.into_any(),

                    "snowflake" => view! {
                        <div class="grid grid-cols-1 sm:grid-cols-2 gap-4">
                            <div>
                                <label class="block text-sm font-medium mb-1">"Warehouse"</label>
                                {if succeeded && !discovered_warehouses.get().is_empty() {
                                    view! {
                                        <DynSelect
                                            value=Signal::derive(move || cfg_warehouse.get())
                                            options=Signal::derive(move || {
                                                discovered_warehouses.get().into_iter()
                                                    .map(|w| (w.clone(), w))
                                                    .collect()
                                            })
                                            on_change=move |val| set_cfg_warehouse.set(val)
                                            placeholder="Select warehouse..."
                                        />
                                    }.into_any()
                                } else {
                                    view! {
                                        <input type="text" class=MODAL_INPUT_CLASS
                                            placeholder="COMPUTE_WH"
                                            prop:value=move || cfg_warehouse.get()
                                            on:input=move |ev| set_cfg_warehouse.set(event_target_value(&ev))
                                        />
                                    }.into_any()
                                }}
                                <p class="text-xs text-muted-foreground mt-1">"Default warehouse for compute"</p>
                            </div>
                            <div>
                                <label class="block text-sm font-medium mb-1">"Default Database"</label>
                                {if succeeded && !discovered_databases.get().is_empty() {
                                    view! {
                                        <DynSelect
                                            value=Signal::derive(move || cfg_database.get())
                                            options=Signal::derive(move || {
                                                discovered_databases.get().into_iter()
                                                    .map(|db| (db.clone(), db))
                                                    .collect()
                                            })
                                            on_change=move |val| set_cfg_database.set(val)
                                            placeholder="Select database..."
                                        />
                                    }.into_any()
                                } else {
                                    view! {
                                        <input type="text" class=MODAL_INPUT_CLASS
                                            placeholder="MY_DATABASE"
                                            prop:value=move || cfg_database.get()
                                            on:input=move |ev| set_cfg_database.set(event_target_value(&ev))
                                        />
                                    }.into_any()
                                }}
                            </div>
                            <div>
                                <label class="block text-sm font-medium mb-1">"Default Schema"</label>
                                <input type="text" class=MODAL_INPUT_CLASS
                                    placeholder="PUBLIC"
                                    prop:value=move || cfg_schema.get()
                                    on:input=move |ev| set_cfg_schema.set(event_target_value(&ev))
                                />
                            </div>
                            <div>
                                <label class="block text-sm font-medium mb-1">"Role"</label>
                                <input type="text" class=MODAL_INPUT_CLASS
                                    placeholder="ACCOUNTADMIN"
                                    prop:value=move || cfg_role.get()
                                    on:input=move |ev| set_cfg_role.set(event_target_value(&ev))
                                />
                                <p class="text-xs text-muted-foreground mt-1">
                                    "Snowflake role to use (leave empty for default)"
                                </p>
                            </div>
                        </div>
                    }.into_any(),

                    "databricks" => view! {
                        <div class="grid grid-cols-1 sm:grid-cols-2 gap-4">
                            <div>
                                <label class="block text-sm font-medium mb-1">"Catalog"</label>
                                {if succeeded && !discovered_catalogs.get().is_empty() {
                                    view! {
                                        <DynSelect
                                            value=Signal::derive(move || cfg_catalog.get())
                                            options=Signal::derive(move || {
                                                discovered_catalogs.get().into_iter()
                                                    .map(|c| (c.clone(), c))
                                                    .collect()
                                            })
                                            on_change=move |val| set_cfg_catalog.set(val)
                                            placeholder="Select catalog..."
                                        />
                                    }.into_any()
                                } else {
                                    view! {
                                        <input type="text" class=MODAL_INPUT_CLASS
                                            placeholder="hive_metastore"
                                            prop:value=move || cfg_catalog.get()
                                            on:input=move |ev| set_cfg_catalog.set(event_target_value(&ev))
                                        />
                                    }.into_any()
                                }}
                                <p class="text-xs text-muted-foreground mt-1">"Unity Catalog or hive_metastore"</p>
                            </div>
                            <div>
                                <label class="block text-sm font-medium mb-1">"Default Schema"</label>
                                <input type="text" class=MODAL_INPUT_CLASS
                                    placeholder="default"
                                    prop:value=move || cfg_schema.get()
                                    on:input=move |ev| set_cfg_schema.set(event_target_value(&ev))
                                />
                            </div>
                        </div>
                    }.into_any(),

                    "sqlserver" | "synapse" => view! {
                        <div class="grid grid-cols-1 sm:grid-cols-2 gap-4">
                            <div>
                                <label class="block text-sm font-medium mb-1">
                                    "Database " <span class="text-error-foreground">"*"</span>
                                </label>
                                {if succeeded && !discovered_databases.get().is_empty() {
                                    view! {
                                        <DynSelect
                                            value=Signal::derive(move || cfg_database.get())
                                            options=Signal::derive(move || {
                                                discovered_databases.get().into_iter()
                                                    .map(|db| (db.clone(), db))
                                                    .collect()
                                            })
                                            on_change=move |val| set_cfg_database.set(val)
                                            placeholder="Select database..."
                                        />
                                    }.into_any()
                                } else {
                                    view! {
                                        <input type="text" class=MODAL_INPUT_CLASS
                                            placeholder="master"
                                            prop:value=move || cfg_database.get()
                                            on:input=move |ev| set_cfg_database.set(event_target_value(&ev))
                                        />
                                    }.into_any()
                                }}
                            </div>
                            <div>
                                <label class="block text-sm font-medium mb-1">"Default Schema"</label>
                                {if succeeded && !discovered_schemas.get().is_empty() {
                                    view! {
                                        <DynSelect
                                            value=Signal::derive(move || cfg_schema.get())
                                            options=Signal::derive(move || {
                                                discovered_schemas.get().into_iter()
                                                    .map(|s| (s.clone(), s))
                                                    .collect()
                                            })
                                            on_change=move |val| set_cfg_schema.set(val)
                                            placeholder="Select schema..."
                                        />
                                    }.into_any()
                                } else {
                                    view! {
                                        <input type="text" class=MODAL_INPUT_CLASS
                                            placeholder="dbo"
                                            prop:value=move || cfg_schema.get()
                                            on:input=move |ev| set_cfg_schema.set(event_target_value(&ev))
                                        />
                                    }.into_any()
                                }}
                                <p class="text-xs text-muted-foreground mt-1">
                                    "Default schema for queries (usually \"dbo\")"
                                </p>
                            </div>
                        </div>
                    }.into_any(),

                    _ => view! { <div></div> }.into_any(),
                }
            }}
        </div>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Edit-Mode Catalog Tab
// ─────────────────────────────────────────────────────────────────────────────

/// Edit-mode catalog tab — stats card, Refresh Now button, and schema/database
/// picker.
///
/// Replaces `apps/frontend/src/components/settings/CatalogSection.jsx`.
#[component]
fn EditModeCatalogTab(
    /// The datasource UUID (used to load stats via `get_catalog_stats`).
    datasource_id: Signal<String>,
    /// The datasource slug (used to trigger a refresh via `refresh_catalog`).
    datasource_slug: Signal<String>,
    /// The datasource type string (e.g. `"bigquery"`, `"postgres"`).
    datasource_type: Signal<String>,
    /// Current `connection_config` from the modal (used for discovery requests).
    connection_config: Signal<serde_json::Value>,
    /// Current credentials from the modal (used for discovery requests).
    credentials: Signal<serde_json::Value>,
    /// Whether this is a sample datasource (read-only).
    is_sample: ReadSignal<bool>,
    /// Selected catalog scope items (projects / databases / schemas / catalogs).
    catalog_selected: ReadSignal<Vec<String>>,
    set_catalog_selected: WriteSignal<Vec<String>>,
    /// BigQuery only: include public datasets.
    bq_include_public: ReadSignal<bool>,
    set_bq_include_public: WriteSignal<bool>,
) -> impl IntoView {
    // ── Load catalog stats on mount ──────────────────────────────────────
    // Input: datasource_id
    let stats_action = Action::new(|ds_id: &String| {
        let ds_id = ds_id.clone();
        async move { get_catalog_stats(ds_id).await }
    });

    // Dispatch stats load once when the component first mounts.
    Effect::new(move |_| {
        let id = datasource_id.get();
        if !id.is_empty() {
            stats_action.dispatch(id);
        }
    });

    // Derived stats from the action result.
    let stats = Signal::derive(move || {
        stats_action.value().get().and_then(|r| r.ok())
    });

    // ── Refresh catalog ──────────────────────────────────────────────────
    // Input: datasource_slug — returned through result so the Effect can
    // distinguish which invocation finished (double-dispatch guard uses
    // `pending()`).
    let refresh_action = Action::new(|slug: &String| {
        let slug = slug.clone();
        async move {
            refresh_catalog(slug).await
        }
    });

    // After a successful refresh, reload stats to show updated counts.
    Effect::new(move |_| {
        if let Some(result) = refresh_action.value().get() {
            match result {
                Ok(_msg) => {
                    // Reload stats — re-dispatch the stats action.
                    let id = datasource_id.get_untracked();
                    if !id.is_empty() {
                        stats_action.dispatch(id);
                    }
                }
                Err(e) => {
                    leptos::logging::error!("Catalog refresh failed: {e}");
                }
            }
        }
    });

    let on_refresh_click = move |_: leptos::ev::MouseEvent| {
        if refresh_action.pending().get_untracked() {
            return;
        }
        let slug = datasource_slug.get_untracked();
        if !slug.is_empty() {
            refresh_action.dispatch(slug);
        }
    };

    // ── Discover resources ───────────────────────────────────────────────
    // Input: (ds_type, conn_cfg, creds, slug_opt)
    type DiscoverInput = (String, serde_json::Value, serde_json::Value, Option<String>);

    let discover_action = Action::new(|input: &DiscoverInput| {
        let (ds_type_val, conn_cfg, creds, slug_opt) = input.clone();
        async move {
            discover_datasource_resources(ds_type_val, conn_cfg, creds, slug_opt).await
        }
    });

    // "idle" | "loading" | "success" | "error"
    let (discover_status, set_discover_status) = signal("idle".to_string());
    let (discover_error, set_discover_error) = signal::<Option<String>>(None);
    let (discovered_items, set_discovered_items) = signal::<Vec<String>>(vec![]);

    Effect::new(move |_| {
        if let Some(result) = discover_action.value().get() {
            match result {
                Ok(r) if r.success => {
                    set_discover_status.set("success".to_string());
                    set_discover_error.set(None);
                    // Extract the items relevant to this datasource type.
                    let ds_type_val = datasource_type.get_untracked();
                    let key = discovery_resource_key_for_type(&ds_type_val);
                    let items = r.resources.get(key).cloned().unwrap_or_default();
                    set_discovered_items.set(items);
                }
                Ok(r) => {
                    set_discover_status.set("error".to_string());
                    set_discover_error.set(Some(r.message));
                    set_discovered_items.set(vec![]);
                }
                Err(e) => {
                    set_discover_status.set("error".to_string());
                    set_discover_error.set(Some(e.to_string()));
                    set_discovered_items.set(vec![]);
                }
            }
        }
    });

    let on_discover_click = move |_: leptos::ev::MouseEvent| {
        if discover_action.pending().get_untracked() {
            return;
        }
        set_discover_status.set("loading".to_string());
        set_discover_error.set(None);
        set_discovered_items.set(vec![]);

        let ds_type_val = datasource_type.get_untracked();
        let conn_cfg = connection_config.get_untracked();
        let creds = credentials.get_untracked();
        let slug = datasource_slug.get_untracked();
        let slug_opt = if slug.is_empty() { None } else { Some(slug) };

        discover_action.dispatch((ds_type_val, conn_cfg, creds, slug_opt));
    };

    // ── Text input for manual item entry ──────────────────────────────────
    let (new_item_input, set_new_item_input) = signal(String::new());

    let on_add_item = move || {
        let val = new_item_input.get_untracked().trim().to_string();
        if val.is_empty() {
            return;
        }
        let mut selected = catalog_selected.get_untracked();
        if selected.contains(&val) {
            return;
        }
        selected.push(val);
        set_catalog_selected.set(selected);
        set_new_item_input.set(String::new());
    };

    // ── Relative time formatter ───────────────────────────────────────────
    let format_relative = |iso: &str| -> String {
        // Parse the RFC3339 timestamp and compute a relative description.
        use std::str::FromStr as _;
        let Ok(dt) = chrono::DateTime::<chrono::Utc>::from_str(iso) else {
            return iso.to_string();
        };
        let now = chrono::Utc::now();
        let diff = now.signed_duration_since(dt);
        let secs = diff.num_seconds();
        if secs < 60 {
            "just now".to_string()
        } else if secs < 3600 {
            let m = secs / 60;
            format!("{m} minute{} ago", if m == 1 { "" } else { "s" })
        } else if secs < 86400 {
            let h = secs / 3600;
            format!("{h} hour{} ago", if h == 1 { "" } else { "s" })
        } else {
            let d = secs / 86400;
            format!("{d} day{} ago", if d == 1 { "" } else { "s" })
        }
    };

    view! {
        <div class="space-y-6">

            // ── Stats card ──────────────────────────────────────────────────
            <div class="rounded-lg border border-border bg-card">
                // Card header with title + refresh button
                <div class="flex items-center justify-between px-4 py-3 border-b border-border">
                    <div class="flex items-center gap-2">
                        <span class="h-4 w-4 text-muted-foreground inline-flex items-center justify-center">
                            <Icon icon=phosphor_leptos::STACK/>
                        </span>
                        <span class="text-sm font-medium text-foreground">"Data Catalog"</span>
                    </div>
                    <Show when=move || !is_sample.get()>
                        <Button
                            variant=ButtonVariant::Outline
                            size=ButtonSize::Sm
                            disabled=Signal::derive(move || refresh_action.pending().get())
                            on:click=on_refresh_click
                        >
                            <span class="h-4 w-4 inline-flex items-center justify-center">
                                <Icon icon=phosphor_leptos::ARROWS_CLOCKWISE/>
                            </span>
                            {move || if refresh_action.pending().get() {
                                "Refreshing..."
                            } else {
                                "Refresh Now"
                            }}
                        </Button>
                    </Show>
                </div>

                // Stats grid
                <div class="p-4">
                    {move || {
                        if stats_action.pending().get() && stats.get().is_none() {
                            view! {
                                <div class="grid grid-cols-3 gap-3">
                                    <Skeleton class="h-14 w-full"/>
                                    <Skeleton class="h-14 w-full"/>
                                    <Skeleton class="h-14 w-full"/>
                                </div>
                            }.into_any()
                        } else {
                            let table_count = stats.get().map(|s| s.table_count).unwrap_or(0);
                            let schema_count = stats.get().map(|s| s.schema_count).unwrap_or(0);
                            let last_indexed_str = stats
                                .get()
                                .and_then(|s| s.last_indexed)
                                .map(|iso| format_relative(&iso))
                                .unwrap_or_else(|| "Never".to_string());
                            let ds_type_val = datasource_type.get();
                            let schema_label = match ds_type_val.as_str() {
                                "bigquery" => "Datasets",
                                _ => "Schemas",
                            };
                            view! {
                                <div class="grid grid-cols-3 gap-3">
                                    <div class="flex flex-col gap-0.5 p-3 rounded-md bg-muted/40">
                                        <span class="text-lg font-semibold font-data text-foreground">
                                            {table_count.to_string()}
                                        </span>
                                        <span class="text-xs text-muted-foreground">"Tables indexed"</span>
                                    </div>
                                    <div class="flex flex-col gap-0.5 p-3 rounded-md bg-muted/40">
                                        <span class="text-lg font-semibold font-data text-foreground">
                                            {schema_count.to_string()}
                                        </span>
                                        <span class="text-xs text-muted-foreground">{schema_label}</span>
                                    </div>
                                    <div class="flex flex-col gap-0.5 p-3 rounded-md bg-muted/40">
                                        <span class="text-sm font-medium font-data text-foreground truncate">
                                            {last_indexed_str}
                                        </span>
                                        <span class="text-xs text-muted-foreground">"Last indexed"</span>
                                    </div>
                                </div>
                            }.into_any()
                        }
                    }}

                    // Refresh error
                    {move || refresh_action.value().get().and_then(|r| r.err()).map(|e| view! {
                        <Alert variant=AlertVariant::Error class="mt-3">
                            <AlertDescription>{e.to_string()}</AlertDescription>
                        </Alert>
                    })}
                </div>
            </div>

            // ── Schema/catalog picker (admin only, not for sample) ──────────
            <Show when=move || !is_sample.get()>
                <div class="space-y-3">
                    {move || {
                        let ds_type_val = datasource_type.get();
                        let item_label = catalog_item_label_for_type(&ds_type_val);
                        let config_label = match ds_type_val.as_str() {
                            "bigquery" => "Projects to Index",
                            "clickhouse" | "mysql" | "snowflake" => "Databases to Index",
                            "databricks" => "Catalogs to Index",
                            _ => "Schemas to Index",
                        };

                        view! {
                            <div>
                                <h4 class="text-sm font-medium text-foreground mb-1">
                                    {config_label}
                                </h4>
                                <p class="text-xs text-muted-foreground mb-3">
                                    "Select which "
                                    {item_label}
                                    " to include in catalog indexing. Leave empty to index all available "
                                    {item_label}
                                    "."
                                </p>
                            </div>
                        }
                    }}

                    // BigQuery: Include Public Datasets toggle
                    <Show when=move || datasource_type.get() == "bigquery">
                        <label class="flex items-center justify-between p-3 rounded-lg border border-border bg-muted/30 cursor-pointer">
                            <div>
                                <span class="text-sm font-medium text-foreground block">
                                    "Include Public Datasets"
                                </span>
                                <span class="text-xs text-muted-foreground">
                                    "Show BigQuery public datasets in search results"
                                </span>
                            </div>
                            <Switch
                                checked=Signal::from(bq_include_public)
                                on_change=Callback::new(move |val: bool| set_bq_include_public.set(val))
                            />
                        </label>
                    </Show>

                    // Discover Available button
                    <div class="flex items-center gap-2">
                        <Button
                            variant=ButtonVariant::Outline
                            size=ButtonSize::Sm
                            disabled=Signal::derive(move || discover_action.pending().get())
                            on:click=on_discover_click
                        >
                            <span class="h-4 w-4 inline-flex items-center justify-center">
                                <Icon icon=phosphor_leptos::MAGNIFYING_GLASS/>
                            </span>
                            {move || if discover_action.pending().get() {
                                "Discovering..."
                            } else {
                                "Discover Available"
                            }}
                        </Button>
                        {move || if discover_status.get() == "success" && !discovered_items.get().is_empty() {
                            let count = discovered_items.get().len();
                            Some(view! {
                                <span class="text-xs text-muted-foreground">
                                    {format!("{count} found")}
                                </span>
                            })
                        } else {
                            None
                        }}
                    </div>

                    // Discovery error
                    {move || discover_error.get().filter(|_| discover_status.get() == "error").map(|msg| view! {
                        <Alert variant=AlertVariant::Warning class="mt-1">
                            <AlertDescription>{msg}</AlertDescription>
                        </Alert>
                    })}

                    // Discovered items: checkbox list
                    <Show when=move || {
                        discover_status.get() == "success" && !discovered_items.get().is_empty()
                    }>
                        <div class="space-y-2">
                            // Select all / Clear buttons + count
                            <div class="flex items-center gap-2">
                                <button
                                    type="button"
                                    class="text-xs text-primary hover:underline"
                                    on:click=move |_| {
                                        set_catalog_selected.set(discovered_items.get_untracked());
                                    }
                                >
                                    "Select all"
                                </button>
                                <span class="text-xs text-muted-foreground">"·"</span>
                                <button
                                    type="button"
                                    class="text-xs text-primary hover:underline"
                                    on:click=move |_| set_catalog_selected.set(vec![])
                                >
                                    "Clear"
                                </button>
                                <span class="text-xs text-muted-foreground ml-auto">
                                    {move || {
                                        let sel = catalog_selected.get().len();
                                        let total = discovered_items.get().len();
                                        format!("{sel} of {total} selected")
                                    }}
                                </span>
                            </div>
                            // Checkbox list
                            <div class="border border-border rounded-md divide-y divide-border max-h-60 overflow-y-auto">
                                <For
                                    each=move || discovered_items.get()
                                    key=|item| item.clone()
                                    let:item
                                >
                                    {
                                        let item_for_change = item.clone();
                                        let item_for_check = item.clone();
                                        view! {
                                            <label class="flex items-center gap-3 px-3 py-2 cursor-pointer hover:bg-muted/40 transition-colors">
                                                <input
                                                    type="checkbox"
                                                    class="h-4 w-4 rounded border-input accent-primary"
                                                    prop:checked=move || catalog_selected.get().contains(&item_for_check)
                                                    on:change=move |ev| {
                                                        let checked = event_target_checked(&ev);
                                                        let val = item_for_change.clone();
                                                        set_catalog_selected.update(|list| {
                                                            if checked {
                                                                if !list.contains(&val) {
                                                                    list.push(val);
                                                                }
                                                            } else {
                                                                list.retain(|i| i != &val);
                                                            }
                                                        });
                                                    }
                                                />
                                                <span class="text-sm font-mono text-foreground">
                                                    {item.clone()}
                                                </span>
                                            </label>
                                        }
                                    }
                                </For>
                            </div>
                        </div>
                    </Show>

                    // Currently selected items (chip list)
                    <Show when=move || !catalog_selected.get().is_empty()>
                        <div class="space-y-1">
                            <p class="text-xs text-muted-foreground font-medium">"Currently selected:"</p>
                            <div class="flex flex-wrap gap-1.5">
                                <For
                                    each=move || catalog_selected.get()
                                    key=|item| item.clone()
                                    let:item
                                >
                                    {
                                        let item_for_remove = item.clone();
                                        view! {
                                            <span class="inline-flex items-center gap-1 px-2 py-0.5 rounded-full border border-border bg-muted/40 text-xs font-mono text-foreground">
                                                {item.clone()}
                                                <button
                                                    type="button"
                                                    class="h-3.5 w-3.5 inline-flex items-center justify-center text-muted-foreground hover:text-foreground transition-colors"
                                                    on:click=move |_| {
                                                        let val = item_for_remove.clone();
                                                        set_catalog_selected.update(|list| {
                                                            list.retain(|i| i != &val);
                                                        });
                                                    }
                                                >
                                                    <Icon icon=phosphor_leptos::X size="10px"/>
                                                </button>
                                            </span>
                                        }
                                    }
                                </For>
                            </div>
                        </div>
                    </Show>

                    // Manual text input (always shown as fallback / supplement)
                    <Show when=move || {
                        discover_status.get() != "success" || discovered_items.get().is_empty()
                    }>
                        <div class="space-y-1.5">
                            <p class="text-xs text-muted-foreground">
                                "Or add items manually:"
                            </p>
                            <div class="flex gap-2">
                                <input
                                    type="text"
                                    class=MODAL_INPUT_CLASS
                                    placeholder=move || {
                                        let ds_type_val = datasource_type.get();
                                        match ds_type_val.as_str() {
                                            "bigquery" => "Enter project ID",
                                            "clickhouse" | "mysql" | "snowflake" => "Enter database name",
                                            "databricks" => "Enter catalog name (e.g. main)",
                                            _ => "Enter schema name (e.g. public)",
                                        }
                                    }
                                    prop:value=move || new_item_input.get()
                                    on:input=move |ev| set_new_item_input.set(event_target_value(&ev))
                                    on:keydown=move |ev| {
                                        if ev.key() == "Enter" {
                                            ev.prevent_default();
                                            on_add_item();
                                        }
                                    }
                                />
                                <Button
                                    variant=ButtonVariant::Outline
                                    size=ButtonSize::Sm
                                    disabled=Signal::derive(move || new_item_input.get().trim().is_empty())
                                    on:click=move |_| on_add_item()
                                >
                                    <span class="h-4 w-4 inline-flex items-center justify-center">
                                        <Icon icon=phosphor_leptos::PLUS/>
                                    </span>
                                </Button>
                            </div>
                        </div>
                    </Show>

                    <p class="text-xs text-muted-foreground">
                        "Changes to the catalog scope take effect on the next refresh."
                    </p>
                </div>
            </Show>
        </div>
    }
}
