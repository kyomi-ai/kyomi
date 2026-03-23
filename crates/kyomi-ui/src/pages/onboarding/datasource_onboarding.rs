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
use leptos_icons::Icon;
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
        <Suspense fallback=move || view! { <LoadingState/> }>
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
        </Suspense>
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

/// Centered loading spinner shown while the onboarding state is being fetched.
#[component]
fn LoadingState() -> impl IntoView {
    view! {
        <div class="min-h-screen bg-background flex items-center justify-center">
            <Spinner class="h-8 w-8 text-muted-foreground"/>
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
                        <h1 class="text-3xl font-bold mb-2">"Welcome to Kyomi!"</h1>
                        <p class="text-muted-foreground">
                            "Choose how you'd like to get started"
                        </p>
                    </div>

                    <div class="space-y-4">
                        // Option 1: Explore with sample data
                        {sample_available.then(|| view! {
                            <div class="border border-border rounded-xl p-5">
                                <div class="flex items-start gap-4">
                                    <Icon
                                        icon=icondata_lu::LuDatabase
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
                        <div class="border border-border rounded-xl p-5">
                            <div class="flex items-start gap-4">
                                <Icon
                                    icon=icondata_lu::LuDatabase
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
    #[cfg(feature = "hydrate")]
    {
        Effect::new(move |_| {
            use wasm_bindgen::prelude::*;
            use wasm_bindgen::JsCast;

            let Some(window) = web_sys::window() else {
                return;
            };

            // OAuth success message types per provider
            let success_types: Vec<String> = [
                "GOOGLE_OAUTH_SUCCESS",
                "BIGQUERY_ENTERPRISE_OAUTH_SUCCESS",
                "SNOWFLAKE_OAUTH_SUCCESS",
                "MICROSOFT_ENTERPRISE_OAUTH_SUCCESS",
                "DATABRICKS_OAUTH_SUCCESS",
            ]
            .iter()
            .map(|s| s.to_string())
            .collect();

            let error_types: Vec<String> = [
                "GOOGLE_OAUTH_ERROR",
                "BIGQUERY_ENTERPRISE_OAUTH_ERROR",
                "SNOWFLAKE_OAUTH_ERROR",
                "MICROSOFT_ENTERPRISE_OAUTH_ERROR",
                "DATABRICKS_OAUTH_ERROR",
            ]
            .iter()
            .map(|s| s.to_string())
            .collect();

            let closure = Closure::<dyn Fn(web_sys::MessageEvent)>::new(
                move |event: web_sys::MessageEvent| {
                    // Verify origin matches current window
                    if let Some(win) = web_sys::window() {
                        let origin = win.location().origin().unwrap_or_default();
                        if event.origin() != origin {
                            return;
                        }
                    }

                    // Parse message data
                    let data = event.data();
                    let msg_type = js_sys::Reflect::get(&data, &JsValue::from_str("type"))
                        .ok()
                        .and_then(|v| v.as_string());

                    let Some(ref msg_type) = msg_type else {
                        return;
                    };

                    if success_types.iter().any(|t| t == msg_type) {
                        set_oauth_connecting.set(None);
                        let provider = match msg_type.as_str() {
                            "GOOGLE_OAUTH_SUCCESS" | "BIGQUERY_ENTERPRISE_OAUTH_SUCCESS" => {
                                "BigQuery"
                            }
                            "SNOWFLAKE_OAUTH_SUCCESS" => "Snowflake",
                            "MICROSOFT_ENTERPRISE_OAUTH_SUCCESS" => "Azure Synapse",
                            "DATABRICKS_OAUTH_SUCCESS" => "Databricks",
                            _ => "datasource",
                        };
                        toast_success(format!("{provider} connected successfully"));
                        // Trigger re-fetch — the Effect above handles redirect/update
                        state_resource.refetch();
                    } else if error_types.iter().any(|t| t == msg_type) {
                        set_oauth_connecting.set(None);
                        let error_msg =
                            js_sys::Reflect::get(&data, &JsValue::from_str("error"))
                                .ok()
                                .and_then(|v| v.as_string())
                                .unwrap_or_else(|| "Failed to connect".to_string());
                        toast_error(error_msg);
                    }
                },
            );

            let _ = window.add_event_listener_with_callback(
                "message",
                closure.as_ref().unchecked_ref(),
            );

            // Store closure in SendWrapper so it can be cleaned up on unmount
            let wrapper = send_wrapper::SendWrapper::new(closure);
            on_cleanup(move || {
                drop(wrapper);
            });
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
                        <h1 class="text-3xl font-bold mb-2">"Set Up Your Credentials"</h1>
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
                        open_oauth_popup(&url, &ds_id, set_oauth_connecting);
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
                <Icon icon=icondata_lu::LuKey attr:class="h-4 w-4 mr-2"/>
                "Enter Credentials"
            </Button>
        }
        .into_any()
    } else {
        // No action needed (shared/connect datasources shouldn't appear here)
        view! {}.into_any()
    };

    view! {
        <div class="flex items-center justify-between p-4 border border-border rounded-xl bg-card">
            <div class="flex items-center gap-3">
                <Icon
                    icon=icondata_lu::LuDatabase
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
            if let Some(Ok(state)) = state_resource.get() {
                if state.has_datasources {
                    nav(
                        "/onboarding",
                        NavigateOptions {
                            replace: true,
                            ..Default::default()
                        },
                    );
                }
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
                        <h1 class="text-3xl font-bold mb-2">"Waiting for Setup"</h1>
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
// OAuth Popup Helper
// ─────────────────────────────────────────────────────────────────────────────

/// Open a centered OAuth popup window and monitor it for closure.
///
/// When the popup is closed (either by the user or after OAuth completion),
/// clears the connecting state. OAuth success/error is handled separately
/// via the `message` event listener.
fn open_oauth_popup(
    url: &str,
    datasource_id: &str,
    set_connecting: WriteSignal<Option<String>>,
) {
    #[cfg(feature = "hydrate")]
    {
        use wasm_bindgen::prelude::*;
        use wasm_bindgen::JsCast;

        let Some(window) = web_sys::window() else {
            toast_error("Browser window not available");
            return;
        };

        // Calculate centered popup position
        let width = 500;
        let height = 600;
        let screen_x = window.screen_x().ok().and_then(|v| v.as_f64()).unwrap_or(0.0) as i32;
        let outer_width = window.outer_width().ok().and_then(|v| v.as_f64()).unwrap_or(1024.0) as i32;
        let screen_y = window.screen_y().ok().and_then(|v| v.as_f64()).unwrap_or(0.0) as i32;
        let outer_height = window.outer_height().ok().and_then(|v| v.as_f64()).unwrap_or(768.0) as i32;
        let left = screen_x + (outer_width - width) / 2;
        let top = screen_y + (outer_height - height) / 2;

        let features = format!(
            "width={width},height={height},left={left},top={top},popup=1"
        );

        let popup = window.open_with_url_and_target_and_features(
            url,
            "oauth-connect",
            &features,
        );

        match popup {
            Ok(Some(popup_window)) => {
                // Monitor popup for manual close. When the popup closes
                // (either by user or after OAuth redirect), clear the
                // connecting state and stop polling.
                let ds_id = datasource_id.to_string();

                // Use shared state so the closure can clear its own interval
                // and drop itself (no leak via forget).
                let state: std::rc::Rc<std::cell::RefCell<Option<(i32, Closure<dyn Fn()>)>>> =
                    std::rc::Rc::new(std::cell::RefCell::new(None));
                let state_inner = state.clone();

                let closure = Closure::<dyn Fn()>::new(move || {
                    let closed = popup_window.closed().unwrap_or(true);
                    if closed {
                        // Clear connecting state if still set to this datasource
                        set_connecting.update(|current| {
                            if current.as_deref() == Some(ds_id.as_str()) {
                                *current = None;
                            }
                        });
                        // Self-clear the interval and drop the closure
                        if let Some((interval_id, _)) = state_inner.borrow().as_ref() {
                            if let Some(win) = web_sys::window() {
                                win.clear_interval_with_handle(*interval_id);
                            }
                        }
                        state_inner.borrow_mut().take();
                    }
                });

                let id = window
                    .set_interval_with_callback_and_timeout_and_arguments_0(
                        closure.as_ref().unchecked_ref(),
                        500, // Check every 500ms
                    )
                    .unwrap_or(0);

                // Store both the interval ID and the closure so the closure stays
                // alive without forget(). When the popup closes, both are dropped.
                *state.borrow_mut() = Some((id, closure));
            }
            _ => {
                set_connecting.set(None);
                toast_error("Popup was blocked. Please allow popups for this site.");
            }
        }
    }

    #[cfg(not(feature = "hydrate"))]
    {
        let _ = (url, datasource_id, set_connecting);
    }
}
