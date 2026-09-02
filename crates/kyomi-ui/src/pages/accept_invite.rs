// SPDX-License-Identifier: AGPL-3.0-or-later

//! Accept Invite page — dedicated landing page for workspace invitations.
//!
//! Route: `/accept-invite/:invitation_id`
//!
//! Standalone page (no layout wrapper) with 5 states, mirroring
//! `accept_ownership.rs`:
//! 1. Loading — spinner + "Loading invitation details..."
//! 2. Error — invitation not found / expired / already processed
//! 3. Ready — invitation details, action buttons
//! 4. Processing — buttons disabled with spinners
//! 5. Success — green checkmark, auto-redirect to `/`

use leptos::prelude::*;
use leptos_router::hooks::use_params_map;

use crate::components::{
    Alert, AlertDescription, AlertTitle, AlertVariant, Button, ButtonVariant,
};
use crate::server_fns::profile::{
    accept_invitation, decline_invitation, get_invitation_for_accept, InvitationDisplay,
};

// ─────────────────────────────────────────────────────────────────────────────
// State machine
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq)]
enum PageState {
    Loading,
    Error { message: String },
    Ready { invitation: InvitationDisplay },
    Processing { invitation: InvitationDisplay },
    Success { workspace_name: String },
}

/// Fetch the invitation and return the resulting page state.
/// Not cfg-gated so the compiler sees all `PageState` variants constructed.
async fn fetch_invitation(invitation_id: String) -> PageState {
    match get_invitation_for_accept(invitation_id).await {
        Ok(Some(invitation)) => {
            if invitation.status != "pending" {
                return PageState::Error {
                    message: format!("This invitation has already been {}.", invitation.status),
                };
            }
            match chrono::DateTime::parse_from_rfc3339(&invitation.expires_at) {
                Ok(expires_at) if expires_at < chrono::Utc::now() => PageState::Error {
                    message: "This invitation has expired.".to_string(),
                },
                _ => PageState::Ready { invitation },
            }
        }
        Ok(None) => PageState::Error {
            message: "This invitation was not found or is no longer available.".to_string(),
        },
        Err(e) => PageState::Error {
            message: format!("Failed to load invitation details: {e}"),
        },
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Main component
// ─────────────────────────────────────────────────────────────────────────────

#[component]
pub fn AcceptInvitePage() -> impl IntoView {
    let (state, set_state) = signal(PageState::Loading);
    let params = use_params_map();

    // ── Fetch invitation on mount ────────────────────────────────────────
    // Extract invitation_id from URL params (browser-only); SSR provides empty string.
    #[cfg(target_arch = "wasm32")]
    let invitation_id = params.read().get("invitation_id").unwrap_or_default();
    #[cfg(not(target_arch = "wasm32"))]
    let invitation_id = {
        let _ = &params;
        String::new()
    };

    // spawn_local compiles on both targets; the extracted function ensures
    // the compiler sees all PageState variants constructed.
    {
        leptos::task::spawn_local(async move {
            if invitation_id.is_empty() {
                set_state.try_set(PageState::Error {
                    message: "No invitation ID provided".to_string(),
                });
                return;
            }
            set_state.try_set(fetch_invitation(invitation_id).await);
        });
    }

    // ── Accept handler ───────────────────────────────────────────────────
    let on_accept = move |_| {
        let current = state.get_untracked();
        let invitation = match current {
            PageState::Ready { invitation } => invitation,
            _ => return,
        };

        let workspace_name = invitation.workspace_name.clone();
        let invitation_id = invitation.invitation_id.clone();
        set_state.set(PageState::Processing {
            invitation: invitation.clone(),
        });

        leptos::task::spawn_local(async move {
            match accept_invitation(invitation_id).await {
                Ok(()) => {
                    set_state.try_set(PageState::Success { workspace_name });

                    // Auto-redirect to / after 3 seconds
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
                                    3000,
                                );
                            closure.forget();
                        }
                    }
                }
                Err(e) => {
                    crate::components::toast::toast_error(format!("Failed to accept invitation: {e}"));
                    set_state.try_set(PageState::Ready { invitation });
                }
            }
        });
    };

    // ── Decline handler ──────────────────────────────────────────────────
    let on_decline = move |_| {
        let current = state.get_untracked();
        let invitation = match current {
            PageState::Ready { invitation } => invitation,
            _ => return,
        };

        let invitation_id = invitation.invitation_id.clone();
        set_state.set(PageState::Processing {
            invitation: invitation.clone(),
        });

        leptos::task::spawn_local(async move {
            match decline_invitation(invitation_id).await {
                Ok(()) => {
                    crate::components::toast::toast_success("Invitation declined");

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
                    crate::components::toast::toast_error(format!("Failed to decline invitation: {e}"));
                    set_state.try_set(PageState::Ready { invitation });
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
                                    _ => icon_mail().into_any(),
                                }
                            }}
                        </div>
                        <h1 class="text-xl font-semibold text-foreground mb-2">
                            "Workspace Invitation"
                        </h1>
                        <p class="text-muted-foreground">
                            {move || {
                                let s = state.get();
                                match &s {
                                    PageState::Loading => "Loading invitation details...".to_string(),
                                    PageState::Ready { .. } => "You've been invited to join a workspace".to_string(),
                                    PageState::Processing { .. } => "Processing your response...".to_string(),
                                    PageState::Success { .. } => "Invitation accepted successfully!".to_string(),
                                    PageState::Error { .. } => "Invitation unavailable".to_string(),
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
                                PageState::Ready { invitation } => {
                                    ready_view(invitation, false, on_accept, on_decline).into_any()
                                }
                                PageState::Processing { .. } => {
                                    view! {
                                        <div class="text-center py-8 space-y-4">
                                            // Branded moment — DESIGN.md Loading State Pattern
                                            <img
                                                src="/kyomi_animated_logo.svg"
                                                alt="Processing"
                                                class="w-12 h-12 mx-auto"
                                            />
                                            <p class="text-muted-foreground">"Processing..."</p>
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
                        // Hardcoded rather than read from `Config::support_email`
                        // (`crates/kyomi-core/src/config.rs`): this page is on a
                        // failure path where the mailto is the user's only remaining
                        // option, and `Config` lives on `ServerContext`, which is
                        // `#[cfg(feature = "ssr")]`-only (`server_fns/mod.rs`) and
                        // does not exist on this component's `hydrate`/wasm32 build.
                        // Reading it would require a server-fn round trip for a
                        // static string. Same shape as the KYO-478 fix on the login
                        // page (`pages/auth/login.rs`) — keep this literal in sync
                        // with `config.rs`'s default.
                        <a href="mailto:support@kyomi.ai" class="text-primary hover:underline">
                            "support@kyomi.ai"
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
        <div class="text-center py-8 space-y-4">
            // Branded moment — DESIGN.md Loading State Pattern
            <img
                src="/kyomi_animated_logo.svg"
                alt="Processing"
                class="w-12 h-12 mx-auto"
            />
            <p class="text-muted-foreground">"Please wait..."</p>
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
    invitation: InvitationDisplay,
    is_processing: bool,
    on_accept: impl Fn(leptos::ev::MouseEvent) + Send + 'static,
    on_decline: impl Fn(leptos::ev::MouseEvent) + Send + 'static,
) -> impl IntoView {
    let workspace_name = invitation.workspace_name.clone();
    let inviter_name = invitation.inviter_name.clone();
    let role = invitation.role.clone();
    let expires_at = invitation.expires_at.clone();
    let date_script = format_date_script(expires_at.clone());

    view! {
        <div class="space-y-6">
            // Invitation info card
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

                    // Inviter
                    <div class="flex items-start gap-3">
                        {icon_user()}
                        <div class="flex-1">
                            <div class="text-sm text-muted-foreground">"Invited by"</div>
                            <div class="text-lg font-medium text-foreground">
                                {inviter_name}
                            </div>
                        </div>
                    </div>

                    // Role
                    <div class="flex items-start gap-3">
                        {icon_shield()}
                        <div class="flex-1">
                            <div class="text-sm text-muted-foreground">"Role"</div>
                            <div class="text-lg font-medium text-foreground">
                                {role}
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

            // Action buttons
            <div class="flex gap-3 justify-end">
                <Button
                    variant=ButtonVariant::Outline
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
                </Button>
                <Button
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
                        view! { <span>"Accept Invitation"</span> }.into_any()
                    }}
                </Button>
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
                    "You've joined " {workspace_name} "."
                </p>
            </div>
            <Alert variant=AlertVariant::Success>
                {icon_check_sm()}
                <div class="ml-2">
                    <AlertDescription>
                        "Redirecting to Kyomi in 3 seconds..."
                    </AlertDescription>
                </div>
            </Alert>
            <div class="text-center">
                <a href="/">
                    <Button>"Go to Kyomi Now"</Button>
                </a>
            </div>
        </div>
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

/// Mail icon — header icon for ready/loading states.
fn icon_mail() -> impl IntoView {
    view! {
        <svg
            class="text-primary"
            xmlns="http://www.w3.org/2000/svg"
            width="32" height="32" viewBox="0 0 24 24"
            fill="none" stroke="currentColor"
            stroke-width="2" stroke-linecap="round" stroke-linejoin="round"
        >
            <rect width="20" height="16" x="2" y="4" rx="2" />
            <path d="m22 7-8.97 5.7a1.94 1.94 0 0 1-2.06 0L2 7" />
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

/// User icon — inviter info.
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

/// Shield icon — role info.
fn icon_shield() -> impl IntoView {
    view! {
        <svg
            class="h-5 w-5 text-muted-foreground mt-0.5"
            xmlns="http://www.w3.org/2000/svg"
            width="24" height="24" viewBox="0 0 24 24"
            fill="none" stroke="currentColor"
            stroke-width="2" stroke-linecap="round" stroke-linejoin="round"
        >
            <path d="M20 13c0 5-3.5 7.5-7.66 8.95a1 1 0 0 1-.67-.01C7.5 20.5 4 18 4 13V6a1 1 0 0 1 1-1c2 0 4.5-1.2 6.24-2.72a1.17 1.17 0 0 1 1.52 0C14.51 3.81 17 5 19 5a1 1 0 0 1 1 1z" />
        </svg>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests (KYO-482)
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use crate::test_support::extract_between;

    /// This file's own source, for a source-text wiring assertion below —
    /// mirrors the `SRC`/`extract_between` pattern in `pages/auth/login.rs`
    /// (KYO-478), kept local here since this file is far below the size
    /// where `docs/standards/code-organization/one-test-topic-per-file-not-one-big-mod-tests.md`
    /// requires splitting `mod tests` out into its own directory.
    const SRC: &str = include_str!("accept_invite.rs");

    /// Pins the real support domain (kyomi.ai, not kyomi.dev — confirmed
    /// against `kyomi_core::Config::support_email`'s default in
    /// `crates/kyomi-core/src/config.rs`). This page is the invite's last
    /// resort: the invite already failed and the mailto is the user's only
    /// remaining option, possibly without an account to sign back in with.
    /// A link to a domain Kyomi doesn't own means the mail goes nowhere
    /// with no bounce — the user never hears back and we never learn they
    /// were stuck (KYO-482).
    #[test]
    fn footer_support_link_points_at_kyomi_ai() {
        let footer_block =
            extract_between(SRC, "\"Need help? Contact \"", "</div>");
        assert!(
            footer_block.contains("mailto:support@kyomi.ai"),
            "the footer support link must point at support@kyomi.ai — the address \
             kyomi_core::Config::support_email defaults to — not any other domain"
        );
        assert!(
            !footer_block.contains("kyomi.dev"),
            "the footer support link must not use the kyomi.dev domain — Kyomi's \
             support address is on kyomi.ai (see config.rs)"
        );
    }
}
