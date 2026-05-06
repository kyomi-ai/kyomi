// SPDX-License-Identifier: AGPL-3.0-or-later

//! Accept Ownership page — matches `apps/frontend/src/pages/AcceptOwnershipPage.jsx`.
//!
//! Route: `/accept-ownership/:transfer_id`
//!
//! Standalone page (no layout wrapper) with 5 states:
//! 1. Loading — spinner + "Loading transfer details..."
//! 2. Error — transfer not found / expired / already processed
//! 3. Ready — transfer details, warning, capabilities, action buttons
//! 4. Processing — buttons disabled with spinners
//! 5. Success — green checkmark, auto-redirect to `/settings/team`

use leptos::prelude::*;
use leptos_router::hooks::use_params_map;

use crate::components::{
    Alert, AlertDescription, AlertTitle, AlertVariant, Button, ButtonVariant,
};
use crate::server_fns::ownership::{
    accept_ownership_transfer, decline_ownership_transfer, get_ownership_transfer,
    OwnershipTransfer,
};

// ─────────────────────────────────────────────────────────────────────────────
// State machine
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq)]
enum PageState {
    Loading,
    Error { message: String },
    Ready { transfer: OwnershipTransfer },
    Processing { transfer: OwnershipTransfer },
    Success { workspace_name: String },
}

