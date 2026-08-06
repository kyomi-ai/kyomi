// SPDX-License-Identifier: AGPL-3.0-or-later

//! Workspace settings page — admin-only workspace configuration.
//!
//! Replaces `apps/frontend/src/components/settings/WorkspaceSettings.jsx`.
//! All data fetching uses server functions instead of REST API calls.

use leptos::prelude::*;

use kyomi_types::Permission;

use crate::components::{
    ActionStatus, Card, CardContent, CardDescription, CardHeader, CardTitle, Skeleton,
    INPUT_CLASS,
};
use crate::components::{Alert, AlertDescription, AlertTitle, AlertVariant, Button, ButtonVariant};
use crate::server_fns::context::UserContext;
use crate::server_fns::workspace::*;
use crate::types::WorkspaceSettingsData;

// ─────────────────────────────────────────────────────────────────────────────
// Access guard
//
// `get_workspace_settings` is gated server-side on
// `ac.require(Permission::ManageWorkspaceSettings, ...)` (server_fns/workspace.rs),
// so a member without the permission already gets a rejected fetch today.
// But `get_workspace_slack_status` is NOT gated — any authenticated workspace
// member can call it and receive Slack `installed`/`team_id`/`team_name`. The
// tab that links here is hidden from members without
// `ManageWorkspaceSettings` OR in personal mode (settings_shell.rs:79-81),
// but nothing stopped a direct navigation to `/settings/workspace` from
// reaching this page and its ungated resource. This guard mirrors the tab's
// full condition set so a page reached directly is denied on exactly the
// same terms as the tab that's supposed to be the only way in.
// ─────────────────────────────────────────────────────────────────────────────

/// Denial message for a member who lacks `ManageWorkspaceSettings` — used
/// both for that specific rejection and, since it's the more conservative of
/// the two possible reasons, for the fail-closed fetch-error branch below
/// where the actual reason (missing permission vs. personal mode) can't be
/// determined.
const MISSING_PERMISSION_MSG: &str =
    "You must be a workspace administrator to manage workspace settings.";

/// Returns the denial message for `ctx`, or `None` if workspace settings
/// access is allowed. Mirrors the tab visibility condition at
/// `settings_shell.rs:79-81` exactly: `can_manage_workspace_settings &&
/// !ctx.is_personal_mode`. A pure function so the two failure branches are
/// unit-testable without standing up the reactive component tree.
fn workspace_access_denial_message(ctx: &UserContext) -> Option<&'static str> {
    if ctx.is_personal_mode {
        return Some("Workspace settings aren't available in personal mode.");
    }
    if !ctx.can(Permission::ManageWorkspaceSettings) {
        return Some(MISSING_PERMISSION_MSG);
    }
    None
}

/// Full access decision for the awaited `UserContext` fetch, covering both
/// the resolved-context case and the fetch-failure case in one place so the
/// view has a single match arm to render. `Ok` delegates to
/// [`workspace_access_denial_message`]; `Err` fails CLOSED to
/// `MISSING_PERMISSION_MSG` — see the comment on [`WorkspacePage`] for why
/// this deliberately diverges from `TeamPage`'s fail-open behaviour.
fn workspace_access_decision(ctx_result: &Result<UserContext, ServerFnError>) -> Option<&'static str> {
    match ctx_result {
        Ok(ctx) => workspace_access_denial_message(ctx),
        Err(_) => Some(MISSING_PERMISSION_MSG),
    }
}

