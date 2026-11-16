// SPDX-License-Identifier: AGPL-3.0-or-later

//! AI settings page — self-hosted AI configuration reference, plus the one
//! in-app control: LLM tuning (reasoning effort and temperature, KYO-972).
//!
//! Self-hosted admins only. AI provider and model configuration on
//! self-hosted deployments is environment-driven (set on the server, not in
//! the app) — this page shows what actually resolved (via
//! `get_resolved_ai_provider`) plus the relevant environment variables and
//! their precedence. Reasoning effort and temperature are the exception:
//! they are tuned per workspace here (`settings.custom_settings`) and
//! override the instance `LLM_*` defaults. In SaaS mode Kyomi manages AI for the
//! workspace and there is no user-facing configuration; navigating here
//! redirects to profile. On self-hosted, only workspace admins/owners
//! (`Permission::ManageAiConfig` — the same permission the workspace AI
//! server functions in `server_fns/ai.rs` require) see the panel; other
//! self-hosted users are redirected to profile too, since the tab is also
//! hidden from them in `SettingsShell::visible_tabs`.

use leptos::prelude::*;
use leptos_router::components::Redirect;

use kyomi_types::Permission;

use crate::components::{
    ActionStatus, Alert, AlertDescription, AlertTitle, AlertVariant, Button, Card, CardContent,
    Label, Skeleton, StatusBadge, StatusBadgeVariant, INPUT_CLASS,
};
use crate::server_fns::ai::{
    get_llm_tuning, get_resolved_ai_provider, list_openrouter_models, update_llm_tuning,
    LlmTuningValues, LlmTuningView, ResolvedAiProviderView,
};
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
// Self-hosted admin view — environment reference plus the LLM tuning card.
// ─────────────────────────────────────────────────────────────────────────────

