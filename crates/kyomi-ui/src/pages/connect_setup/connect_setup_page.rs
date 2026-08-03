// SPDX-License-Identifier: AGPL-3.0-or-later

//! Connect Setup page — multi-step flow for selecting or creating a
//! Kyomi Connect datasource and delivering the token to the CLI.
//!
//! Replaces `apps/frontend/src/pages/ConnectSetupPage.jsx` (443 lines).
//!
//! ## Flow
//!
//! 1. **Admin check** — non-admins see an "Admin Access Required" message.
//! 2. **Step 1 (Select)** — list existing Connect datasources; click to rotate
//!    token, or "Create new datasource" to move to Step 2.
//! 3. **Step 2 (Create)** — name, optional slug (shown on 409 conflict),
//!    database type grid, submit to create and generate token.
//! 4. **Step 3 (Success)** — token display with copy button. If `callback_port`
//!    and `state` query params are present, delivers the token to the CLI's
//!    local callback server automatically.

use leptos::prelude::*;
use phosphor_leptos::Icon;
use leptos_router::hooks::use_query_map;
use wasm_bindgen::JsCast;

use kyomi_types::Permission;

use crate::components::input::INPUT_CLASS;
use crate::components::{
    Alert, AlertDescription, AlertVariant, Button, ButtonSize, ButtonVariant, Card, Spinner,
};
use crate::server_fns::connect::*;
use crate::server_fns::context::get_user_context;

// ─────────────────────────────────────────────────────────────────────────────
// Constants
// ─────────────────────────────────────────────────────────────────────────────

/// Datasource types that support Kyomi Connect.
/// Matches React's `CONNECT_TYPES` array exactly.
///
/// Shared with `pages::settings::datasources` so the create-mode Connect
/// branch restricts its type selector to the same list without duplicating
/// it — the server-side `create_connect_datasource` server_fn is the other
/// source of truth (it rejects `bigquery`/`snowflake`/`databricks`).
pub const CONNECT_TYPES: &[(&str, &str)] = &[
    ("postgres", "PostgreSQL"),
    ("mysql", "MySQL"),
    ("clickhouse", "ClickHouse"),
    ("sqlserver", "SQL Server"),
    ("redshift", "Redshift"),
];

/// Get the display label for a datasource type.
fn get_datasource_label(ds_type: &str) -> &'static str {
    CONNECT_TYPES
        .iter()
        .find(|(val, _)| *val == ds_type)
        .map(|(_, label)| *label)
        .unwrap_or("Unknown")
}

/// Generate a URL-safe slug from a display name (matches backend logic).
///
/// Rules match `kyomi_auth::datasource_service::generate_slug`:
/// - Lowercase, whitespace/underscores → hyphens
/// - Strip non-alphanumeric (except hyphens), collapse consecutive hyphens
/// - Min 3 chars (append `-db` if too short), max 100 chars
fn generate_slug(name: &str) -> String {
    let mut slug: String = name
        .to_lowercase()
        .replace(|c: char| c.is_whitespace() || c == '_', "-")
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-");

    // Min length: match backend's 3-char minimum
    if slug.len() < 3 {
        if slug.is_empty() {
            slug = "datasource".to_string();
        } else {
            slug.push_str("-db");
        }
    }

    // Max length: match backend's 100-char limit
    slug.truncate(100);
    slug
}

// ─────────────────────────────────────────────────────────────────────────────
// State types
// ─────────────────────────────────────────────────────────────────────────────

/// Multi-step flow state.
#[derive(Clone, Copy, PartialEq)]
enum SetupStep {
    Select,
    Create,
    Success,
}

/// Token delivery status for the CLI callback.
#[derive(Clone, Copy, PartialEq)]
enum DeliveryStatus {
    Pending,
    Delivered,
    Failed,
}

// ─────────────────────────────────────────────────────────────────────────────
// Main Page
// ─────────────────────────────────────────────────────────────────────────────

