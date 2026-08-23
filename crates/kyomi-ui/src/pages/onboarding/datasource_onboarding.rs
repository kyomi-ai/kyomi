// SPDX-License-Identifier: AGPL-3.0-or-later

//! Datasource Onboarding page — routes users to the appropriate onboarding
//! experience based on their role and workspace state.
//!
//! Decision tree (5 states):
//! 1. **Loading** — spinner while checking onboarding state
//! 2. **Admin with no datasources** — binary choice card (sample vs connect own)
//! 3. **Invited user with datasources needing credentials** — credential setup list
//! 4. **Non-admin with no datasources** — waiting for admin setup (polls every 10s)
//! 5. **User with all datasources ready** — immediate redirect to `/chat`
//!
//! Mirrors `apps/frontend/src/pages/DatasourceOnboarding.jsx`.

use leptos::prelude::*;
use phosphor_leptos::Icon;
use leptos_router::hooks::use_navigate;
use leptos_router::NavigateOptions;

use crate::components::toast::{toast_error, toast_success};
use crate::components::{Badge, BadgeVariant, Button, ButtonVariant, Card, Spinner};
use crate::server_fns::onboarding::{
    create_sample_datasource, get_oauth_connect_url, get_onboarding_state, CredentialStatusItem,
    OnboardingState,
};

// ─────────────────────────────────────────────────────────────────────────────
// Main Page
// ─────────────────────────────────────────────────────────────────────────────