/// Shared "Access Denied" alert markup for both the confirmed-denial branch
/// and the fail-closed fetch-error branch.
fn access_denied_view(msg: &'static str) -> impl IntoView {
    view! {
        <div class="p-4 sm:p-6">
            <Alert variant=AlertVariant::Error>
                <AlertTitle>"Access Denied"</AlertTitle>
                <AlertDescription>{msg}</AlertDescription>
            </Alert>
        </div>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Main page — guard, then delegate to WorkspacePageInner
// ─────────────────────────────────────────────────────────────────────────────

#[component]
pub fn WorkspacePage() -> impl IntoView {
    // Use the UserContext resource provided by SettingsShell — already resolved, no extra fetch.
    let user_ctx = expect_context::<LocalResource<Result<UserContext, ServerFnError>>>();

    view! {
        <Transition>
            {move || Suspend::new(async move {
                let ctx_result = user_ctx.await;
                // `Err(_)` here deliberately fails CLOSED, unlike `TeamPage`
                // (team.rs), which fails open on a failed context fetch and
                // relies on its server fns to reject unauthorized calls.
                // That's not safe here: `get_workspace_slack_status`
                // (server_fns/workspace.rs) has no permission check at all,
                // so failing open would let a transient `UserContext` fetch
                // failure leak Slack `team_id`/`team_name` to any
                // authenticated member. This also matches the fail-closed
                // convention KYO-240 established for permission derivation
                // elsewhere in this crate. Net effect: an admin who hits
                // this branch (a genuine `UserContext` fetch failure, not
                // normal operation) now sees "Access Denied" instead of a
                // partly-loaded page — normal operation for admins is
                // unchanged.
                match workspace_access_decision(&ctx_result) {
                    Some(msg) => access_denied_view(msg).into_any(),
                    None => view! { <WorkspacePageInner /> }.into_any(),
                }
            })}
        </Transition>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Inner page — only rendered when access is confirmed
// ─────────────────────────────────────────────────────────────────────────────

#[component]
fn WorkspacePageInner() -> impl IntoView {
    let settings = Resource::new(|| (), |_| get_workspace_settings());

    view! {
        <div class="p-4 sm:p-6">
            <h2 class="text-xl font-display text-foreground mb-4">"Workspace Settings"</h2>
            <p class="text-muted-foreground mb-6">
                "Configure workspace-wide preferences (admin only)."
            </p>

            <Transition fallback=move || view! {
                <div class="space-y-6">
                    <Card>
                        <CardHeader>
                            <Skeleton class="h-5 w-1/3"/>
                            <Skeleton class="h-4 w-2/3 mt-1"/>
                        </CardHeader>
                        <CardContent>
                            <Skeleton class="h-10 w-full"/>
                        </CardContent>
                    </Card>
                    <Card>
                        <CardHeader>
                            <Skeleton class="h-5 w-1/3"/>
                            <Skeleton class="h-4 w-2/3 mt-1"/>
                        </CardHeader>
                        <CardContent>
                            <Skeleton class="h-20 w-full"/>
                        </CardContent>
                    </Card>
                </div>
            }>
                {move || Suspend::new(async move {
                    match settings.await {
                        Ok(data) => {
                            view! {
                                <div class="space-y-6">
                                    <WorkspaceNameCard data=data/>
                                    <WorkspaceSlackSection/>
                                </div>
                            }.into_any()
                        },
                        Err(e) => {
                            let msg = e.to_string();
                            view! {
                                <Card>
                                    <div class="p-6">
                                        <p class="text-error-foreground">"Failed to load workspace settings: " {msg}</p>
                                    </div>
                                </Card>
                            }.into_any()
                        },
                    }
                })}
            </Transition>
        </div>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Workspace Name Card
// ─────────────────────────────────────────────────────────────────────────────

#[component]
fn WorkspaceNameCard(data: WorkspaceSettingsData) -> impl IntoView {
    let (name, set_name) = signal(data.workspace_name.clone());
    let save_action = Action::new(|name: &String| {
        let name = name.clone();
        async move { update_workspace_name(name).await }
    });

    let on_blur = move |_| {
        let current = name.get();
        if !current.trim().is_empty() {
            save_action.dispatch(current);
        }
    };

    view! {
        <Card>
            <CardHeader>
                <div class="flex items-center justify-between">
                    <div>
                        <CardTitle>"Workspace Name"</CardTitle>
                        <CardDescription>
                            "Give your workspace a meaningful name to help identify it."
                        </CardDescription>
                    </div>
                    <ActionStatus action=save_action/>
                </div>
            </CardHeader>
            <CardContent>
                <input
                    type="text"
                    class=INPUT_CLASS
                    placeholder="My Workspace"
                    prop:value=name
                    on:input=move |ev| set_name.set(event_target_value(&ev))
                    on:blur=on_blur
                />
            </CardContent>
        </Card>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Slack section — feature-gated wrapper to avoid cfg inside view! macro
// ─────────────────────────────────────────────────────────────────────────────

#[component]
fn WorkspaceSlackSection() -> impl IntoView {
    view! { <WorkspaceSlackCard/> }
}

#[component]
fn WorkspaceSlackCard() -> impl IntoView {
    let slack_status = Resource::new(|| (), |_| get_workspace_slack_status());
    let (slack_error, set_slack_error) = signal::<Option<String>>(None);
    let (slack_success, set_slack_success) = signal::<Option<String>>(None);
    let (uninstalling, set_uninstalling) = signal(false);

    // Check URL params for OAuth callback result
    #[cfg(target_arch = "wasm32")]
    {
        if let Some(window) = web_sys::window()
            && let Ok(search) = window.location().search() {
                if search.contains("slack=installed") {
                    set_slack_success.set(Some("Kyomi has been added to your Slack workspace!".to_string()));
                    // Clear URL param
                    let _ = window.history().and_then(|h| {
                        let pathname = window.location().pathname().unwrap_or_default();
                        h.replace_state_with_url(&wasm_bindgen::JsValue::NULL, "", Some(&pathname))
                    });
                } else if search.contains("slack=error") {
                    set_slack_error.set(Some("Failed to install Slack integration. Please try again.".to_string()));
                    let _ = window.history().and_then(|h| {
                        let pathname = window.location().pathname().unwrap_or_default();
                        h.replace_state_with_url(&wasm_bindgen::JsValue::NULL, "", Some(&pathname))
                    });
                }
            }
    }

    let handle_install = move |_| {
        set_slack_error.set(None);
        leptos::task::spawn_local(async move {
            match get_slack_install_url().await {
                Ok(url) => {
                    #[cfg(target_arch = "wasm32")]
                    {
                        if let Some(window) = web_sys::window() {
                            let _ = window.location().set_href(&url);
                        }
                    }
                    #[cfg(not(target_arch = "wasm32"))]
                    {
                        let _ = url;
                    }
                }
                Err(e) => {
                    set_slack_error.try_set(Some(e.to_string()));
                }
            }
        });
    };

    let handle_uninstall = {
        move |_| {
            // Get the team_id from current status for the uninstall call
            let team_id = slack_status
                .get()
                .and_then(|r| r.ok())
                .and_then(|s| s.team_id.clone())
                .unwrap_or_default();

            set_uninstalling.set(true);
            set_slack_error.set(None);
            set_slack_success.set(None);

            leptos::task::spawn_local(async move {
                match uninstall_workspace_slack(team_id).await {
                    Ok(()) => {
                        set_slack_success.try_set(Some("Slack integration removed successfully.".to_string()));
                        slack_status.refetch();
                    }
                    Err(e) => {
                        set_slack_error.try_set(Some(e.to_string()));
                    }
                }
                set_uninstalling.try_set(false);
            });
        }
    };

    view! {
        <Card>
            <CardHeader>
                <div class="flex items-center justify-between">
                    <div>
                        <CardTitle>"Slack Integration"</CardTitle>
                        <CardDescription>
                            "Enable watch alerts to post to Slack channels as Kyomi."
                        </CardDescription>
                    </div>
                </div>
            </CardHeader>
            <CardContent>
                // Success alert
                {move || slack_success.get().map(|msg| view! {
                    <Alert variant=AlertVariant::Success attr:class="mb-4">
                        <AlertDescription>{msg}</AlertDescription>
                    </Alert>
                })}

                // Error alert
                {move || slack_error.get().map(|msg| view! {
                    <Alert variant=AlertVariant::Error attr:class="mb-4">
                        <AlertDescription>{msg}</AlertDescription>
                    </Alert>
                })}

                <Transition fallback=move || view! {
                    <div class="flex items-center gap-2 text-muted-foreground">
                        <span class="h-4 w-4 animate-spin rounded-full border-2 border-current border-t-transparent"></span>
                        <span>"Loading Slack status..."</span>
                    </div>
                }>
                    {move || Suspend::new(async move {
                        match slack_status.await {
                            Ok(status) if status.installed => {
                                let team_name = status.team_name.clone().unwrap_or_default();
                                view! {
                                    <div class="space-y-4">
                                        <div class="flex items-center gap-2">
                                            <div class="h-2 w-2 rounded-full bg-success"></div>
                                            <span class="text-sm text-foreground">
                                                "Connected to "
                                                <strong>{team_name}</strong>
                                            </span>
                                        </div>
                                        <Button
                                            variant=ButtonVariant::Outline
                                            on:click=handle_uninstall
                                            disabled=uninstalling.get()
                                        >
                                            {move || {
                                                if uninstalling.get() {
                                                    view! {
                                                        <span class="inline-flex items-center gap-2">
                                                            <span class="h-4 w-4 animate-spin rounded-full border-2 border-current border-t-transparent"></span>
                                                            "Removing..."
                                                        </span>
                                                    }.into_any()
                                                } else {
                                                    view! {
                                                        <span class="inline-flex items-center gap-2">
                                                            <span class="inline-flex">
                                                                <phosphor_leptos::Icon icon=phosphor_leptos::PLUGS size="16px"/>
                                                            </span>
                                                            "Remove Integration"
                                                        </span>
                                                    }.into_any()
                                                }
                                            }}
                                        </Button>
                                    </div>
                                }.into_any()
                            },
                            Ok(_) => {
                                // Not installed
                                view! {
                                    <div class="space-y-4">
                                        <p class="text-sm text-muted-foreground">
                                            "Connect Kyomi to your Slack workspace to receive watch alerts in channels."
                                        </p>
                                        <Button on:click=handle_install>
                                            <span class="inline-flex items-center gap-2">
                                                <span class="inline-flex">
                                                    <phosphor_leptos::Icon icon=phosphor_leptos::ARROW_SQUARE_OUT size="16px"/>
                                                </span>
                                                "Add Kyomi to Slack"
                                            </span>
                                        </Button>
                                    </div>
                                }.into_any()
                            },
                            Err(e) => {
                                let msg = e.to_string();
                                // If the error is about tier, show upgrade prompt
                                if msg.contains("Kyomi Cloud subscription") {
                                    view! {
                                        <Alert variant=AlertVariant::Info>
                                            <AlertDescription>
                                                "Slack integration requires an active Kyomi Cloud subscription. "
                                                <a href="/settings/billing" class="text-primary font-medium hover:underline">
                                                    "Upgrade to enable"
                                                </a>
                                            </AlertDescription>
                                        </Alert>
                                    }.into_any()
                                } else {
                                    view! {
                                        <Alert variant=AlertVariant::Error>
                                            <AlertDescription>{msg}</AlertDescription>
                                        </Alert>
                                    }.into_any()
                                }
                            },
                        }
                    })}
                </Transition>
            </CardContent>
        </Card>
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    /// Minimal `UserContext` fixture — `is_personal_mode` and `permissions`
    /// vary between the cases below.
    fn user_ctx_fixture(is_personal_mode: bool, permissions: Vec<Permission>) -> UserContext {
        UserContext {
            user_id: "user-1".to_string(),
            email: "user@example.com".to_string(),
            name: None,
            workspace_id: Some("ws-1".to_string()),
            workspace_name: Some("Test Workspace".to_string()),
            is_owner: false,
            subscription_tier: "team".to_string(),
            subscription_status: "active".to_string(),
            is_personal_mode,
            is_self_hosted: false,
            billing_enabled: false,
            capabilities: HashMap::new(),
            chart_palette: "balanced".to_string(),
            permissions,
        }
    }

    // ── workspace_access_denial_message ─────────────────────────────────────

    #[test]
    fn denies_when_permission_is_absent() {
        let ctx = user_ctx_fixture(false, Vec::new());

        let denial = workspace_access_denial_message(&ctx);

        assert_eq!(denial, Some(MISSING_PERMISSION_MSG));
    }

    #[test]
    fn denies_in_personal_mode_even_with_the_permission() {
        // The subset trap KYO-260 fell into: gating on the permission alone
        // without also checking `is_personal_mode` — settings_shell.rs:80
        // requires both `can_manage_workspace_settings && !is_personal_mode`.
        let ctx = user_ctx_fixture(true, vec![Permission::ManageWorkspaceSettings]);

        let denial = workspace_access_denial_message(&ctx);

        assert_eq!(
            denial,
            Some("Workspace settings aren't available in personal mode.")
        );
    }

    #[test]
    fn allows_when_permission_is_present_and_not_personal_mode() {
        let ctx = user_ctx_fixture(false, vec![Permission::ManageWorkspaceSettings]);

        let denial = workspace_access_denial_message(&ctx);

        assert_eq!(denial, None);
    }

    #[test]
    fn denies_when_neither_permission_nor_personal_mode_condition_is_met() {
        // Personal mode with no permission either — still denied, and on the
        // personal-mode message since that check runs first (matching
        // settings_shell.rs's short-circuit order).
        let ctx = user_ctx_fixture(true, Vec::new());

        let denial = workspace_access_denial_message(&ctx);

        assert_eq!(
            denial,
            Some("Workspace settings aren't available in personal mode.")
        );
    }

    // ── workspace_access_decision (fail-closed on fetch failure) ────────────

    #[test]
    fn denies_on_a_failed_context_fetch() {
        // The deliberate deviation from `TeamPage`: a failed `UserContext`
        // fetch must fail CLOSED here, not open, because
        // `get_workspace_slack_status` has no permission check of its own.
        let result: Result<UserContext, ServerFnError> =
            Err(ServerFnError::new("simulated network failure"));

        let denial = workspace_access_decision(&result);

        assert_eq!(denial, Some(MISSING_PERMISSION_MSG));
    }

    #[test]
    fn decision_allows_when_the_fetch_succeeds_and_access_is_granted() {
        let ctx = user_ctx_fixture(false, vec![Permission::ManageWorkspaceSettings]);
        let result: Result<UserContext, ServerFnError> = Ok(ctx);

        let denial = workspace_access_decision(&result);

        assert_eq!(denial, None);
    }

    #[test]
    fn decision_denies_when_the_fetch_succeeds_but_access_is_not_granted() {
        let ctx = user_ctx_fixture(false, Vec::new());
        let result: Result<UserContext, ServerFnError> = Ok(ctx);

        let denial = workspace_access_decision(&result);

        assert_eq!(denial, Some(MISSING_PERMISSION_MSG));
    }
}
