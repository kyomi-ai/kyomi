// SPDX-License-Identifier: AGPL-3.0-or-later

//! AI settings page — read-only self-hosted AI configuration reference.
//!
//! Self-hosted admins only. AI provider and model configuration on
//! self-hosted deployments is environment-driven (set on the server, not in
//! the app) — this page names the relevant environment variables and their
//! precedence but exposes no controls, since there is nothing here for an
//! in-app control to do. In SaaS mode Kyomi manages AI for the workspace and
//! there is no user-facing configuration; navigating here redirects to
//! profile. On self-hosted, only workspace admins/owners
//! (`Permission::ManageAiConfig` — the same permission the workspace AI
//! server functions in `server_fns/ai.rs` require) see the panel; other
//! self-hosted users are redirected to profile too, since the tab is also
//! hidden from them in `SettingsShell::visible_tabs`.

use leptos::prelude::*;
use leptos_router::components::Redirect;

use kyomi_types::Permission;

use crate::components::{Alert, AlertDescription, AlertTitle, AlertVariant, Card, CardContent, Skeleton};
use crate::server_fns::context::UserContext;

// ─────────────────────────────────────────────────────────────────────────────
// Page entry point
// ─────────────────────────────────────────────────────────────────────────────

#[component]
pub fn AiPage() -> impl IntoView {
    let user_ctx = expect_context::<LocalResource<Result<UserContext, ServerFnError>>>();

    view! {
        <div class="p-4 sm:p-6 space-y-6">
            // Page header — Instrument Serif, matches DESIGN.md typography.
            // h2 (not h1) because SettingsShell already emits the landmark h1.
            <header class="space-y-1">
                <h2 class="text-3xl font-display text-foreground">"AI"</h2>
            </header>

            <Transition fallback=move || view! {
                <div class="space-y-4">
                    <Skeleton class="h-16 w-full"/>
                    <Skeleton class="h-40 w-full"/>
                </div>
            }>
                {move || Suspend::new(async move {
                    let ctx_result = user_ctx.await;
                    let is_self_hosted = ctx_result
                        .as_ref()
                        .map(|c| c.is_self_hosted)
                        .unwrap_or(false);
                    let can_manage_ai_config = ctx_result
                        .as_ref()
                        .map(|c| c.can(Permission::ManageAiConfig))
                        .unwrap_or(false);

                    if !is_self_hosted {
                        // SaaS: Kyomi manages AI for the workspace, no user-facing config.
                        view! { <Redirect path="/settings/profile"/> }.into_any()
                    } else if !can_manage_ai_config {
                        // Self-hosted, but not a workspace admin/owner — the tab is
                        // already hidden for this user in `visible_tabs`; this is the
                        // direct-URL guard for the same rule.
                        view! { <Redirect path="/settings/profile"/> }.into_any()
                    } else {
                        view! { <SelfHostedView/> }.into_any()
                    }
                })}
            </Transition>
        </div>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Self-hosted admin view — informational only, no controls.
// ─────────────────────────────────────────────────────────────────────────────

#[component]
fn SelfHostedView() -> impl IntoView {
    view! {
        <Card>
            <CardContent>
                <div class="space-y-4 p-2">
                    <div>
                        <h2 class="text-xl font-display text-foreground">"Configuration"</h2>
                        <p class="text-sm text-muted-foreground">
                            "AI provider and model are set on the server via environment variables \u{2014} there is no in-app control to change them."
                        </p>
                    </div>
                    <Alert variant=AlertVariant::Info>
                        <AlertTitle>"Environment variables"</AlertTitle>
                        <AlertDescription>
                            <p>
                                "Set "
                                <code class="px-1 py-0.5 bg-muted rounded-md text-xs font-mono">"LLM_PROVIDER"</code>
                                " and "
                                <code class="px-1 py-0.5 bg-muted rounded-md text-xs font-mono">"LLM_API_KEY"</code>
                                " together to choose a provider explicitly. Without them, "
                                <code class="px-1 py-0.5 bg-muted rounded-md text-xs font-mono">"ANTHROPIC_API_KEY"</code>
                                " alone defaults the provider to Anthropic."
                            </p>
                            <p class="mt-2">
                                <code class="px-1 py-0.5 bg-muted rounded-md text-xs font-mono">"LLM_MODEL"</code>
                                " and "
                                <code class="px-1 py-0.5 bg-muted rounded-md text-xs font-mono">"LLM_BASE_URL"</code>
                                " override the provider's default model and API base URL when set. "
                                <code class="px-1 py-0.5 bg-muted rounded-md text-xs font-mono">"LLM_TITLE_MODEL"</code>
                                " optionally selects a separate, typically cheaper, model for lightweight background tasks \u{2014} chat session titles and dashboard/knowledge summaries."
                            </p>
                            <p class="mt-2">
                                "Ask your Kyomi admin to change these on the server and restart it \u{2014} this page cannot modify them."
                            </p>
                        </AlertDescription>
                    </Alert>
                    <p class="text-xs text-muted-foreground pt-2">
                        "Powers: Chat · Watch · Dashboard Copilot · Chart Builder"
                    </p>
                </div>
            </CardContent>
        </Card>
    }
}
