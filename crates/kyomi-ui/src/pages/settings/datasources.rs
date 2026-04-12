// SPDX-License-Identifier: AGPL-3.0-or-later

//! Data Sources settings page — list, toggle, delete, create, and edit datasources.
//!
//! Replaces `apps/frontend/src/components/settings/DatasourceSettings.jsx` and
//! `apps/frontend/src/components/settings/DatasourceModal.jsx`.

use leptos::prelude::*;
use leptos_icons::Icon;

use crate::components::{
    Alert, AlertDescription, AlertVariant, Badge, BadgeVariant, Button, ButtonSize, ButtonVariant,
    Card, ConfirmDialog, EmptyState, Modal, ModalSize, Skeleton, Switch,
};
use crate::components::DynSelect;
use crate::server_fns::datasources::*;
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

// ─────────────────────────────────────────────────────────────────────────────
// Main Page
// ─────────────────────────────────────────────────────────────────────────────

/// Data Sources settings page content.
#[component]
pub fn DatasourcesPage() -> impl IntoView {
    let datasources_resource = Resource::new(|| (), |_| list_datasources());

    view! {
        <Transition fallback=move || view! { <DatasourcesLoadingSkeleton/> }>
            {move || Suspend::new(async move {
                match datasources_resource.await {
                    Ok(datasources) => {
                        view! {
                            <DatasourcesContent
                                initial_datasources=datasources
                                datasources_resource=datasources_resource
                            />
                        }.into_any()
                    }
                    Err(e) => view! {
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
            })}
        </Transition>
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
    datasources_resource: Resource<Result<Vec<DatasourceInfo>, ServerFnError>>,
) -> impl IntoView {
    let (datasources, set_datasources) = signal(initial_datasources);

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
        // Refresh the datasource list
        datasources_resource.refetch();
    });

    // When resource refreshes, update local list
    Effect::new(move |_| {
        if let Some(Ok(list)) = datasources_resource.get() {
            set_datasources.set(list);
        }
    });

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
                        set_datasources.update(|list| {
                            list.retain(|d| d.id != ds.id);
                        });
                    }
                    Err(e) => {
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
                <Button
                    on:click=move |_| set_modal_datasource_id.set(Some(None))
                >
                    <span class="h-4 w-4 inline-flex items-center justify-center">
                        <Icon icon=icondata_lu::LuPlus/>
                    </span>
                    "Add Datasource"
                </Button>
            </div>

            // Datasources List
            <Card>
                <div class="p-0">
                    <Show
                        when=move || !datasources.get().is_empty()
                        fallback=move || view! {
                            <EmptyState
                                icon=std::sync::Arc::new(|| view! {
                                    <Icon icon=icondata_lu::LuDatabase width="48" height="48"/>
                                }.into_any())
                                title="No datasources configured"
                                description="Connect a data source to start querying your data"
                                action=std::sync::Arc::new(move || view! {
                                    <Button on:click=move |_| set_modal_datasource_id.set(Some(None))>
                                        <span class="h-4 w-4 inline-flex items-center justify-center">
                                            <Icon icon=icondata_lu::LuPlus/>
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
                    set_datasources.update(|list| {
                        if let Some(d) = list.iter_mut().find(|d| d.id == ds_id) {
                            d.user_enabled = new_val;
                        }
                    });
                }
                Err(e) => {
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

                // Settings button — opens modal
                <Button
                    variant=ButtonVariant::Outline
                    size=ButtonSize::Sm
                    on:click=on_settings_click
                >
                    <span class="h-4 w-4 sm:mr-1 inline-flex items-center justify-center">
                        <Icon icon=icondata_lu::LuSettings/>
                    </span>
                    <span class="hidden sm:inline">"Settings"</span>
                </Button>

                // Delete button
                <button
                    class="inline-flex items-center justify-center gap-2 whitespace-nowrap rounded-md text-sm font-medium transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring h-8 rounded-md px-3 text-xs text-foreground hover:bg-secondary hover:text-accent-foreground"
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
    let (saving, set_saving) = signal(false);
    let (testing, set_testing) = signal(false);
    let (test_result, set_test_result) = signal::<Option<TestConnectionResult>>(None);
    let (settings_loading, set_settings_loading) = signal(false);
    let (error_msg, set_error_msg) = signal::<Option<String>>(None);

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
        // Don't reset sample_available / sample_already_added here — those are
        // refreshed by an async effect when the modal opens in create mode.
        set_creating_sample.set(false);
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
                            set_name.set(settings.name.clone());
                            set_slug.set(settings.slug.clone());
                            set_ds_type.set(settings.datasource_type.clone());

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
                            set_cfg_host.set(str_val("host"));
                            set_cfg_port.set(
                                cfg.get("port")
                                    .and_then(|v| v.as_i64())
                                    .map(|n| n.to_string())
                                    .unwrap_or_default(),
                            );
                            set_cfg_ssl_mode.set(
                                if str_val("ssl_mode").is_empty() {
                                    "require".to_string()
                                } else {
                                    str_val("ssl_mode")
                                }
                            );
                            set_cfg_database.set(str_val("database"));
                            set_cfg_schema.set(str_val("schema"));
                            set_cfg_warehouse.set(str_val("warehouse"));
                            set_cfg_account.set(str_val("account"));
                            set_cfg_role.set(str_val("role"));
                            set_cfg_catalog.set(str_val("catalog"));
                            set_cfg_server_hostname.set(str_val("server_hostname"));
                            set_cfg_http_path.set(str_val("http_path"));
                            set_cfg_secure.set(bool_val("secure"));
                            set_cfg_encrypt.set(
                                cfg.get("encrypt")
                                    .and_then(|v| v.as_bool())
                                    .unwrap_or(true),
                            );
                            set_cfg_trust_cert.set(bool_val("trust_server_certificate"));
                            set_is_sample.set(bool_val("is_sample"));
                            set_cfg_shared_credentials.set(settings.shared_credentials);
                            set_cfg_oauth_client_id.set(str_val("oauth_client_id"));
                            set_cfg_oauth_client_secret.set(str_val("oauth_client_secret"));

                            // BigQuery auth mode
                            if let Some(ref auth_mode) = settings.auth_mode {
                                set_bq_auth_mode.set(auth_mode.clone());
                                set_sf_auth_mode.set(auth_mode.clone());
                            }

                            // Service account
                            if let Some(ref email) = settings.service_account_email {
                                set_service_account_email.set(email.clone());
                            }

                            // Load user settings (masked credentials)
                            let user = &settings.user_settings;
                            let user_str = |key: &str| -> String {
                                user.get(key)
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("")
                                    .to_string()
                            };
                            set_cred_billing_project.set(user_str("billing_project"));
                            set_cred_default_project.set(user_str("default_project"));
                            // Note: passwords are not pre-filled (security)
                        }
                        Err(e) => {
                            set_error_msg.set(Some(format!("Failed to load settings: {e}")));
                        }
                    }
                    set_settings_loading.set(false);
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
                    set_sample_available.set(result.configured && result.is_admin);
                    set_sample_already_added.set(result.already_added);
                }
                Err(_) => {
                    set_sample_available.set(false);
                    set_sample_already_added.set(false);
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
                        set_sample_already_added.set(true);
                    }
                    set_error_msg.set(Some(msg));
                }
            }
            set_creating_sample.set(false);
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
    let do_test_and_discover = move || {
        let ds_type_val = ds_type.get_untracked();
        let conn_cfg = build_connection_config();
        let creds = build_credentials();
        let ds_id = datasource_id.get_untracked();
        let slug_val = slug.get_untracked();

        set_testing.set(true);
        set_test_result.set(None);
        set_discovery_status.set("loading".to_string());
        set_discovery_error.set(None);
        set_discovered_databases.set(vec![]);
        set_discovered_schemas.set(vec![]);
        set_discovered_warehouses.set(vec![]);
        set_discovered_catalogs.set(vec![]);

        leptos::task::spawn_local(async move {
            // In edit mode, use the existing datasource ID for OAuth credential lookup
            let ds_slug = ds_id
                .as_deref()
                .map(|_| slug_val)
                .filter(|s| !s.is_empty());

            let result = discover_datasource_resources(
                ds_type_val,
                conn_cfg,
                creds,
                ds_slug,
            ).await;

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

            set_testing.set(false);
        });
    };

    // ── Save ─────────────────────────────────────────────────────────────
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

        set_saving.set(true);
        set_error_msg.set(None);

        leptos::task::spawn_local(async move {
            let result = match ds_id {
                None => {
                    // Create mode
                    create_datasource_modal(
                        name_val,
                        slug_val,
                        ds_type_val,
                        conn_cfg,
                        creds,
                    ).await
                }
                Some(id) => {
                    // Edit mode — save connection settings first
                    let update_result = update_datasource_settings(
                        id.clone(),
                        name_val,
                        slug_val,
                        conn_cfg,
                    ).await;

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
            };

            match result {
                Ok(saved) => {
                    on_saved.run(saved);
                }
                Err(e) => {
                    set_error_msg.set(Some(e.to_string()));
                }
            }

            set_saving.set(false);
        });
    };

    // ── Derived: is create mode ──────────────────────────────────────────
    let is_create_mode = Signal::derive(move || datasource_id.get().is_none());

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
                        let is_saving = saving.get();
                        let sample = is_sample.get();
                        view! {
                            // Edit mode footer.
                            //
                            // The sample datasource is intentionally read-only:
                            // its connection settings are managed by the server
                            // via `SAMPLE_CLICKHOUSE_*` env vars, so there is
                            // nothing for the user to save. We surface a single
                            // "Close" button (no "Save") to make the read-only
                            // nature explicit, matching the React reference
                            // in `apps/frontend/src/components/settings/
                            // datasources/DatasourceModal.jsx` around the
                            // sample tab render path.
                            <Button
                                variant=ButtonVariant::Outline
                                on:click=move |_| on_close.run(())
                            >
                                {if sample { "Close" } else { "Cancel" }}
                            </Button>
                            <Show when=move || !is_sample.get()>
                                <Button
                                    disabled=is_saving
                                    on:click=move |_| do_save()
                                >
                                    {move || if saving.get() { "Saving..." } else { "Save" }}
                                </Button>
                            </Show>
                        }.into_any()
                    }
                >
                    // Create mode footer
                    {move || {
                        let do_save = do_save;
                        let is_connection_tab = active_tab.get() == "connection";
                        let can_next = test_result.get().map(|r| r.success).unwrap_or(false) && !name.get().is_empty();
                        let is_saving = saving.get();
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
                                        {move || if saving.get() { "Creating..." } else { "Create" }}
                                    </Button>
                                }.into_any()
                            }}
                        }.into_any()
                    }}
                </Show>
            }.into_any()
        });

    view! {
        // Wrap in a reactive block so the modal title updates when name changes
        {move || {
            let title = modal_title.get();
            view! {
                <Modal
                    show=open
                    on_close=on_close
                    title=title
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
                        <div class="flex items-center justify-center py-12">
                            <span class="text-sm text-muted-foreground">"Loading settings..."</span>
                        </div>
                    </Show>

                    <Show when=move || !settings_loading.get()>
                        // Tab bar
                        <Show when=move || is_create_mode.get()>
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

                        // Content area (min-height from React)
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
                                                        <Icon icon=icondata_lu::LuDatabase/>
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
                                                    <Icon icon=icondata_lu::LuDatabase/>
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
                                        ds_type=ds_type
                                        sf_auth_mode=sf_auth_mode
                                        cfg_host=cfg_host
                                        set_cfg_host=set_cfg_host
                                        cfg_port=cfg_port
                                        set_cfg_port=set_cfg_port
                                        cfg_ssl_mode=cfg_ssl_mode
                                        set_cfg_ssl_mode=set_cfg_ssl_mode
                                        cfg_database=cfg_database
                                        set_cfg_database=set_cfg_database
                                        cfg_account=cfg_account
                                        set_cfg_account=set_cfg_account
                                        cfg_server_hostname=cfg_server_hostname
                                        set_cfg_server_hostname=set_cfg_server_hostname
                                        cfg_http_path=cfg_http_path
                                        set_cfg_http_path=set_cfg_http_path
                                        cfg_secure=cfg_secure
                                        set_cfg_secure=set_cfg_secure
                                        cfg_encrypt=cfg_encrypt
                                        set_cfg_encrypt=set_cfg_encrypt
                                        cfg_trust_cert=cfg_trust_cert
                                        set_cfg_trust_cert=set_cfg_trust_cert
                                        cfg_oauth_client_id=cfg_oauth_client_id
                                        set_cfg_oauth_client_id=set_cfg_oauth_client_id
                                        cfg_oauth_client_secret=cfg_oauth_client_secret
                                        set_cfg_oauth_client_secret=set_cfg_oauth_client_secret
                                    />

                                    // Credentials section (non-BigQuery / non-Snowflake-OAuth)
                                    <ProviderCredentialsFields
                                        ds_type=ds_type
                                        sf_auth_mode=sf_auth_mode
                                        bq_auth_mode=bq_auth_mode
                                        cred_username=cred_username
                                        set_cred_username=set_cred_username
                                        cred_password=cred_password
                                        set_cred_password=set_cred_password
                                        cred_access_token=cred_access_token
                                        set_cred_access_token=set_cred_access_token
                                        cred_private_key=cred_private_key
                                        set_cred_private_key=set_cred_private_key
                                        cfg_shared_credentials=cfg_shared_credentials
                                        set_cfg_shared_credentials=set_cfg_shared_credentials
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
                                                    disabled=move || testing.get()
                                                    on:click=move |_| do_test_and_discover()
                                                >
                                                    <span class="h-4 w-4 inline-flex items-center justify-center">
                                                        <Icon icon=icondata_lu::LuPlug/>
                                                    </span>
                                                    {move || if testing.get() { "Discovering..." } else { "Test & Discover" }}
                                                </button>
                                                {move || test_result.get().map(|r| {
                                                    if r.success {
                                                        view! {
                                                            <div class="flex items-center gap-2 text-sm text-success-foreground">
                                                                <Icon icon=icondata_lu::LuCheck attr:class="h-4 w-4"/>
                                                                "Connected"
                                                            </div>
                                                        }.into_any()
                                                    } else {
                                                        view! {
                                                            <div class="flex items-center gap-2 text-sm text-error-foreground">
                                                                <Icon icon=icondata_lu::LuX attr:class="h-4 w-4"/>
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
                                            ds_type=ds_type
                                            discovery_succeeded=discovery_succeeded
                                            discovered_databases=discovered_databases
                                            discovered_schemas=discovered_schemas
                                            discovered_warehouses=discovered_warehouses
                                            discovered_catalogs=discovered_catalogs
                                            cfg_database=cfg_database
                                            set_cfg_database=set_cfg_database
                                            cfg_schema=cfg_schema
                                            set_cfg_schema=set_cfg_schema
                                            cfg_warehouse=cfg_warehouse
                                            set_cfg_warehouse=set_cfg_warehouse
                                            cfg_catalog=cfg_catalog
                                            set_cfg_catalog=set_cfg_catalog
                                            cfg_role=cfg_role
                                            set_cfg_role=set_cfg_role
                                        />
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
                                            <Icon icon=icondata_lu::LuCheck attr:class="h-5 w-5 text-success-foreground"/>
                                            <span class="text-sm font-medium">"Connection verified"</span>
                                        </div>
                                        <p class="text-xs text-muted-foreground mt-1">
                                            "Click Create to add this datasource. The catalog will be indexed automatically."
                                        </p>
                                    </div>
                                </div>
                            </Show>

                        </div>
                    </Show>
                }
            }}
                </Modal>
            }
        }}
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
                        <Icon icon=icondata_lu::LuLink attr:class="h-4 w-4"/>
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
                                    <Icon icon=icondata_lu::LuLink attr:class="h-4 w-4"/>
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
                                    <Icon icon=icondata_lu::LuCheck attr:class="h-4 w-4 text-success-foreground"/>
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

/// Renders the connection fields (host/port/ssl_mode/account/etc.) for the active provider.
/// Also renders the database text input so users can type a database name before Test & Discover.
/// After Test & Discover, DiscoveryFields replaces the text input with a dropdown of real db names.
#[allow(clippy::too_many_arguments)]
#[component]
fn ProviderConnectionFields(
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
) -> impl IntoView {
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

/// Renders credentials fields for the active provider.
/// Skipped for BigQuery (handled in BigQueryAuthModeSection) and Snowflake OAuth.
#[allow(clippy::too_many_arguments)]
#[component]
fn ProviderCredentialsFields(
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
) -> impl IntoView {
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
                            <Icon icon=icondata_lu::LuLock attr:class="h-4 w-4 text-muted-foreground"/>
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
/// In create mode after successful Test & Discover, shows dropdowns.
/// In edit mode, shows text inputs pre-filled from saved config.
#[allow(clippy::too_many_arguments)]
#[component]
fn DiscoveryFields(
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
) -> impl IntoView {
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
