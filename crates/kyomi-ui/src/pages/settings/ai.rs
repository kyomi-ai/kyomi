// SPDX-License-Identifier: AGPL-3.0-or-later

//! AI settings page — workspace-level AI configuration.
//!
//! Self-hosted only. In SaaS mode Kyomi manages AI for the workspace and there
//! is no user-facing configuration; navigating here redirects to profile.

use leptos::prelude::*;
use leptos_router::components::Redirect;
use crate::components::{Card, CardContent, Skeleton};
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

                    if is_self_hosted {
                        view! { <SelfHostedView/> }.into_any()
                    } else {
                        view! { <Redirect path="/settings/profile"/> }.into_any()
                    }
                })}
            </Transition>
        </div>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Self-hosted view — Kyomi-mode model selector only. No BYOK, no mode toggle.
// ─────────────────────────────────────────────────────────────────────────────

#[component]
fn SelfHostedView() -> impl IntoView {
    view! {
        <Card>
            <CardContent>
                <div class="space-y-4 p-2">
                    <div>
                        <h2 class="text-xl font-display text-foreground">"Model"</h2>
                        <p class="text-sm text-muted-foreground">
                            "Kyomi provides the LLM infrastructure. Your admin picks the model; all workspace members use it."
                        </p>
                    </div>
                    <KyomiModelPanel/>
                    <p class="text-xs text-muted-foreground pt-2">
                        "Powers: Chat · Watch · Dashboard Copilot · Chart Builder"
                    </p>
                </div>
            </CardContent>
        </Card>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Kyomi credits panel — dynamic OpenRouter model dropdown
// ─────────────────────────────────────────────────────────────────────────────

#[component]
fn KyomiModelPanel() -> impl IntoView {
    view! {
        <p class="text-sm text-foreground">
            "Kyomi automatically selects the best model for each task. "
            "No configuration needed \u{2014} just chat."
        </p>
    }
}