#[component]
fn SelfHostedView() -> impl IntoView {
    view! {
        <div class="space-y-6">
            <ResolvedProviderPanel/>
            <LlmTuningCard/>
            <Card>
                <CardContent>
                    <div class="space-y-4 p-2">
                        <div>
                            <h2 class="text-xl font-display text-foreground">"Configuration"</h2>
                            <p class="text-sm text-muted-foreground">
                                "AI provider and model are set on the server via environment variables \u{2014} there is no in-app control to change them. Reasoning effort and temperature can be tuned above."
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
        </div>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Resolved provider panel — what is actually in effect, not just var names.
// ─────────────────────────────────────────────────────────────────────────────

#[component]
fn ResolvedProviderPanel() -> impl IntoView {
    let resolved = Resource::new(|| (), |_| get_resolved_ai_provider());

    view! {
        <Card>
            <CardContent>
                <div class="space-y-4 p-2">
                    <div>
                        <h2 class="text-xl font-display text-foreground">"Resolved Provider"</h2>
                        <p class="text-sm text-muted-foreground">
                            "What Kyomi is actually using for this workspace right now \u{2014} not just where it would look."
                        </p>
                    </div>
                    <Transition fallback=move || view! {
                        <div class="rounded-md border border-border divide-y divide-border">
                            <div class="flex items-center justify-between px-4 py-3">
                                <Skeleton class="h-4 w-20"/>
                                <Skeleton class="h-4 w-28"/>
                            </div>
                            <div class="flex items-center justify-between px-4 py-3">
                                <Skeleton class="h-4 w-16"/>
                                <Skeleton class="h-4 w-40"/>
                            </div>
                            <div class="flex items-center justify-between px-4 py-3">
                                <Skeleton class="h-4 w-16"/>
                                <Skeleton class="h-5 w-24"/>
                            </div>
                        </div>
                    }>
                        {move || Suspend::new(async move {
                            match resolved.await {
                                Ok(ResolvedAiProviderView::Resolved {
                                    provider,
                                    model,
                                    base_url,
                                    has_api_key,
                                    title_model,
                                }) => {
                                    view! {
                                        <div class="rounded-md border border-border divide-y divide-border">
                                            <div class="flex items-center justify-between px-4 py-3 gap-4">
                                                <span class="text-sm text-muted-foreground">"Provider"</span>
                                                <code class="px-1 py-0.5 bg-muted rounded-md text-xs font-mono">
                                                    {provider}
                                                </code>
                                            </div>
                                            <div class="flex items-center justify-between px-4 py-3 gap-4">
                                                <span class="text-sm text-muted-foreground flex-shrink-0">"Model"</span>
                                                {match model {
                                                    Some(m) => view! {
                                                        <code class="px-1 py-0.5 bg-muted rounded-md text-xs font-mono truncate min-w-0">
                                                            {m}
                                                        </code>
                                                    }.into_any(),
                                                    None => view! {
                                                        <span class="text-sm text-muted-foreground italic">
                                                            "Provider default"
                                                        </span>
                                                    }.into_any(),
                                                }}
                                            </div>
                                            <div class="flex items-center justify-between px-4 py-3 gap-4">
                                                <span class="text-sm text-muted-foreground">"API Key"</span>
                                                {if has_api_key {
                                                    view! {
                                                        <StatusBadge variant=StatusBadgeVariant::Success>
                                                            "Configured"
                                                        </StatusBadge>
                                                    }.into_any()
                                                } else {
                                                    view! {
                                                        <StatusBadge variant=StatusBadgeVariant::Warning>
                                                            "Not set"
                                                        </StatusBadge>
                                                    }.into_any()
                                                }}
                                            </div>
                                            {base_url.map(|url| view! {
                                                <div class="flex items-center justify-between px-4 py-3 gap-4">
                                                    <span class="text-sm text-muted-foreground flex-shrink-0">"Base URL"</span>
                                                    <code class="px-1 py-0.5 bg-muted rounded-md text-xs font-mono truncate min-w-0">
                                                        {url}
                                                    </code>
                                                </div>
                                            })}
                                            {title_model.map(|tm| view! {
                                                <div class="flex items-center justify-between px-4 py-3 gap-4">
                                                    <span class="text-sm text-muted-foreground flex-shrink-0">"Title Model"</span>
                                                    <code class="px-1 py-0.5 bg-muted rounded-md text-xs font-mono truncate min-w-0">
                                                        {tm}
                                                    </code>
                                                </div>
                                            })}
                                        </div>
                                    }.into_any()
                                },
                                Ok(ResolvedAiProviderView::Unconfigured { reason }) => {
                                    view! {
                                        <Alert variant=AlertVariant::Warning>
                                            <AlertTitle>"AI is not configured"</AlertTitle>
                                            <AlertDescription>{reason}</AlertDescription>
                                        </Alert>
                                    }.into_any()
                                },
                                Err(e) => {
                                    view! {
                                        <Alert variant=AlertVariant::Error>
                                            <AlertTitle>"Couldn't determine what's configured"</AlertTitle>
                                            <AlertDescription>{e.to_string()}</AlertDescription>
                                        </Alert>
                                    }.into_any()
                                },
                            }
                        })}
                    </Transition>
                </div>
            </CardContent>
        </Card>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// LLM tuning — reasoning effort and temperature (KYO-972)
// ─────────────────────────────────────────────────────────────────────────────

/// Reasoning-effort suggestions for a provider. Suggestions only: the field is
/// free text and the value is passed to the provider unchanged, because the
/// vocabulary differs per provider and model and keeps changing.
fn provider_effort_suggestions(provider: &str) -> Vec<String> {
    let list: &[&str] = match provider {
        "anthropic" => &["low", "medium", "high", "xhigh", "max"],
        "gemini" => &["minimal", "low", "medium", "high", "1024"],
        _ => &["none", "minimal", "low", "medium", "high", "xhigh", "max"],
    };
    list.iter().map(|s| (*s).to_string()).collect()
}

const TEMPERATURE_SUGGESTIONS: &[&str] = &["omit", "0", "0.1", "0.3", "0.7", "1"];

#[component]
fn TuningField(
    label: &'static str,
    hint: &'static str,
    value: RwSignal<String>,
    placeholder: String,
    list_id: &'static str,
) -> impl IntoView {
    view! {
        <div class="space-y-2">
            <Label>{label}</Label>
            <input
                type="text"
                class=INPUT_CLASS
                list=list_id
                placeholder=placeholder
                autocomplete="off"
                prop:value=move || value.get()
                on:input=move |ev| value.set(event_target_value(&ev))
            />
            <p class="text-xs text-muted-foreground">{hint}</p>
        </div>
    }
}

#[component]
fn LlmTuningCard() -> impl IntoView {
    let tuning = Resource::new(|| (), |_| get_llm_tuning());
    // Model-specific effort suggestions from OpenRouter, when that is the
    // backend. Errors and empty lists degrade to the static suggestions.
    let models = Resource::new(|| (), |_| list_openrouter_models(false));
    let resolved = Resource::new(|| (), |_| get_resolved_ai_provider());

    view! {
        <Card>
            <CardContent>
                <div class="space-y-4 p-2">
                    <div>
                        <h2 class="text-xl font-display text-foreground">"LLM Tuning"</h2>
                        <p class="text-sm text-muted-foreground">
                            "Reasoning effort and temperature for this workspace. Leave a field empty to use the instance default (shown as the placeholder); type "
                            <code class="px-1 py-0.5 bg-muted rounded-md text-xs font-mono">"omit"</code>
                            " to send no value, which some models require."
                        </p>
                    </div>
                    <Transition fallback=move || view! { <Skeleton class="h-40 w-full"/> }>
                        {move || Suspend::new(async move {
                            match tuning.await {
                                Ok(view_data) => {
                                    let model_id = match resolved.await {
                                        Ok(ResolvedAiProviderView::Resolved { model, .. }) => model,
                                        _ => None,
                                    };
                                    let model_efforts = models
                                        .await
                                        .ok()
                                        .and_then(|list| {
                                            list.into_iter().find(|m| Some(&m.id) == model_id.as_ref())
                                        })
                                        .map(|m| (m.supported_efforts, m.default_effort));
                                    view! { <LlmTuningForm data=view_data model_efforts/> }.into_any()
                                }
                                Err(e) => view! {
                                    <Alert variant=AlertVariant::Error>
                                        <AlertTitle>"Couldn't load LLM tuning"</AlertTitle>
                                        <AlertDescription>{e.to_string()}</AlertDescription>
                                    </Alert>
                                }.into_any(),
                            }
                        })}
                    </Transition>
                </div>
            </CardContent>
        </Card>
    }
}

#[component]
fn LlmTuningForm(
    data: LlmTuningView,
    model_efforts: Option<(Vec<String>, Option<String>)>,
) -> impl IntoView {
    let LlmTuningView { provider, workspace, instance, warnings } = data;

    let (mut efforts, default_effort) = match model_efforts {
        Some((supported, default)) if !supported.is_empty() => (supported, default),
        _ => (provider_effort_suggestions(&provider), None),
    };
    efforts.push("omit".to_string());

    let reasoning = RwSignal::new(workspace.reasoning_effort.clone());
    let title_reasoning = RwSignal::new(workspace.title_reasoning_effort.clone());
    let watch_reasoning = RwSignal::new(workspace.watch_reasoning_effort.clone());
    let temperature = RwSignal::new(workspace.temperature.clone());
    let title_temperature = RwSignal::new(workspace.title_temperature.clone());
    let watch_temperature = RwSignal::new(workspace.watch_temperature.clone());

    let placeholder = |instance_value: &str, fallback: &str| {
        if instance_value.is_empty() {
            fallback.to_string()
        } else {
            format!("{instance_value} (instance)")
        }
    };
    let effort_default = match &default_effort {
        Some(d) => format!("model default: {d}"),
        None => "provider default".to_string(),
    };
    let ph_reasoning = placeholder(&instance.reasoning_effort, &effort_default);
    let ph_title_reasoning = placeholder(&instance.title_reasoning_effort, "low if a title model and chat effort are set, else none");
    let ph_watch_reasoning = placeholder(&instance.watch_reasoning_effort, "same as chat");
    let ph_temperature = placeholder(&instance.temperature, "0.7 (chat), 0.1 (copilot)");
    let ph_title_temperature = placeholder(&instance.title_temperature, "0.3");
    let ph_watch_temperature = placeholder(&instance.watch_temperature, "same as chat");

    let save = Action::new(|values: &LlmTuningValues| {
        let values = values.clone();
        async move { update_llm_tuning(values).await }
    });
    let on_save = move |_| {
        save.dispatch(LlmTuningValues {
            reasoning_effort: reasoning.get_untracked(),
            title_reasoning_effort: title_reasoning.get_untracked(),
            watch_reasoning_effort: watch_reasoning.get_untracked(),
            temperature: temperature.get_untracked(),
            title_temperature: title_temperature.get_untracked(),
            watch_temperature: watch_temperature.get_untracked(),
        });
    };

    let save_error = move || {
        save.value().get().and_then(|r| r.err()).map(|e| e.to_string())
    };

    view! {
        {(!warnings.is_empty()).then(|| view! {
            <Alert variant=AlertVariant::Warning>
                <AlertTitle>"Some saved values are invalid and are being ignored"</AlertTitle>
                <AlertDescription>
                    <ul class="list-disc pl-5">
                        {warnings.iter().map(|w| view! { <li class="font-mono text-xs">{w.clone()}</li> }).collect_view()}
                    </ul>
                    <p class="mt-2">"Fix or clear the fields below and save."</p>
                </AlertDescription>
            </Alert>
        })}
        <datalist id="llm-effort-suggestions">
            {efforts.into_iter().map(|v| view! { <option value=v></option> }).collect_view()}
        </datalist>
        <datalist id="llm-temperature-suggestions">
            {TEMPERATURE_SUGGESTIONS.iter().map(|v| view! { <option value=*v></option> }).collect_view()}
        </datalist>
        <div class="grid gap-4 sm:grid-cols-2">
            <TuningField label="Reasoning effort \u{2014} chat"
                hint="Passed to the provider unchanged."
                value=reasoning placeholder=ph_reasoning list_id="llm-effort-suggestions"/>
            <TuningField label="Temperature \u{2014} chat"
                hint="A number from 0 to 2, or omit."
                value=temperature placeholder=ph_temperature list_id="llm-temperature-suggestions"/>
            <TuningField label="Reasoning effort \u{2014} watches"
                hint="Scheduled watch runs."
                value=watch_reasoning placeholder=ph_watch_reasoning list_id="llm-effort-suggestions"/>
            <TuningField label="Temperature \u{2014} watches"
                hint="Scheduled watch runs."
                value=watch_temperature placeholder=ph_watch_temperature list_id="llm-temperature-suggestions"/>
            <TuningField label="Reasoning effort \u{2014} titles and summaries"
                hint="Cheap background calls."
                value=title_reasoning placeholder=ph_title_reasoning list_id="llm-effort-suggestions"/>
            <TuningField label="Temperature \u{2014} titles and summaries"
                hint="Cheap background calls."
                value=title_temperature placeholder=ph_title_temperature list_id="llm-temperature-suggestions"/>
        </div>
        {move || save_error().map(|msg| view! {
            <Alert variant=AlertVariant::Error>
                <AlertTitle>"Not saved"</AlertTitle>
                <AlertDescription>{msg}</AlertDescription>
            </Alert>
        })}
        <div class="flex items-center gap-3">
            <Button on:click=on_save>"Save"</Button>
            <ActionStatus action=save/>
        </div>
    }
}