/// Fetch the ownership transfer and return the resulting page state.
/// Not cfg-gated so the compiler sees all `PageState` variants constructed.
async fn fetch_ownership_transfer(transfer_id: String) -> PageState {
    match get_ownership_transfer(transfer_id).await {
        Ok(Some(transfer)) => PageState::Ready { transfer },
        Ok(None) => PageState::Error {
            message: "Transfer request not found or has expired".to_string(),
        },
        Err(e) => PageState::Error {
            message: format!("Failed to load transfer details: {e}"),
        },
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Main component
// ─────────────────────────────────────────────────────────────────────────────

#[component]
pub fn AcceptOwnershipPage() -> impl IntoView {
    let (state, set_state) = signal(PageState::Loading);
    let params = use_params_map();

    // ── Fetch transfer on mount ──────────────────────────────────────────
    // Extract transfer_id from URL params (browser-only); SSR provides empty string.
    #[cfg(target_arch = "wasm32")]
    let transfer_id = params.read().get("transfer_id").unwrap_or_default();
    #[cfg(not(target_arch = "wasm32"))]
    let transfer_id = {
        let _ = &params;
        String::new()
    };

    // spawn_local compiles on both targets; the extracted function ensures
    // the compiler sees all PageState variants constructed.
    {
        leptos::task::spawn_local(async move {
            if transfer_id.is_empty() {
                set_state.set(PageState::Error {
                    message: "No transfer ID provided".to_string(),
                });
                return;
            }
            set_state.set(fetch_ownership_transfer(transfer_id).await);
        });
    }

    // ── Accept handler ───────────────────────────────────────────────────
    let on_accept = move |_| {
        let current = state.get_untracked();
        let transfer = match current {
            PageState::Ready { transfer } => transfer,
            _ => return,
        };

        let workspace_name = transfer.workspace_name.clone();
        let transfer_id = transfer.transfer_id.clone();
        set_state.set(PageState::Processing {
            transfer: transfer.clone(),
        });

        leptos::task::spawn_local(async move {
            match accept_ownership_transfer(transfer_id).await {
                Ok(()) => {
                    set_state.set(PageState::Success { workspace_name });

                    // Auto-redirect to /settings/team after 3 seconds
                    #[cfg(target_arch = "wasm32")]
                    {
                        use wasm_bindgen::prelude::*;
                        if let Some(window) = web_sys::window() {
                            let closure = Closure::once(move || {
                                if let Some(window) = web_sys::window() {
                                    let _ = window.location().set_href("/settings/team");
                                }
                            });
                            let _ = window
                                .set_timeout_with_callback_and_timeout_and_arguments_0(
                                    closure.as_ref().unchecked_ref(),
                                    3000,
                                );
                            closure.forget();
                        }
                    }
                }
                Err(e) => {
                    crate::components::toast::toast_error(format!("Failed to accept transfer: {e}"));
                    set_state.set(PageState::Ready { transfer });
                }
            }
        });
    };

    // ── Decline handler ──────────────────────────────────────────────────
    let on_decline = move |_| {
        let current = state.get_untracked();
        let transfer = match current {
            PageState::Ready { transfer } => transfer,
            _ => return,
        };

        let transfer_id = transfer.transfer_id.clone();
        set_state.set(PageState::Processing {
            transfer: transfer.clone(),
        });

        leptos::task::spawn_local(async move {
            match decline_ownership_transfer(transfer_id).await {
                Ok(()) => {
                    crate::components::toast::toast_success("Transfer request declined");

                    // Redirect to dashboard after 2 seconds
                    #[cfg(target_arch = "wasm32")]
                    {
                        use wasm_bindgen::prelude::*;
                        if let Some(window) = web_sys::window() {
                            let closure = Closure::once(move || {
                                if let Some(window) = web_sys::window() {
                                    let _ = window.location().set_href("/");
                                }
                            });
                            let _ = window
                                .set_timeout_with_callback_and_timeout_and_arguments_0(
                                    closure.as_ref().unchecked_ref(),
                                    2000,
                                );
                            closure.forget();
                        }
                    }
                }
                Err(e) => {
                    crate::components::toast::toast_error(format!("Failed to decline transfer: {e}"));
                    set_state.set(PageState::Ready { transfer });
                }
            }
        });
    };

    // ── Render ────────────────────────────────────────────────────────────
    view! {
        <div class="min-h-screen bg-gradient-to-br from-background via-muted/30 to-muted/50 flex items-center justify-center p-4">
            <div class="w-full max-w-2xl">
                <div class="bg-card/80 backdrop-blur-sm rounded-lg shadow border border-border overflow-hidden">
                    // Header
                    <div class="p-8 text-center">
                        <div class="w-20 h-20 bg-primary/10 rounded-lg flex items-center justify-center mx-auto mb-6">
                            {move || {
                                let s = state.get();
                                match &s {
                                    PageState::Success { .. } => icon_check_circle_large().into_any(),
                                    PageState::Error { .. } => icon_alert_circle_large().into_any(),
                                    _ => icon_arrow_right_left().into_any(),
                                }
                            }}
                        </div>
                        <h1 class="text-xl font-semibold text-foreground mb-2">
                            "Workspace Ownership Transfer"
                        </h1>
                        <p class="text-muted-foreground">
                            {move || {
                                let s = state.get();
                                match &s {
                                    PageState::Loading => "Loading transfer details...".to_string(),
                                    PageState::Ready { .. } => "You have been offered workspace ownership".to_string(),
                                    PageState::Processing { .. } => "Processing your response...".to_string(),
                                    PageState::Success { .. } => "Transfer accepted successfully!".to_string(),
                                    PageState::Error { .. } => "Transfer request unavailable".to_string(),
                                }
                            }}
                        </p>
                    </div>

                    // Content section
                    <div class="px-8 pb-8">
                        {move || {
                            let s = state.get();
                            match s {
                                PageState::Loading => loading_view().into_any(),
                                PageState::Error { message } => error_view(message).into_any(),
                                PageState::Ready { transfer } => {
                                    ready_view(transfer, false, on_accept, on_decline).into_any()
                                }
                                PageState::Processing { .. } => {
                                    view! {
                                        <div class="text-center py-8">
                                            <div class="inline-flex items-center justify-center w-16 h-16 rounded-full bg-muted mb-4">
                                                <crate::components::Spinner class="h-8 w-8" />
                                            </div>
                                            <p class="text-muted-foreground mt-4">"Processing..."</p>
                                        </div>
                                    }.into_any()
                                }
                                PageState::Success { workspace_name } => {
                                    success_view(workspace_name).into_any()
                                }
                            }
                        }}
                    </div>
                </div>

                // Footer
                <div class="text-center mt-8">
                    <p class="text-sm text-muted-foreground">
                        "Need help? Contact "
                        <a href="mailto:support@kyomi.dev" class="text-primary hover:underline">
                            "support@kyomi.dev"
                        </a>
                    </p>
                </div>
            </div>
        </div>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Loading view
// ─────────────────────────────────────────────────────────────────────────────

fn loading_view() -> impl IntoView {
    view! {
        <div class="text-center py-8">
            {spinner_xl()}
            <p class="text-muted-foreground mt-4">"Please wait..."</p>
        </div>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Error view
// ─────────────────────────────────────────────────────────────────────────────

fn error_view(message: String) -> impl IntoView {
    view! {
        <div class="space-y-4">
            <Alert variant=AlertVariant::Error>
                {icon_alert_circle_sm()}
                <div class="ml-2">
                    <AlertTitle>"Error"</AlertTitle>
                    <AlertDescription>{message}</AlertDescription>
                </div>
            </Alert>
            <div class="text-center">
                <a href="/">
                    <Button variant=ButtonVariant::Outline>
                        "Go to Dashboard"
                    </Button>
                </a>
            </div>
        </div>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Ready / Processing view
// ─────────────────────────────────────────────────────────────────────────────

fn ready_view(
    transfer: OwnershipTransfer,
    is_processing: bool,
    on_accept: impl Fn(leptos::ev::MouseEvent) + Send + 'static,
    on_decline: impl Fn(leptos::ev::MouseEvent) + Send + 'static,
) -> impl IntoView {
    let workspace_name = transfer.workspace_name.clone();
    let from_email = transfer.from_user_email.clone();
    let expires_at = transfer.expires_at.clone();
    let date_script = format_date_script(expires_at.clone());

    view! {
        <div class="space-y-6">
            // Transfer info card
            <div class="bg-muted/50 rounded-lg p-6 border border-border">
                <div class="space-y-4">
                    // Workspace
                    <div class="flex items-start gap-3">
                        {icon_building2()}
                        <div class="flex-1">
                            <div class="text-sm text-muted-foreground">"Workspace"</div>
                            <div class="text-lg font-semibold text-foreground">
                                {workspace_name}
                            </div>
                        </div>
                    </div>

                    // Current owner
                    <div class="flex items-start gap-3">
                        {icon_user()}
                        <div class="flex-1">
                            <div class="text-sm text-muted-foreground">"Current Owner"</div>
                            <div class="text-lg font-medium text-foreground">
                                {from_email}
                            </div>
                        </div>
                    </div>

                    // Expiration
                    <div class="pt-2 border-t border-border">
                        <div class="text-sm text-muted-foreground">"Expires"</div>
                        <div class="text-foreground" id="expires-at-display">
                            {expires_at.clone()}
                        </div>
                        // Format the date client-side
                        {date_script}
                    </div>
                </div>
            </div>

            // Warning alert
            <Alert variant=AlertVariant::Warning>
                {icon_alert_circle_sm()}
                <div class="ml-2">
                    <AlertTitle>"Important"</AlertTitle>
                    <AlertDescription>
                        "By accepting ownership, you will become the workspace owner with full control over billing, settings, and member management. The current owner will be downgraded to a workspace admin."
                    </AlertDescription>
                </div>
            </Alert>

            // Capabilities list
            <div class="bg-muted/50 rounded-lg p-6 border border-border">
                <h3 class="font-semibold text-foreground mb-3">
                    "As the workspace owner, you will be able to:"
                </h3>
                <ul class="space-y-2 text-sm text-muted-foreground">
                    {capability_item("Manage workspace billing and subscription")}
                    {capability_item("Delete the workspace")}
                    {capability_item("Add and remove workspace members")}
                    {capability_item("Transfer ownership to another member")}
                    {capability_item("Configure workspace settings and integrations")}
                </ul>
            </div>

            // Action buttons
            <div class="flex gap-3 justify-end">
                <button
                    class="inline-flex items-center justify-center gap-2 whitespace-nowrap rounded-md text-sm font-medium transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:pointer-events-none disabled:opacity-50 border border-input bg-background text-foreground shadow-sm hover:bg-secondary hover:text-accent-foreground h-9 px-6"
                    disabled=is_processing
                    on:click=on_decline
                >
                    {if is_processing {
                        view! {
                            <div class="flex items-center gap-2">
                                {spinner_sm()}
                                <span>"Declining..."</span>
                            </div>
                        }.into_any()
                    } else {
                        view! { <span>"Decline"</span> }.into_any()
                    }}
                </button>
                <button
                    class="inline-flex items-center justify-center gap-2 whitespace-nowrap rounded-md text-sm font-medium transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:pointer-events-none disabled:opacity-50 bg-primary text-primary-foreground shadow hover:bg-primary/90 h-9 px-6"
                    disabled=is_processing
                    on:click=on_accept
                >
                    {if is_processing {
                        view! {
                            <div class="flex items-center gap-2">
                                {spinner_sm()}
                                <span>"Accepting..."</span>
                            </div>
                        }.into_any()
                    } else {
                        view! { <span>"Accept Ownership"</span> }.into_any()
                    }}
                </button>
            </div>
        </div>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Success view
// ─────────────────────────────────────────────────────────────────────────────

fn success_view(workspace_name: String) -> impl IntoView {
    view! {
        <div class="space-y-4">
            <div class="text-center py-8">
                {icon_check_circle_success()}
                <div class="mt-4 font-semibold text-success-foreground">"Success!"</div>
                <p class="text-muted-foreground mt-2">
                    "You are now the owner of " {workspace_name} "."
                </p>
            </div>
            <Alert variant=AlertVariant::Success>
                {icon_check_sm()}
                <div class="ml-2">
                    <AlertDescription>
                        "Redirecting to workspace settings in 3 seconds..."
                    </AlertDescription>
                </div>
            </Alert>
            <div class="text-center">
                <a href="/settings/team">
                    <Button>"Go to Settings Now"</Button>
                </a>
            </div>
        </div>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Capability list item
// ─────────────────────────────────────────────────────────────────────────────

fn capability_item(text: &'static str) -> impl IntoView {
    view! {
        <li class="flex items-start gap-2">
            <svg
                class="h-4 w-4 text-success-foreground mt-0.5 flex-shrink-0"
                xmlns="http://www.w3.org/2000/svg"
                width="24" height="24" viewBox="0 0 24 24"
                fill="none" stroke="currentColor"
                stroke-width="2" stroke-linecap="round" stroke-linejoin="round"
            >
                <path d="M22 11.08V12a10 10 0 1 1-5.93-9.14" />
                <path d="m9 11 3 3L22 4" />
            </svg>
            <span>{text}</span>
        </li>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Client-side date formatting
// ─────────────────────────────────────────────────────────────────────────────

/// Render a small inline script that formats an ISO date string into the
/// user's local date/time and replaces the placeholder element content.
fn format_date_script(iso_date: String) -> impl IntoView {
    let script = format!(
        r#"(function(){{var el=document.getElementById('expires-at-display');if(el){{var d=new Date('{}');el.textContent=d.toLocaleDateString()+' at '+d.toLocaleTimeString();}}}})();"#,
        iso_date
    );
    view! {
        <script>{script}</script>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// SVG icons — inline to avoid npm/lucide dependency
// ─────────────────────────────────────────────────────────────────────────────

/// Spinner (Loader2) — xl size for loading states.
fn spinner_xl() -> impl IntoView {
    view! {
        <svg
            class="animate-spin h-12 w-12 text-primary mx-auto"
            xmlns="http://www.w3.org/2000/svg"
            width="24" height="24" viewBox="0 0 24 24"
            fill="none" stroke="currentColor"
            stroke-width="2" stroke-linecap="round" stroke-linejoin="round"
        >
            <path d="M21 12a9 9 0 1 1-6.219-8.56" />
        </svg>
    }
}

/// Spinner (Loader2) — sm size for inside buttons.
fn spinner_sm() -> impl IntoView {
    view! {
        <svg
            class="animate-spin h-4 w-4"
            xmlns="http://www.w3.org/2000/svg"
            width="24" height="24" viewBox="0 0 24 24"
            fill="none" stroke="currentColor"
            stroke-width="2" stroke-linecap="round" stroke-linejoin="round"
        >
            <path d="M21 12a9 9 0 1 1-6.219-8.56" />
        </svg>
    }
}

/// ArrowRightLeft icon — header icon for ready/loading states.
fn icon_arrow_right_left() -> impl IntoView {
    view! {
        <svg
            class="text-primary"
            xmlns="http://www.w3.org/2000/svg"
            width="32" height="32" viewBox="0 0 24 24"
            fill="none" stroke="currentColor"
            stroke-width="2" stroke-linecap="round" stroke-linejoin="round"
        >
            <path d="m16 3 4 4-4 4" />
            <path d="M20 7H4" />
            <path d="m8 21-4-4 4-4" />
            <path d="M4 17h16" />
        </svg>
    }
}

/// Large CheckCircle icon — header icon for success state.
fn icon_check_circle_large() -> impl IntoView {
    view! {
        <svg
            class="text-success-foreground"
            xmlns="http://www.w3.org/2000/svg"
            width="32" height="32" viewBox="0 0 24 24"
            fill="none" stroke="currentColor"
            stroke-width="2" stroke-linecap="round" stroke-linejoin="round"
        >
            <path d="M22 11.08V12a10 10 0 1 1-5.93-9.14" />
            <path d="m9 11 3 3L22 4" />
        </svg>
    }
}

/// Large AlertCircle icon — header icon for error state.
fn icon_alert_circle_large() -> impl IntoView {
    view! {
        <svg
            class="text-destructive"
            xmlns="http://www.w3.org/2000/svg"
            width="32" height="32" viewBox="0 0 24 24"
            fill="none" stroke="currentColor"
            stroke-width="2" stroke-linecap="round" stroke-linejoin="round"
        >
            <circle cx="12" cy="12" r="10" />
            <line x1="12" x2="12" y1="8" y2="12" />
            <line x1="12" x2="12.01" y1="16" y2="16" />
        </svg>
    }
}

/// Small AlertCircle icon — for use inside Alert components.
fn icon_alert_circle_sm() -> impl IntoView {
    view! {
        <svg
            class="h-4 w-4"
            xmlns="http://www.w3.org/2000/svg"
            width="24" height="24" viewBox="0 0 24 24"
            fill="none" stroke="currentColor"
            stroke-width="2" stroke-linecap="round" stroke-linejoin="round"
        >
            <circle cx="12" cy="12" r="10" />
            <line x1="12" x2="12" y1="8" y2="12" />
            <line x1="12" x2="12.01" y1="16" y2="16" />
        </svg>
    }
}

/// CheckCircle icon — large for success view body.
fn icon_check_circle_success() -> impl IntoView {
    view! {
        <svg
            class="h-16 w-16 text-success-foreground mx-auto"
            xmlns="http://www.w3.org/2000/svg"
            width="24" height="24" viewBox="0 0 24 24"
            fill="none" stroke="currentColor"
            stroke-width="2" stroke-linecap="round" stroke-linejoin="round"
        >
            <path d="M22 11.08V12a10 10 0 1 1-5.93-9.14" />
            <path d="m9 11 3 3L22 4" />
        </svg>
    }
}

/// Small CheckCircle icon — for success alert.
fn icon_check_sm() -> impl IntoView {
    view! {
        <svg
            class="h-4 w-4"
            xmlns="http://www.w3.org/2000/svg"
            width="24" height="24" viewBox="0 0 24 24"
            fill="none" stroke="currentColor"
            stroke-width="2" stroke-linecap="round" stroke-linejoin="round"
        >
            <path d="M22 11.08V12a10 10 0 1 1-5.93-9.14" />
            <path d="m9 11 3 3L22 4" />
        </svg>
    }
}

/// Building2 icon — workspace info.
fn icon_building2() -> impl IntoView {
    view! {
        <svg
            class="h-5 w-5 text-muted-foreground mt-0.5"
            xmlns="http://www.w3.org/2000/svg"
            width="24" height="24" viewBox="0 0 24 24"
            fill="none" stroke="currentColor"
            stroke-width="2" stroke-linecap="round" stroke-linejoin="round"
        >
            <path d="M6 22V4a2 2 0 0 1 2-2h8a2 2 0 0 1 2 2v18Z" />
            <path d="M6 12H4a2 2 0 0 0-2 2v6a2 2 0 0 0 2 2h2" />
            <path d="M18 9h2a2 2 0 0 1 2 2v9a2 2 0 0 1-2 2h-2" />
            <path d="M10 6h4" />
            <path d="M10 10h4" />
            <path d="M10 14h4" />
            <path d="M10 18h4" />
        </svg>
    }
}

/// User icon — owner info.
fn icon_user() -> impl IntoView {
    view! {
        <svg
            class="h-5 w-5 text-muted-foreground mt-0.5"
            xmlns="http://www.w3.org/2000/svg"
            width="24" height="24" viewBox="0 0 24 24"
            fill="none" stroke="currentColor"
            stroke-width="2" stroke-linecap="round" stroke-linejoin="round"
        >
            <path d="M19 21v-2a4 4 0 0 0-4-4H9a4 4 0 0 0-4 4v2" />
            <circle cx="12" cy="7" r="4" />
        </svg>
    }
}