/// Connect Setup page — full-screen centered card with multi-step flow.
#[component]
pub fn ConnectSetupPage() -> impl IntoView {
    // ── Query parameters (from CLI redirect) ────────────────────────────
    let params = use_query_map();
    let callback_port =
        Signal::derive(move || params.read().get("callback_port").unwrap_or_default());
    let callback_state = Signal::derive(move || params.read().get("state").unwrap_or_default());
    let has_callback =
        Signal::derive(move || !callback_port.get().is_empty() && !callback_state.get().is_empty());

    // ── User context (for admin check) ──────────────────────────────────
    let user_ctx_resource = Resource::new(|| (), |_| get_user_context());

    // ── Page state ──────────────────────────────────────────────────────
    let (step, set_step) = signal(SetupStep::Select);
    let (datasources, set_datasources) = signal(Vec::<ConnectDatasource>::new());
    let (loading, set_loading) = signal(true);
    let (error, set_error) = signal(Option::<String>::None);
    let (token, set_token) = signal(Option::<String>::None);
    let (delivery_status, set_delivery_status) = signal(Option::<DeliveryStatus>::None);

    // Create form state
    let (new_name, set_new_name) = signal(String::new());
    let (new_type, set_new_type) = signal("postgres".to_string());
    let (show_slug, set_show_slug) = signal(false);
    let (new_slug, set_new_slug) = signal(String::new());

    // Token generation in progress for a specific datasource ID — used to show
    // the per-row spinner while the rotate action is pending.
    let (generating_token_for, set_generating_token_for) = signal(Option::<String>::None);

    // ── Fetch Connect datasources on mount ──────────────────────────────
    let fetch_datasources = Action::new(move |_: &()| async move {
        set_loading.set(true);
        match list_connect_datasources().await {
            Ok(ds) => {
                set_datasources.set(ds);
                set_error.set(None);
            }
            Err(e) => {
                set_error.set(Some(format!("Failed to load datasources: {e}")));
            }
        }
        set_loading.set(false);
    });

    // Trigger initial fetch
    Effect::new(move || {
        fetch_datasources.dispatch(());
    });

    // ── Deliver token to CLI callback ───────────────────────────────────
    // Uses JsFuture + web_sys::window().fetch() + clipboard — all !Send on
    // wasm32. Must remain spawn_local; try_set guards all deferred writes.
    let deliver_token = move |token_value: String| {
        if !has_callback.get_untracked() {
            return;
        }

        set_delivery_status.set(Some(DeliveryStatus::Pending));

        let port = callback_port.get_untracked();
        let state = callback_state.get_untracked();

        leptos::task::spawn_local(async move {
            let callback_url = format!(
                "http://127.0.0.1:{port}/callback?token={token}&state={state}",
                token = js_sys::encode_uri_component(&token_value),
                state = js_sys::encode_uri_component(&state),
            );

            let result = async {
                let window = web_sys::window().ok_or("No window")?;
                let promise = window.fetch_with_str(&callback_url);
                let resp_val = wasm_bindgen_futures::JsFuture::from(promise)
                    .await
                    .map_err(|_| "Fetch failed")?;
                let resp: web_sys::Response = resp_val.dyn_into().map_err(|_| "Not a Response")?;
                if resp.ok() {
                    Ok(())
                } else {
                    Err("Non-OK response")
                }
            }
            .await;

            match result {
                Ok(()) => { set_delivery_status.try_set(Some(DeliveryStatus::Delivered)); }
                Err(_) => { set_delivery_status.try_set(Some(DeliveryStatus::Failed)); }
            }
        });
    };

    // ── Rotate token Action (select existing datasource) ────────────────
    // Returns (ds_id, new_token) so the Effect can clear the per-row spinner
    // using the same dispatch-time ds_id rather than reading a live signal.
    let rotate_action =
        Action::new(move |ds_id: &String| {
            let ds_id = ds_id.clone();
            async move {
                rotate_connect_token(ds_id.clone())
                    .await
                    .map(|tok| (ds_id, tok))
            }
        });

    Effect::new(move |_| {
        if let Some(result) = rotate_action.value().get() {
            match result {
                Ok((ds_id, new_token)) => {
                    let _ = ds_id; // threaded back for identity; cleared by generating_token_for below
                    set_generating_token_for.set(None);
                    set_token.set(Some(new_token.clone()));
                    set_step.set(SetupStep::Success);
                    // deliver_token sets delivery_status synchronously, then
                    // uses spawn_local internally for the !Send browser fetch.
                    deliver_token(new_token);
                }
                Err(e) => {
                    set_generating_token_for.set(None);
                    set_error.set(Some(format!("Failed to generate token: {e}")));
                }
            }
        }
    });

    let on_select_datasource = move |ds_id: String| {
        if rotate_action.pending().get_untracked() {
            return;
        }
        set_generating_token_for.set(Some(ds_id.clone()));
        set_error.set(None);
        rotate_action.dispatch(ds_id);
    };

    // ── Create datasource Action ────────────────────────────────────────
    // Input: (name, slug_opt, ds_type). Returns (submitted_name, server_result)
    // so the Effect can use the dispatch-time name for slug conflict handling
    // without reading the live signal (which the user may have edited).
    let create_action = Action::new(
        move |(name, slug, ds_type): &(String, Option<String>, String)| {
            let name = name.clone();
            let slug = slug.clone();
            let ds_type = ds_type.clone();
            async move {
                let result = create_connect_datasource(name.clone(), slug, ds_type).await;
                (name, result)
            }
        },
    );

    Effect::new(move |_| {
        if let Some((submitted_name, result)) = create_action.value().get() {
            match result {
                Ok(resp) => {
                    let tok = resp.connect_token;
                    set_token.set(Some(tok.clone()));
                    set_step.set(SetupStep::Success);
                    // deliver_token sets delivery_status synchronously, then
                    // uses spawn_local internally for the !Send browser fetch.
                    deliver_token(tok);
                }
                Err(e) => {
                    let msg = e.to_string();
                    // 409 conflict — show slug field so user can customize.
                    if msg.contains("already exists")
                        || msg.contains("duplicate")
                        || msg.contains("conflict")
                    {
                        set_show_slug.set(true);
                        if new_slug.try_get_untracked().map(|s| s.is_empty()).unwrap_or(false) {
                            set_new_slug.set(generate_slug(&submitted_name));
                        }
                    }
                    set_error.set(Some(msg));
                }
            }
        }
    });

    let on_create_submit = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();

        if create_action.pending().get_untracked() {
            return;
        }

        let name = new_name.get_untracked().trim().to_string();
        if name.is_empty() {
            return;
        }

        let slug = if show_slug.get_untracked() && !new_slug.get_untracked().is_empty() {
            Some(new_slug.get_untracked())
        } else {
            None
        };
        let ds_type = new_type.get_untracked();

        set_error.set(None);
        create_action.dispatch((name, slug, ds_type));
    };

    view! {
        <Transition fallback=move || view! {
            <div class="min-h-screen flex items-center justify-center bg-background p-4">
                <Card class="max-w-lg w-full p-8">
                    <div class="flex items-center justify-center py-12">
                        // Branded moment — DESIGN.md Loading State Pattern
                        <img
                            src="/kyomi_animated_logo.svg"
                            alt="Processing"
                            class="w-12 h-12"
                        />
                    </div>
                </Card>
            </div>
        }>
            {move || Suspend::new(async move {
                // Every server_fns::connect fn gates on
                // `ac.require(Permission::ManageConnect, ...)` — mirror that here.
                let can_manage_connect = match user_ctx_resource.await {
                    Ok(ctx) => ctx.can(Permission::ManageConnect),
                    Err(_) => false,
                };

                if !can_manage_connect {
                    return view! {
                        <AdminRequired/>
                    }.into_any();
                }

                view! {
                    <div class="min-h-screen flex items-center justify-center bg-background p-4">
                        <Card class="max-w-lg w-full p-8">
                            // Logo + Header
                            <div class="text-center mb-8">
                                <div class="mb-6">
                                    <img src="/kyomi_full_logo.svg" alt="Kyomi"
                                        class="h-10 mx-auto dark:hidden"/>
                                    <img src="/kyomi_full_logo_white.svg" alt="Kyomi"
                                        class="h-10 mx-auto hidden dark:block"/>
                                </div>
                                <h2 class="text-xl font-semibold mb-2">"Connect Setup"</h2>
                                <p class="text-muted-foreground">
                                    {move || match step.get() {
                                        SetupStep::Select => "Select or create a datasource for your agent",
                                        SetupStep::Create => "Create a new datasource",
                                        SetupStep::Success => "Your Connect token is ready",
                                    }}
                                </p>
                            </div>

                            // Error alert
                            {move || error.get().map(|msg| view! {
                                <div class="mb-6">
                                    <Alert variant=AlertVariant::Error>
                                        <AlertDescription>{msg}</AlertDescription>
                                    </Alert>
                                </div>
                            })}

                            // Loading indicator
                            <Show when=move || loading.get()>
                                <div class="flex items-center justify-center py-12">
                                    // Branded moment — DESIGN.md Loading State Pattern
                                    <img
                                        src="/kyomi_animated_logo.svg"
                                        alt="Processing"
                                        class="w-12 h-12"
                                    />
                                </div>
                            </Show>

                            // Step: Select datasource
                            <Show when=move || !loading.get() && step.get() == SetupStep::Select>
                                <SelectStep
                                    datasources=datasources
                                    generating_token_for=generating_token_for
                                    on_select=on_select_datasource
                                    on_create_new=move || {
                                        set_error.set(None);
                                        set_step.set(SetupStep::Create);
                                    }
                                />
                            </Show>

                            // Step: Create datasource
                            <Show when=move || !loading.get() && step.get() == SetupStep::Create>
                                <CreateStep
                                    new_name=new_name
                                    set_new_name=set_new_name
                                    new_slug=new_slug
                                    set_new_slug=set_new_slug
                                    show_slug=show_slug
                                    new_type=new_type
                                    set_new_type=set_new_type
                                    creating=Signal::from(create_action.pending())
                                    on_submit=on_create_submit
                                    on_back=move || {
                                        set_error.set(None);
                                        set_show_slug.set(false);
                                        set_new_slug.set(String::new());
                                        set_step.set(SetupStep::Select);
                                    }
                                />
                            </Show>

                            // Step: Success — token display
                            <Show when=move || !loading.get() && step.get() == SetupStep::Success && token.get().is_some()>
                                <SuccessStep
                                    token=Signal::derive(move || token.get().unwrap_or_default())
                                    has_callback=has_callback
                                    delivery_status=delivery_status
                                />
                            </Show>
                        </Card>
                    </div>
                }.into_any()
            })}
        </Transition>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Admin Required
// ─────────────────────────────────────────────────────────────────────────────

/// Non-admin users see this centered card.
#[component]
fn AdminRequired() -> impl IntoView {
    view! {
        <div class="min-h-screen flex items-center justify-center bg-background p-4">
            <Card class="max-w-lg w-full p-8">
                <div class="text-center mb-8">
                    <div class="mb-6">
                        <img src="/kyomi_full_logo.svg" alt="Kyomi"
                            class="h-10 mx-auto dark:hidden"/>
                        <img src="/kyomi_full_logo_white.svg" alt="Kyomi"
                            class="h-10 mx-auto hidden dark:block"/>
                    </div>
                    <h2 class="text-xl font-semibold mb-2">"Admin Access Required"</h2>
                    <p class="text-muted-foreground">
                        "Only workspace admins can set up Kyomi Connect datasources. "
                        "Contact your workspace admin to get a Connect token."
                    </p>
                </div>
            </Card>
        </div>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Select Step
// ─────────────────────────────────────────────────────────────────────────────

/// Datasource selection list — Step 1.
#[component]
fn SelectStep(
    datasources: ReadSignal<Vec<ConnectDatasource>>,
    generating_token_for: ReadSignal<Option<String>>,
    on_select: impl Fn(String) + Clone + Send + Sync + 'static,
    on_create_new: impl Fn() + Clone + Send + Sync + 'static,
) -> impl IntoView {
    let on_select = StoredValue::new(on_select);
    let on_create_new = StoredValue::new(on_create_new);

    view! {
        <div class="space-y-3">
            // Existing datasource list
            <Show when=move || !datasources.get().is_empty()>
                <div class="space-y-3">
                    <For
                        each=move || datasources.get()
                        key=|ds| ds.id.clone()
                        let:ds
                    >
                        {
                            let ds_id = ds.id.clone();
                            let ds_id_click = ds.id.clone();
                            let ds_id_check = ds.id.clone();
                            let ds_name = ds.name.clone();
                            let ds_type = ds.datasource_type.clone();
                            let label = get_datasource_label(&ds.datasource_type);
                            let on_select = on_select;
                            view! {
                                <button
                                    on:click=move |_| {
                                        on_select.with_value(|f| f(ds_id_click.clone()));
                                    }
                                    disabled=move || generating_token_for.get().as_deref() == Some(&ds_id_check)
                                    class="w-full flex items-center gap-3 p-4 border border-border rounded-lg bg-card hover:border-primary/40 hover:bg-primary/5 transition-colors text-left group disabled:opacity-60"
                                >
                                    <DatasourceIconInline ds_type=ds_type.clone()/>
                                    <div class="flex-1 min-w-0">
                                        <span class="font-medium">{ds_name}</span>
                                        <div class="text-sm text-muted-foreground">{label}</div>
                                    </div>
                                    {move || {
                                        if generating_token_for.get().as_deref() == Some(&ds_id) {
                                            view! {
                                                <span class="shrink-0">
                                                    <Spinner class="text-muted-foreground"/>
                                                </span>
                                            }.into_any()
                                        } else {
                                            view! {
                                                <span class="text-xs text-muted-foreground opacity-0 group-hover:opacity-100 transition-opacity shrink-0">
                                                    "Generate token"
                                                </span>
                                            }.into_any()
                                        }
                                    }}
                                </button>
                            }
                        }
                    </For>
                </div>
            </Show>

            // "or" divider
            <Show when=move || !datasources.get().is_empty()>
                <div class="relative my-4">
                    <div class="absolute inset-0 flex items-center">
                        <span class="w-full border-t border-border"/>
                    </div>
                    <div class="relative flex justify-center text-xs">
                        <span class="bg-card px-2 text-muted-foreground">"or"</span>
                    </div>
                </div>
            </Show>

            // Create new button
            {move || {
                let has_ds = !datasources.get().is_empty();
                let variant = if has_ds { ButtonVariant::Outline } else { ButtonVariant::Default };
                let on_create_new = on_create_new;
                view! {
                    <Button
                        variant=variant
                        size=ButtonSize::Lg
                        class="w-full"
                        on:click=move |_| {
                            on_create_new.with_value(|f| f());
                        }
                    >
                        <span class="h-4 w-4 inline-flex items-center justify-center">
                            <Icon icon=phosphor_leptos::PLUS/>
                        </span>
                        "Create new datasource"
                    </Button>
                }
            }}

            // Warning text
            <Show when=move || !datasources.get().is_empty()>
                <p class="text-xs text-center text-muted-foreground mt-4">
                    "Generating a new token will disconnect any currently connected agent."
                </p>
            </Show>
        </div>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Create Step
// ─────────────────────────────────────────────────────────────────────────────

/// New datasource creation form — Step 2.
#[component]
fn CreateStep(
    new_name: ReadSignal<String>,
    set_new_name: WriteSignal<String>,
    new_slug: ReadSignal<String>,
    set_new_slug: WriteSignal<String>,
    show_slug: ReadSignal<bool>,
    new_type: ReadSignal<String>,
    set_new_type: WriteSignal<String>,
    creating: Signal<bool>,
    on_submit: impl Fn(leptos::ev::SubmitEvent) + Clone + Send + Sync + 'static,
    on_back: impl Fn() + Clone + Send + Sync + 'static,
) -> impl IntoView {
    let on_back = StoredValue::new(on_back);

    view! {
        <form on:submit=on_submit class="space-y-5">
            // Back button
            <button
                type="button"
                on:click=move |_| on_back.with_value(|f| f())
                class="flex items-center gap-1 text-sm text-muted-foreground hover:text-foreground transition-colors -mt-1 mb-2"
            >
                <span class="h-3.5 w-3.5 inline-flex items-center justify-center">
                    <Icon icon=phosphor_leptos::ARROW_LEFT/>
                </span>
                "Back"
            </button>

            // Name input
            <div>
                <label class="block text-sm font-semibold mb-2">"Datasource name"</label>
                <input
                    type="text"
                    class=INPUT_CLASS
                    prop:value=move || new_name.get()
                    on:input=move |ev| set_new_name.set(event_target_value(&ev))
                    placeholder="e.g. Production PostgreSQL"
                    autofocus=true
                    required=true
                />
            </div>

            // Slug input (shown only after 409 conflict)
            <Show when=move || show_slug.get()>
                <div>
                    <label class="block text-sm font-semibold mb-2">"Slug"</label>
                    <input
                        type="text"
                        class=format!("{INPUT_CLASS} font-mono")
                        prop:value=move || new_slug.get()
                        on:input=move |ev| {
                            let val: String = event_target_value(&ev)
                                .to_lowercase()
                                .chars()
                                .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
                                .collect();
                            set_new_slug.set(val);
                        }
                        placeholder="my-database"
                        required=true
                    />
                    <p class="text-xs text-muted-foreground mt-1">
                        "Must be unique within your workspace."
                    </p>
                </div>
            </Show>

            // Database type grid
            <div>
                <label class="block text-sm font-semibold mb-2">"Database type"</label>
                <div class="grid grid-cols-2 gap-2">
                    {CONNECT_TYPES.iter().map(|(value, label)| {
                        let value = *value;
                        let label = *label;
                        view! {
                            <button
                                type="button"
                                on:click=move |_| set_new_type.set(value.to_string())
                                class=move || {
                                    let selected = new_type.get() == value;
                                    if selected {
                                        "flex items-center gap-2 p-3 rounded-lg border text-left text-sm transition-colors border-primary bg-primary/5 text-foreground"
                                    } else {
                                        "flex items-center gap-2 p-3 rounded-lg border text-left text-sm transition-colors border-border text-muted-foreground hover:border-muted-foreground/40 hover:text-foreground"
                                    }
                                }
                            >
                                <DatasourceIconInline ds_type=value.to_string() small=true/>
                                {label}
                            </button>
                        }
                    }).collect_view()}
                </div>
            </div>

            // Submit button
            //
            // `button_type="submit"` is required: `Button` defaults to
            // `type="button"` so buttons don't accidentally submit, but
            // this one is the submit control for the `on:submit` form
            // above. Without it the handler never fires and the button
            // is silently inert (KYO-281).
            <Button
                size=ButtonSize::Lg
                class="w-full"
                disabled=Signal::derive(move || creating.get() || new_name.get().trim().is_empty())
                button_type="submit"
            >
                {move || {
                    if creating.get() {
                        view! {
                            <Spinner class="text-primary-foreground"/>
                            "Creating..."
                        }.into_any()
                    } else {
                        view! {
                            "Create & Generate Token"
                        }.into_any()
                    }
                }}
            </Button>
        </form>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Success Step
// ─────────────────────────────────────────────────────────────────────────────

/// Token display and delivery status — Step 3.
#[component]
fn SuccessStep(
    token: Signal<String>,
    has_callback: Signal<bool>,
    delivery_status: ReadSignal<Option<DeliveryStatus>>,
) -> impl IntoView {
    let (copied, set_copied) = signal(false);
    let (install_copied, set_install_copied) = signal(false);

    // Copy token to clipboard
    let on_copy_token = move |_: leptos::ev::MouseEvent| {
        let text = token.get_untracked();
        leptos::task::spawn_local(async move {
            if let Some(window) = web_sys::window() {
                let clipboard = window.navigator().clipboard();
                let promise = clipboard.write_text(&text);
                let _ = wasm_bindgen_futures::JsFuture::from(promise).await;
                set_copied.try_set(true);
                gloo_timers::future::TimeoutFuture::new(2000).await;
                set_copied.try_set(false);
            }
        });
    };

    // Copy install command to clipboard
    let on_copy_install = move |_: leptos::ev::MouseEvent| {
        let Some(tok) = token.try_get_untracked() else {
            return;
        };
        let cmd =
            format!("curl -fsSL https://connect.kyomi.ai/install.sh | sh -s -- --token \"{tok}\"");
        leptos::task::spawn_local(async move {
            if let Some(window) = web_sys::window() {
                let clipboard = window.navigator().clipboard();
                let promise = clipboard.write_text(&cmd);
                let _ = wasm_bindgen_futures::JsFuture::from(promise).await;
                set_install_copied.try_set(true);
                gloo_timers::future::TimeoutFuture::new(2000).await;
                set_install_copied.try_set(false);
            }
        });
    };

    view! {
        <div class="space-y-5">
            // Delivery status — delivered
            <Show when=move || has_callback.get() && delivery_status.get() == Some(DeliveryStatus::Delivered)>
                <div class="flex items-center justify-center gap-2 text-sm text-foreground py-2">
                    <span class="h-5 w-5 text-success-foreground inline-flex items-center justify-center">
                        <Icon icon=phosphor_leptos::CHECK_CIRCLE/>
                    </span>
                    "Token delivered to CLI. You can close this tab."
                </div>
            </Show>

            // Delivery status — pending
            <Show when=move || has_callback.get() && delivery_status.get() == Some(DeliveryStatus::Pending)>
                <div class="flex items-center justify-center gap-2 text-sm text-muted-foreground py-2">
                    <Spinner/>
                    "Sending token to CLI..."
                </div>
            </Show>

            // Token display
            <div>
                <label class="block text-sm font-semibold mb-2">"Connect token"</label>
                <div class="flex items-center gap-2 p-3 rounded-lg border border-border bg-muted/30">
                    <code class="flex-1 text-xs text-foreground font-mono break-all select-all line-clamp-3">
                        {move || token.get()}
                    </code>
                    <CopyButton copied=copied on_click=on_copy_token/>
                </div>
            </div>

            // Manual instructions (shown when no callback or delivery failed)
            <Show when=move || !has_callback.get() || delivery_status.get() == Some(DeliveryStatus::Failed)>
                <div class="border border-border rounded-lg p-5 space-y-2">
                    <p class="font-semibold">"Next steps"</p>
                    <p class="text-sm text-muted-foreground">
                        "Install and configure Kyomi Connect in one command:"
                    </p>
                    <div class="flex items-center gap-2 px-3 py-2 rounded-lg bg-muted/30 border border-border">
                        <code class="text-xs text-foreground font-mono flex-1 break-all">
                            "curl -fsSL https://connect.kyomi.ai/install.sh | sh -s -- --token \""
                            {move || token.get()}
                            "\""
                        </code>
                        <CopyButton copied=install_copied on_click=on_copy_install/>
                    </div>
                    <p class="text-xs text-muted-foreground mt-1">
                        "Already installed? Run: "
                        <code class="font-mono">"kyomi-connect setup --token <TOKEN>"</code>
                    </p>
                </div>
            </Show>

            // Failed delivery message
            <Show when=move || has_callback.get() && delivery_status.get() == Some(DeliveryStatus::Failed)>
                <p class="text-xs text-center text-muted-foreground">
                    "Could not reach the CLI automatically. Copy the token and paste it in your terminal."
                </p>
            </Show>
        </div>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Shared helper components
// ─────────────────────────────────────────────────────────────────────────────

/// Copy-to-clipboard button — shows a checkmark icon after copying.
///
/// Uses inline SVG icons (Lucide Copy and Check) to avoid icon library
/// dependencies for just two icons.
#[component]
fn CopyButton(
    copied: ReadSignal<bool>,
    on_click: impl Fn(leptos::ev::MouseEvent) + 'static,
) -> impl IntoView {
    view! {
        <button
            type="button"
            on:click=on_click
            class="shrink-0 p-1 rounded-md hover:bg-muted/50 transition-colors text-muted-foreground hover:text-foreground"
            title="Copy to clipboard"
        >
            {move || {
                if copied.get() {
                    view! {
                        <span class="h-4 w-4 text-success-foreground inline-flex items-center justify-center">
                            <Icon icon=phosphor_leptos::CHECK/>
                        </span>
                    }.into_any()
                } else {
                    view! {
                        <span class="h-4 w-4 inline-flex items-center justify-center">
                            <Icon icon=phosphor_leptos::COPY/>
                        </span>
                    }.into_any()
                }
            }}
        </button>
    }
}

/// Inline datasource icon — renders a database icon with the type label.
///
/// The React app uses `<DatasourceIcon>` which renders provider-specific SVGs.
/// In Leptos, we use the generic database icon from Lucide for all types,
/// since the provider-specific icon set is not yet ported.
#[component]
fn DatasourceIconInline(ds_type: String, #[prop(default = false)] small: bool) -> impl IntoView {
    let _ = ds_type; // Reserved for future provider-specific icons
    let class = if small {
        "h-5 w-5 text-muted-foreground inline-flex items-center justify-center"
    } else {
        "h-8 w-8 text-muted-foreground inline-flex items-center justify-center"
    };

    view! {
        <span class=class>
            <Icon icon=phosphor_leptos::DATABASE/>
        </span>
    }
}