/// Datasource onboarding page — the entry point after signup or invitation.
#[component]
pub fn DatasourceOnboardingPage() -> impl IntoView {
    let state_resource = Resource::new(|| (), |_| get_onboarding_state());

    view! {
        <Transition fallback=move || view! { <LoadingState/> }>
            {move || Suspend::new(async move {
                match state_resource.await {
                    Ok(state) => {
                        view! {
                            <OnboardingRouter
                                initial_state=state
                                state_resource=state_resource
                            />
                        }.into_any()
                    }
                    Err(e) => view! {
                        <div class="min-h-screen bg-background flex items-center justify-center p-4">
                            <Card>
                                <div class="p-8 text-center">
                                    <p class="text-error-foreground">
                                        {format!("Failed to load onboarding state: {e}")}
                                    </p>
                                </div>
                            </Card>
                        </div>
                    }.into_any()
                }
            })}
        </Transition>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// State Router — decides which sub-view to show
// ─────────────────────────────────────────────────────────────────────────────

/// Routes to the correct onboarding sub-view based on the server state.
#[component]
fn OnboardingRouter(
    initial_state: OnboardingState,
    state_resource: Resource<Result<OnboardingState, ServerFnError>>,
) -> impl IntoView {
    let navigate = use_navigate();

    // If all datasources are ready, redirect immediately
    if initial_state.has_datasources && !initial_state.needs_credentials {
        let nav = navigate.clone();
        // Use Effect to navigate outside render phase
        Effect::new(move |_| {
            nav(
                "/chat",
                NavigateOptions {
                    replace: true,
                    ..Default::default()
                },
            );
        });
        return view! { <LoadingState/> }.into_any();
    }

    // State 2: Admin with no datasources — show choice card
    if !initial_state.has_datasources && initial_state.is_admin {
        return view! {
            <AdminChoiceCard sample_available=initial_state.sample_available/>
        }
        .into_any();
    }

    // State 3: User with datasources needing credentials
    if initial_state.needs_credentials {
        let items_needing_action: Vec<_> = initial_state
            .credential_status
            .iter()
            .filter(|c| c.needs_action)
            .cloned()
            .collect();

        return view! {
            <CredentialSetup
                items=items_needing_action
                total=initial_state.total_datasources
                state_resource=state_resource
            />
        }
        .into_any();
    }

    // State 4: Non-admin with no datasources — waiting for admin
    view! {
        <WaitingForSetup state_resource=state_resource/>
    }
    .into_any()
}

// ─────────────────────────────────────────────────────────────────────────────
// State 1: Loading
// ─────────────────────────────────────────────────────────────────────────────

#[component]
fn LoadingState() -> impl IntoView {
    view! {
        <div class="min-h-screen bg-background flex items-center justify-center">
            <img src="/kyomi_animated_logo.svg" alt="Processing" class="w-12 h-12" />
        </div>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// State 2: Admin Choice Card
// ─────────────────────────────────────────────────────────────────────────────

/// Binary choice card for admins with no datasources.
///
/// Option 1 (if sample available): "Explore with Sample Data"
/// Option 2: "Connect Your Own Database" -> navigate to /settings/datasources
#[component]
fn AdminChoiceCard(sample_available: bool) -> impl IntoView {
    let navigate = use_navigate();
    let (creating_sample, set_creating_sample) = signal(false);

    let nav_settings = navigate.clone();
    let nav_chat = navigate.clone();

    let handle_explore_sample = Action::new(move |_: &()| {
        set_creating_sample.set(true);
        let nav = nav_chat.clone();
        async move {
            match create_sample_datasource().await {
                Ok(()) => {
                    toast_success("Sample datasource added — start exploring!");
                    nav(
                        "/chat",
                        NavigateOptions {
                            replace: true,
                            ..Default::default()
                        },
                    );
                }
                Err(e) => {
                    toast_error(format!("Failed to add sample datasource: {e}"));
                    set_creating_sample.set(false);
                }
            }
        }
    });

    let handle_connect_own = move |_| {
        nav_settings(
            "/settings/datasources",
            NavigateOptions::default(),
        );
    };

    view! {
        <div class="min-h-screen flex items-center justify-center bg-background p-4">
            <Card>
                <div class="max-w-xl w-full p-8">
                    <div class="text-center mb-8">
                        <h1 class="text-xl font-semibold mb-2">"Welcome to Kyomi!"</h1>
                        <p class="text-muted-foreground">
                            "Choose how you'd like to get started"
                        </p>
                    </div>

                    <div class="space-y-4">
                        // Option 1: Explore with sample data
                        {sample_available.then(|| view! {
                            <div class="border border-border rounded-lg p-5">
                                <div class="flex items-start gap-4">
                                    <Icon
                                        icon=phosphor_leptos::DATABASE
                                        attr:class="h-6 w-6 mt-0.5 text-muted-foreground flex-shrink-0"
                                    />
                                    <div class="flex-1">
                                        <h3 class="font-semibold mb-1">"Explore with Sample Data"</h3>
                                        <p class="text-sm text-muted-foreground mb-3">
                                            "Dive in with our Acme Analytics demo dataset — no setup required"
                                        </p>
                                        <Button
                                            class="w-full"
                                            on:click=move |_| { handle_explore_sample.dispatch(()); }
                                        >
                                            {move || if creating_sample.get() {
                                                "Setting up...".to_string()
                                            } else {
                                                "Start Exploring".to_string()
                                            }}
                                        </Button>
                                    </div>
                                </div>
                            </div>
                        })}

                        // Option 2: Connect own database
                        <div class="border border-border rounded-lg p-5">
                            <div class="flex items-start gap-4">
                                <Icon
                                    icon=phosphor_leptos::DATABASE
                                    attr:class="h-6 w-6 mt-0.5 text-muted-foreground flex-shrink-0"
                                />
                                <div class="flex-1">
                                    <h3 class="font-semibold mb-1">"Connect Your Own Database"</h3>
                                    <p class="text-sm text-muted-foreground mb-3">
                                        "Connect your data warehouse to ask questions about your real data"
                                    </p>
                                    <Button
                                        variant=if sample_available { ButtonVariant::Outline } else { ButtonVariant::Default }
                                        class="w-full"
                                        on:click=handle_connect_own
                                    >
                                        "Connect Datasource"
                                    </Button>
                                </div>
                            </div>
                        </div>
                    </div>

                    <p class="text-xs text-center text-muted-foreground mt-6">
                        "You can always change this later in Settings"
                    </p>
                </div>
            </Card>
        </div>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// State 3: Credential Setup
// ─────────────────────────────────────────────────────────────────────────────

/// Credential setup UI for invited users with existing datasources.
///
/// Shows a list of datasources needing credentials, with per-datasource
/// action buttons (OAuth connect or password setup navigation).
#[component]
fn CredentialSetup(
    items: Vec<CredentialStatusItem>,
    total: usize,
    state_resource: Resource<Result<OnboardingState, ServerFnError>>,
) -> impl IntoView {
    let navigate = use_navigate();
    let (oauth_connecting, set_oauth_connecting) = signal::<Option<String>>(None);

    // Store items in a signal so we can update the list after OAuth success
    let (credential_items, set_credential_items) = signal(items);

    // ── Watch for state_resource updates ────────────────────────────────
    // When the resource is refetched (after OAuth success or error), update
    // the credential items list or redirect to /chat if all done.
    {
        let nav = navigate.clone();
        Effect::new(move |_| {
            if let Some(Ok(state)) = state_resource.get() {
                if !state.needs_credentials {
                    nav(
                        "/chat",
                        NavigateOptions {
                            replace: true,
                            ..Default::default()
                        },
                    );
                } else {
                    let new_items: Vec<_> = state
                        .credential_status
                        .iter()
                        .filter(|c| c.needs_action)
                        .cloned()
                        .collect();
                    set_credential_items.set(new_items);
                }
            }
        });
    }

    // ── OAuth message listener ───────────────────────────────────────────
    // Listen for postMessage from OAuth popup windows. On success, refetch
    // the state_resource (the Effect above handles the rest).
    #[cfg(target_arch = "wasm32")]
    {
        use crate::utils::oauth_popup::{
            install_oauth_listener, translate_google_oauth_error, OAuthMessage,
        };

        let cleanup = install_oauth_listener(move |msg| {
            match msg {
                OAuthMessage::GoogleSuccess { .. } | OAuthMessage::BigqueryEnterpriseSuccess { .. } => {
                    set_oauth_connecting.try_set(None);
                    toast_success("BigQuery connected successfully");
                    state_resource.refetch();
                }
                OAuthMessage::SnowflakeSuccess { .. } => {
                    set_oauth_connecting.try_set(None);
                    toast_success("Snowflake connected successfully");
                    state_resource.refetch();
                }
                OAuthMessage::MicrosoftEnterpriseSuccess { .. } => {
                    set_oauth_connecting.try_set(None);
                    toast_success("Azure Synapse connected successfully");
                    state_resource.refetch();
                }
                OAuthMessage::DatabricksSuccess { .. } => {
                    set_oauth_connecting.try_set(None);
                    toast_success("Databricks connected successfully");
                    state_resource.refetch();
                }
                OAuthMessage::MicrosoftSuccess { .. } => {
                    set_oauth_connecting.try_set(None);
                    toast_success("Microsoft connected successfully");
                    state_resource.refetch();
                }
                // GoogleError is split into its own arm (KYO-421) so the
                // allowlist-rejection translation below applies ONLY to the
                // shared kyomi_oauth Google flow — see
                // translate_google_oauth_error's doc comment in
                // utils::oauth_popup for why applying it to the other five
                // providers' errors would misdescribe those.
                OAuthMessage::GoogleError { error } => {
                    set_oauth_connecting.try_set(None);
                    toast_error(translate_google_oauth_error(error));
                }
                OAuthMessage::SnowflakeError { error }
                | OAuthMessage::DatabricksError { error }
                | OAuthMessage::MicrosoftError { error }
                | OAuthMessage::MicrosoftEnterpriseError { error }
                | OAuthMessage::BigqueryEnterpriseError { error } => {
                    set_oauth_connecting.try_set(None);
                    toast_error(error);
                }
            }
        });
        let cleanup_cell = std::cell::Cell::new(Some(Box::new(cleanup) as Box<dyn FnOnce()>));
        let cleanup_wrapper = send_wrapper::SendWrapper::new(cleanup_cell);
        on_cleanup(move || {
            if let Some(f) = cleanup_wrapper.take().take() {
                f();
            }
        });
    }

    let nav_skip = navigate.clone();
    let nav_password = navigate.clone();

    let handle_skip = move |_| {
        nav_skip("/chat", NavigateOptions::default());
    };

    view! {
        <div class="min-h-screen flex items-center justify-center bg-background p-4">
            <Card>
                <div class="max-w-2xl w-full p-8">
                    <div class="text-center mb-6">
                        <h1 class="text-xl font-semibold mb-2">"Set Up Your Credentials"</h1>
                        <p class="text-muted-foreground">
                            {move || {
                                let t = total;
                                let suffix = if t != 1 { "s" } else { "" };
                                format!(
                                    "Your workspace has {t} datasource{suffix} configured. \
                                     Please provide your credentials to access them."
                                )
                            }}
                        </p>
                    </div>

                    <div class="space-y-3 mb-6">
                        <For
                            each=move || credential_items.get()
                            key=|item| item.datasource_id.clone()
                            let:item
                        >
                            <CredentialRow
                                item=item
                                oauth_connecting=oauth_connecting
                                set_oauth_connecting=set_oauth_connecting
                                navigate=nav_password.clone()
                            />
                        </For>
                    </div>

                    <Button
                        variant=ButtonVariant::Ghost
                        class="w-full"
                        on:click=handle_skip
                    >
                        "Skip for now"
                    </Button>
                </div>
            </Card>
        </div>
    }
}

/// A single row in the credential setup list.
#[component]
fn CredentialRow(
    item: CredentialStatusItem,
    oauth_connecting: ReadSignal<Option<String>>,
    set_oauth_connecting: WriteSignal<Option<String>>,
    navigate: impl Fn(&str, NavigateOptions) + Clone + 'static,
) -> impl IntoView {
    let ds_id = item.datasource_id.clone();
    let ds_type = item.datasource_type.clone();
    let ds_name = item.datasource_name.clone();
    let ds_slug = item.slug.clone();
    let auth_method = item.auth_method.clone();
    let oauth_provider = item.oauth_provider.clone();
    let auth_mode = item.auth_mode.clone();
    let status = item.status.clone();

    let is_expired = status == "expired";

    // Determine OAuth provider label
    let provider_label = oauth_provider
        .as_deref()
        .map(|p| match p {
            "google" => "Google",
            "snowflake" => "Snowflake",
            "microsoft" => "Microsoft",
            "databricks" => "Databricks",
            _ => p,
        })
        .unwrap_or("OAuth");

    let oauth_button_text = if is_expired {
        format!("Reconnect {provider_label}")
    } else {
        format!("Connect with {provider_label}")
    };

    let ds_id_for_connecting = ds_id.clone();
    let is_connecting = Memo::new(move |_| {
        oauth_connecting.get().as_deref() == Some(ds_id_for_connecting.as_str())
    });

    let ds_type_display = ds_type.clone();

    // Build click handler based on auth method
    let action_view = if auth_method == "oauth" {
        let ds_type_oauth = ds_type.clone();
        let ds_slug_oauth = ds_slug.clone();
        let ds_id_oauth = ds_id.clone();
        let auth_mode_oauth = auth_mode.clone().unwrap_or_default();

        let handle_oauth = Action::new(move |_: &()| {
            let ds_type = ds_type_oauth.clone();
            let ds_slug = ds_slug_oauth.clone();
            let ds_id = ds_id_oauth.clone();
            let auth_mode = auth_mode_oauth.clone();
            async move {
                // Get OAuth URL from server using the datasource's actual auth_mode
                match get_oauth_connect_url(
                    ds_type,
                    auth_mode,
                    Some(ds_slug),
                )
                .await
                {
                    Ok(url) => {
                        set_oauth_connecting.set(Some(ds_id.clone()));
                        #[cfg(target_arch = "wasm32")]
                        {
                            use crate::utils::oauth_popup::open_oauth_popup;
                            use wasm_bindgen::prelude::*;
                            use wasm_bindgen::JsCast;

                            match open_oauth_popup(&url, &ds_id) {
                                Some(popup_window) => {
                                    // Monitor popup for manual close. When the popup
                                    // closes (user dismissed it without completing OAuth),
                                    // clear the connecting state so the button re-enables.
                                    let ds_id_monitor = ds_id.clone();
                                    type PopupMonitorState = std::rc::Rc<std::cell::RefCell<Option<(i32, Closure<dyn Fn()>)>>>;
                                    let state: PopupMonitorState =
                                        std::rc::Rc::new(std::cell::RefCell::new(None));
                                    let state_inner = state.clone();

                                    let closure = Closure::<dyn Fn()>::new(move || {
                                        let closed = popup_window.closed().unwrap_or(true);
                                        if closed {
                                            set_oauth_connecting.update(|current| {
                                                if current.as_deref() == Some(ds_id_monitor.as_str()) {
                                                    *current = None;
                                                }
                                            });
                                            if let Some((interval_id, _)) = state_inner.borrow().as_ref()
                                                && let Some(win) = web_sys::window() {
                                                    win.clear_interval_with_handle(*interval_id);
                                                }
                                            state_inner.borrow_mut().take();
                                        }
                                    });

                                    if let Some(window) = web_sys::window() {
                                        let id = window
                                            .set_interval_with_callback_and_timeout_and_arguments_0(
                                                closure.as_ref().unchecked_ref(),
                                                500,
                                            )
                                            .unwrap_or(0);
                                        *state.borrow_mut() = Some((id, closure));
                                    }
                                }
                                None => {
                                    set_oauth_connecting.set(None);
                                    toast_error(
                                        "Popup was blocked. Please allow popups for this site.",
                                    );
                                }
                            }
                        }
                        #[cfg(not(target_arch = "wasm32"))]
                        {
                            let _ = url;
                        }
                    }
                    Err(e) => {
                        toast_error(format!("Failed to get OAuth URL: {e}"));
                    }
                }
            }
        });

        let button_variant = if is_expired {
            ButtonVariant::Outline
        } else {
            ButtonVariant::Default
        };

        let oauth_text = oauth_button_text.clone();
        let connecting_text = if is_expired {
            "Reconnecting...".to_string()
        } else {
            "Connecting...".to_string()
        };

        view! {
            <Button
                variant=button_variant
                on:click=move |_| { handle_oauth.dispatch(()); }
            >
                {move || if is_connecting.get() {
                    view! {
                        <span class="flex items-center gap-2">
                            <Spinner class="h-4 w-4"/>
                            {connecting_text.clone()}
                        </span>
                    }.into_any()
                } else {
                    view! {
                        <span>{oauth_text.clone()}</span>
                    }.into_any()
                }}
            </Button>
        }
        .into_any()
    } else if auth_method == "password" {
        let nav = navigate.clone();
        let slug = ds_slug.clone();
        let handle_password = move |_| {
            // Navigate to datasource settings with pre-selected datasource
            nav(
                &format!("/settings/datasources?open={slug}"),
                NavigateOptions::default(),
            );
        };

        view! {
            <Button on:click=handle_password>
                <Icon icon=phosphor_leptos::KEY attr:class="h-4 w-4 mr-2"/>
                "Enter Credentials"
            </Button>
        }
        .into_any()
    } else {
        // No action needed (shared/connect datasources shouldn't appear here)
        ().into_any()
    };

    view! {
        <div class="flex items-center justify-between p-4 border border-border rounded-lg bg-card">
            <div class="flex items-center gap-3">
                <Icon
                    icon=phosphor_leptos::DATABASE
                    attr:class="w-8 h-8 text-muted-foreground"
                />
                <div>
                    <div class="flex items-center gap-2">
                        <span class="font-medium">{ds_name}</span>
                        {is_expired.then(|| view! {
                            <Badge variant=BadgeVariant::Warning>"Expired"</Badge>
                        })}
                    </div>
                    <div class="text-sm text-muted-foreground capitalize">{ds_type_display}</div>
                </div>
            </div>
            {action_view}
        </div>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// State 4: Waiting for Setup
// ─────────────────────────────────────────────────────────────────────────────

/// Waiting state for non-admin users when no datasources exist.
///
/// Polls every 10 seconds to check if an admin has added datasources.
/// When datasources appear, redirects to the credential setup flow or `/chat`.
#[component]
fn WaitingForSetup(
    state_resource: Resource<Result<OnboardingState, ServerFnError>>,
) -> impl IntoView {
    let navigate = use_navigate();
    #[cfg(not(feature = "hydrate"))]
    let _ = &state_resource;

    // Poll every 10 seconds for datasource availability
    #[cfg(feature = "hydrate")]
    {
        let nav = navigate.clone();
        Effect::new(move |_| {
            use wasm_bindgen::prelude::*;
            use wasm_bindgen::JsCast;

            let Some(window) = web_sys::window() else {
                return;
            };

            let closure = Closure::<dyn Fn()>::new(move || {
                state_resource.refetch();
            });

            let interval_id = window
                .set_interval_with_callback_and_timeout_and_arguments_0(
                    closure.as_ref().unchecked_ref(),
                    10_000, // 10 seconds
                )
                .unwrap_or(0);

            // Clean up interval on unmount
            let wrapper = send_wrapper::SendWrapper::new(closure);
            on_cleanup(move || {
                if let Some(window) = web_sys::window() {
                    window.clear_interval_with_handle(interval_id);
                }
                drop(wrapper);
            });
        });

        // Watch for state changes — redirect when datasources appear
        Effect::new(move |_| {
            if let Some(Ok(state)) = state_resource.get()
                && state.has_datasources {
                    nav(
                        "/onboarding",
                        NavigateOptions {
                            replace: true,
                            ..Default::default()
                        },
                    );
                }
        });
    }

    let nav_chat = navigate.clone();
    let handle_go_to_chat = move |_| {
        nav_chat("/chat", NavigateOptions::default());
    };

    view! {
        <div class="min-h-screen flex items-center justify-center bg-background p-4">
            <Card>
                <div class="max-w-lg w-full p-8">
                    <div class="text-center mb-6">
                        <h1 class="text-xl font-semibold mb-2">"Waiting for Setup"</h1>
                        <p class="text-muted-foreground">
                            "Your workspace administrator needs to configure datasources before you can start."
                        </p>
                    </div>
                    <p class="text-sm text-muted-foreground text-center mb-6">
                        "Please contact your workspace admin to set up the database connections. \
                         Once they have configured the datasources, you will be able to connect \
                         your credentials and start using Kyomi."
                    </p>
                    <Button
                        variant=ButtonVariant::Ghost
                        class="w-full"
                        on:click=handle_go_to_chat
                    >
                        "Go to Chat anyway"
                    </Button>
                </div>
            </Card>
        </div>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests (KYO-421)
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    /// This file's own source, for source-text wiring assertions below —
    /// mirrors the `SRC`/`extract_between` pattern in
    /// `pages/settings/datasources/tests/mod.rs`, kept local here since this
    /// file is far below that module's collision-risk size (see
    /// `docs/standards/testing/one-test-topic-per-file-not-one-big-mod-tests.md`).
    const SRC: &str = include_str!("datasource_onboarding.rs");

    /// `SRC` sliced to production code only, cutting off at this test
    /// module's own opening marker — otherwise a whole-file scan would
    /// match this module's own literals (e.g. the marker strings the tests
    /// below search for, which this doc comment and the assertions
    /// themselves repeat verbatim).
    const TEST_MOD_MARKER: &str = "#[cfg(test)]\nmod tests {";
    fn production_src() -> &'static str {
        SRC.split(TEST_MOD_MARKER)
            .next()
            .expect("TEST_MOD_MARKER must be found in SRC")
    }

    /// Returns the substring of `src` starting just after the first
    /// occurrence of `start` and ending just before the first occurrence of
    /// `end` that follows it. Panics with a descriptive message if either
    /// marker isn't found, so a typo'd marker fails loudly instead of
    /// silently matching an empty/wrong range.
    fn extract_between<'a>(src: &'a str, start: &str, end: &str) -> &'a str {
        let start_idx = src
            .find(start)
            .unwrap_or_else(|| panic!("start marker not found: {start:?}"));
        let after_start = start_idx + start.len();
        let end_idx = src[after_start..]
            .find(end)
            .unwrap_or_else(|| panic!("end marker not found after start: {end:?}"));
        &src[after_start..after_start + end_idx]
    }

    // ── KYO-421: GoogleError must translate; the other five providers must not ──
    //
    // The onboarding OAuth `postMessage` listener used to fold GoogleError
    // into the same combined match arm as Snowflake/Databricks/Microsoft/
    // Microsoft-enterprise/BigQuery-enterprise, so a user rejected by
    // Google's shared-app allowlist during onboarding saw Google's raw
    // OAuth string instead of the KYO-408 translated message the
    // settings-page listeners already show. Mirrors
    // `google_error_translation_is_not_applied_to_other_providers_error_arms`
    // in `pages/settings/datasources/tests/oauth.rs` — the negative half is
    // load-bearing: it is the regression a careless "just wrap toast_error"
    // fix could reintroduce by translating the other five providers' errors
    // too, which would misdescribe them (they have no Kyomi allowlist to be
    // rejected from).

    #[test]
    fn onboarding_google_error_arm_calls_the_translation() {
        let google_arm = extract_between(
            production_src(),
            "OAuthMessage::GoogleError { error } => {",
            "OAuthMessage::SnowflakeError { error }",
        );
        assert!(
            google_arm.contains("translate_google_oauth_error(error)"),
            "the onboarding listener's GoogleError arm must call \
             translate_google_oauth_error, the same as the settings-page \
             list-level and modal-level listeners (KYO-421)"
        );
    }

    #[test]
    fn onboarding_other_providers_error_arm_does_not_call_the_translation() {
        let others_arm = extract_between(
            production_src(),
            "OAuthMessage::SnowflakeError { error }\n                | OAuthMessage::DatabricksError { error }\n                \
             | OAuthMessage::MicrosoftError { error }\n                | OAuthMessage::MicrosoftEnterpriseError { error }\n                \
             | OAuthMessage::BigqueryEnterpriseError { error } => {",
            "toast_error(error);\n                }",
        );
        assert!(
            !others_arm.contains("translate_google_oauth_error"),
            "the onboarding listener's Snowflake/Databricks/Microsoft/\
             Microsoft-enterprise/BigQuery-enterprise error arm must pass \
             `error` straight to toast_error, not through \
             translate_google_oauth_error — that function assumes a Google \
             shared-app allowlist rejection specifically and would \
             misdescribe these providers' errors (KYO-421)"
        );
    }
}
