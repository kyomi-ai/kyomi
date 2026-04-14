// SPDX-License-Identifier: AGPL-3.0-or-later

//! AI settings page — workspace-level AI configuration.
//!
//! Workspace BYOK is a SaaS-only feature. In self-hosted mode this page shows
//! only a Kyomi-mode model selector (single card, no mode toggle, no BYOK
//! panel) so the route stays useful but doesn't surface a broken feature.
//!
//! All data flows through the Track 4 server functions:
//! - [`get_workspace_ai_config`] — read current config (any member)
//! - [`update_workspace_ai_config`] — write config (admin only)
//! - [`test_workspace_ai_config`] — dry-run BYOK credentials (admin only)
//!
//! The page deliberately uses a single [`Resource`] so the status banner,
//! mode selector, and active panel all re-render from the same source of
//! truth after every save.

use leptos::prelude::*;
use leptos_icons::Icon;

use crate::components::{
    Button, ButtonVariant, Card, CardContent, Label, Skeleton, INPUT_CLASS,
};
use crate::components::select::{CHEVRON_STYLE, SELECT_CLASS};
use crate::components::toast::{toast_error, toast_success};
use crate::pages::settings::ai_models::{
    label_for_model, provider_label, KYOMI_CREDITS_MODELS,
};
use crate::server_fns::ai::{
    get_workspace_ai_config, list_workspace_ai_models, test_workspace_ai_config,
    update_workspace_ai_config, AiModelInfo, WorkspaceAiConfigView,
};
use crate::server_fns::context::UserContext;

// ─────────────────────────────────────────────────────────────────────────────
// Constants
// ─────────────────────────────────────────────────────────────────────────────

const KYOMI_PROVIDER: &str = "kyomi";
const DEFAULT_KYOMI_MODEL: &str = "claude-sonnet-4-5-20250929";
const CUSTOM_MODEL_SENTINEL: &str = "__custom__";

const PROVIDER_OPTIONS: &[(&str, &str)] = &[
    ("anthropic", "Anthropic"),
    ("openai", "OpenAI"),
    ("gemini", "Gemini"),
];

// ─────────────────────────────────────────────────────────────────────────────
// Page entry point
// ─────────────────────────────────────────────────────────────────────────────

#[component]
pub fn AiPage() -> impl IntoView {
    let user_ctx = expect_context::<LocalResource<Result<UserContext, ServerFnError>>>();

    // Single source of truth. Bump `version` after a successful save to
    // re-fetch and re-render the banner, mode selector, and panel from the
    // same data.
    let (version, set_version) = signal(0u32);
    let config = Resource::new(
        move || version.get(),
        |_| get_workspace_ai_config(),
    );

    let refresh = Callback::new(move |_: ()| {
        set_version.update(|v| *v += 1);
    });

    view! {
        <div class="p-4 sm:p-6 space-y-6 bg-background">
            // Page header — Instrument Serif, matches DESIGN.md typography.
            <header class="space-y-1">
                <h1 class="text-3xl font-display text-foreground">"AI"</h1>
                <p class="text-sm text-muted-foreground">
                    "Choose how AI is billed for your workspace and which model to use."
                </p>
            </header>

            <Transition fallback=move || view! {
                <div class="space-y-4">
                    <Skeleton class="h-16 w-full"/>
                    <Skeleton class="h-40 w-full"/>
                </div>
            }>
                {move || Suspend::new(async move {
                    let ctx_result = user_ctx.await;
                    let cfg_result = config.await;

                    let is_self_hosted = ctx_result
                        .as_ref()
                        .map(|c| c.is_self_hosted)
                        .unwrap_or(false);
                    let is_admin = ctx_result
                        .as_ref()
                        .map(|c| c.workspace_roles.iter().any(|r| r == "workspace_admin"))
                        .unwrap_or(false);
                    let is_owner = ctx_result
                        .as_ref()
                        .map(|c| c.is_owner)
                        .unwrap_or(false);

                    match cfg_result {
                        Ok(cfg) => {
                            if is_self_hosted {
                                view! {
                                    <SelfHostedView cfg=cfg is_admin=is_admin refresh=refresh/>
                                }.into_any()
                            } else {
                                view! {
                                    <SaasView cfg=cfg is_admin=is_admin is_owner=is_owner refresh=refresh/>
                                }.into_any()
                            }
                        }
                        Err(e) => view! {
                            <Card>
                                <CardContent>
                                    <p class="text-sm text-error-foreground p-2">
                                        "Failed to load AI configuration: " {e.to_string()}
                                    </p>
                                </CardContent>
                            </Card>
                        }.into_any(),
                    }
                })}
            </Transition>

            // Feature context footer — muted, always visible.
            <p class="text-xs text-muted-foreground pt-2">
                "Powers: Chat · Watch · Dashboard Copilot · Chart Builder"
            </p>
        </div>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Self-hosted view — Kyomi-mode model selector only. No BYOK, no mode toggle.
// ─────────────────────────────────────────────────────────────────────────────

#[component]
fn SelfHostedView(
    cfg: WorkspaceAiConfigView,
    is_admin: bool,
    refresh: Callback<()>,
) -> impl IntoView {
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
                    <KyomiModelPanel cfg=cfg is_admin=is_admin refresh=refresh/>
                </div>
            </CardContent>
        </Card>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// SaaS view — status banner + mode selector + active panel
// ─────────────────────────────────────────────────────────────────────────────

#[component]
fn SaasView(
    cfg: WorkspaceAiConfigView,
    is_admin: bool,
    is_owner: bool,
    refresh: Callback<()>,
) -> impl IntoView {
    // Local signal tracks the *selected* mode in the UI. Initialised from the
    // current config, but the user can flip it (admin only) to preview the
    // other panel without saving.
    let initial_is_byok = cfg.provider != KYOMI_PROVIDER;
    let (byok_selected, set_byok_selected) = signal(initial_is_byok);

    let cfg_for_banner = cfg.clone();
    let cfg_for_kyomi = cfg.clone();
    let cfg_for_byok = cfg;

    view! {
        <div class="space-y-6">
            <StatusBanner cfg=cfg_for_banner is_owner=is_owner/>

            <ModeSelector
                byok_selected=byok_selected
                set_byok_selected=set_byok_selected
                is_admin=is_admin
            />

            {move || {
                let cfg_for_byok = cfg_for_byok.clone();
                let cfg_for_kyomi = cfg_for_kyomi.clone();
                if byok_selected.get() {
                    view! {
                        <ByokPanel
                            cfg=cfg_for_byok
                            is_admin=is_admin
                            refresh=refresh
                        />
                    }.into_any()
                } else {
                    view! {
                        <Card>
                            <CardContent>
                                <div class="p-2">
                                    <KyomiModelPanel
                                        cfg=cfg_for_kyomi
                                        is_admin=is_admin
                                        refresh=refresh
                                    />
                                </div>
                            </CardContent>
                        </Card>
                    }.into_any()
                }
            }}
        </div>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Status banner
// ─────────────────────────────────────────────────────────────────────────────

#[component]
fn StatusBanner(cfg: WorkspaceAiConfigView, is_owner: bool) -> impl IntoView {
    let is_byok = cfg.provider != KYOMI_PROVIDER;
    let model_label = cfg
        .model
        .as_deref()
        .map(|m| label_for_model(&cfg.provider, m))
        .unwrap_or_else(|| "(no model set)".to_string());

    let (container_class, icon) = if is_byok {
        (
            "flex items-start gap-3 rounded-md border border-accent/40 bg-accent-light p-4",
            icondata_lu::LuKeyRound,
        )
    } else {
        (
            "flex items-start gap-3 rounded-md border border-border bg-surface-alt p-4",
            icondata_lu::LuSparkles,
        )
    };

    // BYOK text is a single static string; Kyomi text has an optional balance
    // clause and an owner-only "Buy more" link appended inline.
    if is_byok {
        let text = format!(
            "Using your {} API key · {} · Workspace bundle not consumed",
            provider_label(&cfg.provider),
            model_label,
        );
        return view! {
            <div class=container_class>
                <Icon icon=icon width="18" height="18"/>
                <div class="flex-1 text-sm text-foreground">
                    {text}
                </div>
            </div>
        }
        .into_any();
    }

    // Kyomi mode: prefix + optional "$X.XX remaining in token bundle" clause.
    // If the balance is `None` (edge case: workspace row missing) we omit the
    // clause rather than rendering "$0.00", which would be misleading.
    let prefix = format!("Using Kyomi credits · {model_label}");
    let balance_clause = cfg
        .ai_bundle_balance_usd
        .map(|b| format!(" · ${b:.2} remaining in token bundle"));

    view! {
        <div class=container_class>
            <Icon icon=icon width="18" height="18"/>
            <div class="flex-1 text-sm text-foreground">
                {prefix}
                {balance_clause}
                {move || if is_owner {
                    Some(view! {
                        " "
                        <a
                            href="/settings/billing"
                            class="text-accent hover:underline"
                        >
                            "Buy more"
                        </a>
                    })
                } else {
                    None
                }}
            </div>
        </div>
    }
    .into_any()
}

// ─────────────────────────────────────────────────────────────────────────────
// Mode selector — two radio-style cards
// ─────────────────────────────────────────────────────────────────────────────

#[component]
fn ModeSelector(
    byok_selected: ReadSignal<bool>,
    set_byok_selected: WriteSignal<bool>,
    is_admin: bool,
) -> impl IntoView {
    view! {
        <div class="space-y-2">
            <div class="grid grid-cols-1 md:grid-cols-2 gap-3">
                <ModeCard
                    title="Kyomi credits"
                    body="Pay Kyomi per request. Use our infrastructure, no setup."
                    selected=Signal::derive(move || !byok_selected.get())
                    is_admin=is_admin
                    on_select=Callback::new(move |_| set_byok_selected.set(false))
                />
                <ModeCard
                    title="Your own API key"
                    body="Bring your own Anthropic, OpenAI, or Gemini key. Pay the provider directly."
                    selected=Signal::derive(move || byok_selected.get())
                    is_admin=is_admin
                    on_select=Callback::new(move |_| set_byok_selected.set(true))
                />
            </div>
            <Show when=move || !is_admin>
                <p class="text-xs text-muted-foreground">
                    "Only workspace admins can change AI configuration."
                </p>
            </Show>
        </div>
    }
}

#[component]
fn ModeCard(
    title: &'static str,
    body: &'static str,
    selected: Signal<bool>,
    is_admin: bool,
    on_select: Callback<()>,
) -> impl IntoView {
    let class = move || {
        let base = "relative text-left rounded-md p-4 transition-colors w-full";
        let state = if selected.get() {
            "border-2 border-accent bg-accent-light/30"
        } else {
            "border border-border hover:border-[--color-border-strong]"
        };
        let interact = if is_admin {
            "cursor-pointer"
        } else {
            "cursor-not-allowed opacity-70"
        };
        format!("{base} {state} {interact}")
    };

    view! {
        <button
            type="button"
            class=class
            disabled=!is_admin
            on:click=move |_| {
                if is_admin {
                    on_select.run(());
                }
            }
        >
            <Show when=move || selected.get()>
                <div class="absolute top-3 right-3 text-accent">
                    <Icon icon=icondata_lu::LuCheck width="18" height="18"/>
                </div>
            </Show>
            <div class="font-semibold text-foreground pr-6">{title}</div>
            <div class="text-sm text-muted-foreground mt-1">{body}</div>
        </button>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Kyomi credits panel — curated Anthropic-only model dropdown
// ─────────────────────────────────────────────────────────────────────────────

#[component]
fn KyomiModelPanel(
    cfg: WorkspaceAiConfigView,
    is_admin: bool,
    refresh: Callback<()>,
) -> impl IntoView {
    // Pick the initial model: server value, clamped to a Kyomi-supported
    // option. If the server model isn't in the curated list, fall back to
    // `DEFAULT_KYOMI_MODEL` so the select always matches an `<option>`.
    let initial_model = cfg
        .model
        .clone()
        .filter(|m| KYOMI_CREDITS_MODELS.iter().any(|opt| opt.id == m))
        .unwrap_or_else(|| DEFAULT_KYOMI_MODEL.to_string());

    let (selected_model, set_selected_model) = signal(initial_model.clone());
    let baseline = initial_model;

    let dirty = Signal::derive(move || selected_model.get() != baseline);

    let save_action = Action::new(move |model: &String| {
        let model = model.clone();
        async move {
            match update_workspace_ai_config(
                KYOMI_PROVIDER.to_string(),
                None,
                None,
                Some(model),
            )
            .await
            {
                Ok(_) => {
                    toast_success("AI configuration saved.");
                    refresh.run(());
                }
                Err(e) => toast_error(format!("Failed to save: {e}")),
            }
        }
    });

    view! {
        <div class="space-y-4">
            <div class="space-y-2">
                <Label>"Model"</Label>
                <select
                    class=SELECT_CLASS
                    style=CHEVRON_STYLE
                    disabled=!is_admin
                    prop:value=move || selected_model.get()
                    on:change=move |ev| set_selected_model.set(event_target_value(&ev))
                >
                    {KYOMI_CREDITS_MODELS.iter().map(|m| view! {
                        <option value=m.id>{m.label}</option>
                    }).collect_view()}
                </select>
                <p class="text-xs text-muted-foreground">
                    "Kyomi provides the LLM infrastructure. Your admin picks the model; all workspace members use it."
                </p>
            </div>

            <Show when=move || is_admin && dirty.get()>
                <div>
                    <Button
                        on:click=move |_| {
                            save_action.dispatch(selected_model.get_untracked());
                        }
                        disabled=Signal::derive(move || save_action.pending().get())
                    >
                        {move || if save_action.pending().get() { "Saving..." } else { "Save" }}
                    </Button>
                </div>
            </Show>
        </div>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// BYOK panel — provider / api_key / model / advanced disclosure
// ─────────────────────────────────────────────────────────────────────────────

#[component]
fn ByokPanel(
    cfg: WorkspaceAiConfigView,
    is_admin: bool,
    refresh: Callback<()>,
) -> impl IntoView {
    // If the server is still in Kyomi mode, default the BYOK form to Anthropic.
    let initial_provider = if cfg.provider == KYOMI_PROVIDER {
        "anthropic".to_string()
    } else {
        cfg.provider.clone()
    };

    let initial_model = cfg.model.clone().unwrap_or_default();
    let initial_base_url = cfg.base_url.clone().unwrap_or_default();
    let had_api_key = cfg.has_api_key;

    let (provider, set_provider) = signal(initial_provider);
    let (api_key, set_api_key) = signal(String::new());
    let (model_choice, set_model_choice) = signal(initial_model.clone());
    let (custom_model, set_custom_model) = signal(initial_model);
    let (base_url, set_base_url) = signal(initial_base_url);
    let (show_advanced, set_show_advanced) = signal(false);
    let (test_result, set_test_result) = signal::<Option<Result<String, String>>>(None);

    // Bumped after a successful test or save to trigger a model-list refetch.
    let (refetch_models_version, set_refetch_models_version) = signal(0u32);
    // The most recently *tested* (validated) candidate key. Passed to the
    // model-list resource so we can fetch the live list before the user
    // commits via Save.
    let (last_tested_key, set_last_tested_key) = signal::<Option<String>>(None);
    // Tracks whether we've already reconciled `model_choice` against the
    // initial fetched-models list (to convert unknown ids → custom mode).
    let (initial_apply_done, set_initial_apply_done) = signal(false);

    // Live model list for the current BYOK provider. Also tracks the
    // advanced-panel base_url override so users with custom proxy endpoints
    // list models from their configured URL, not the provider default.
    let models_resource = Resource::new(
        move || {
            (
                provider.get(),
                refetch_models_version.get(),
                last_tested_key.get(),
                base_url.get(),
            )
        },
        |(prov, _, candidate_key, url)| async move {
            let url_opt = {
                let trimmed = url.trim();
                if trimmed.is_empty() {
                    None
                } else {
                    Some(trimmed.to_string())
                }
            };
            list_workspace_ai_models(prov, candidate_key, url_opt).await
        },
    );

    // Reset model_choice + custom_model + last_tested_key when the provider
    // changes.
    Effect::new(move |prev_provider: Option<String>| {
        let current = provider.get();
        if let Some(prev) = prev_provider.as_ref()
            && prev != &current
        {
            set_model_choice.set(String::new());
            set_custom_model.set(String::new());
            set_test_result.set(None);
            set_last_tested_key.set(None);
            set_initial_apply_done.set(false);
        }
        current
    });

    // After the first successful fetch, if the currently-selected model isn't
    // in the returned list, flip to custom-model mode preserving the value.
    // Runs at most once per provider switch.
    Effect::new(move |_| {
        if initial_apply_done.get() {
            return;
        }
        let Some(Ok(list)) = models_resource.get() else {
            return;
        };
        let current = model_choice.get_untracked();
        if !current.is_empty()
            && current != CUSTOM_MODEL_SENTINEL
            && !list.iter().any(|m| m.id == current)
        {
            set_custom_model.set(current);
            set_model_choice.set(CUSTOM_MODEL_SENTINEL.to_string());
        }
        set_initial_apply_done.set(true);
    });

    // Resolve the actual model ID to send to the server.
    let effective_model = Signal::derive(move || {
        let choice = model_choice.get();
        if choice == CUSTOM_MODEL_SENTINEL {
            custom_model.get().trim().to_string()
        } else {
            choice.trim().to_string()
        }
    });

    let test_action = Action::new(move |_: &()| async move {
        let prov = provider.get_untracked();
        let key = api_key.get_untracked();
        if key.trim().is_empty() {
            set_test_result.set(Some(Err("Enter an API key first.".to_string())));
            return;
        }
        let url_override = base_url.get_untracked();
        let url_opt = if url_override.trim().is_empty() {
            None
        } else {
            Some(url_override.trim().to_string())
        };
        let model_opt = {
            let m = effective_model.get_untracked();
            if m.is_empty() { None } else { Some(m) }
        };

        let key_for_refetch = key.clone();
        match test_workspace_ai_config(prov, key, url_opt, model_opt).await {
            Ok(result) => {
                if result.ok {
                    set_test_result.set(Some(Ok(result.message)));
                    // Promote the just-tested key so the model list resource
                    // can fetch with it (no need to Save first).
                    set_last_tested_key.set(Some(key_for_refetch));
                    set_initial_apply_done.set(false);
                    set_refetch_models_version.update(|v| *v += 1);
                } else {
                    set_test_result.set(Some(Err(result.message)));
                }
            }
            Err(e) => set_test_result.set(Some(Err(e.to_string()))),
        }
    });

    let save_action = Action::new(move |_: &()| async move {
        let prov = provider.get_untracked();
        let key = api_key.get_untracked();
        let model = effective_model.get_untracked();
        if model.is_empty() {
            toast_error("Pick a model (or enter a custom model ID).");
            return;
        }
        // Only send the api_key when the user typed something new. Passing
        // `None` preserves the existing encrypted key on the server.
        let key_opt = if key.trim().is_empty() { None } else { Some(key) };
        if key_opt.is_none() && !had_api_key {
            toast_error("Enter an API key.");
            return;
        }
        let url_override = base_url.get_untracked();
        let url_opt = if url_override.trim().is_empty() {
            None
        } else {
            Some(url_override.trim().to_string())
        };

        match update_workspace_ai_config(prov, key_opt, url_opt, Some(model)).await {
            Ok(_) => {
                toast_success("AI configuration saved.");
                set_api_key.set(String::new());
                // The stored key is now valid — drop the last-tested fallback
                // and refetch models against the persisted credentials.
                set_last_tested_key.set(None);
                set_refetch_models_version.update(|v| *v += 1);
                refresh.run(());
            }
            Err(e) => toast_error(format!("Failed to save: {e}")),
        }
    });

    view! {
        <Card>
            <CardContent>
                <div class="space-y-5 p-2">
                    // ── Provider ─────────────────────────────────────────
                    <div class="space-y-2">
                        <Label>"Provider"</Label>
                        <select
                            class=SELECT_CLASS
                            style=CHEVRON_STYLE
                            disabled=!is_admin
                            prop:value=move || provider.get()
                            on:change=move |ev| set_provider.set(event_target_value(&ev))
                        >
                            {PROVIDER_OPTIONS.iter().map(|(id, label)| view! {
                                <option value=*id>{*label}</option>
                            }).collect_view()}
                        </select>
                    </div>

                    // ── API key ──────────────────────────────────────────
                    <div class="space-y-2">
                        <Label>"API key"</Label>
                        <div class="flex gap-2">
                            <input
                                type="password"
                                class=INPUT_CLASS
                                disabled=!is_admin
                                placeholder=move || {
                                    if had_api_key && api_key.get().is_empty() {
                                        "\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}".to_string()
                                    } else {
                                        match provider.get().as_str() {
                                            "anthropic" => "sk-ant-...".to_string(),
                                            "openai" => "sk-...".to_string(),
                                            "gemini" => "AIza...".to_string(),
                                            _ => "API key".to_string(),
                                        }
                                    }
                                }
                                prop:value=move || api_key.get()
                                on:input=move |ev| set_api_key.set(event_target_value(&ev))
                            />
                            <Button
                                variant=ButtonVariant::Outline
                                disabled=Signal::derive(move || !is_admin || test_action.pending().get())
                                on:click=move |_| {
                                    test_action.dispatch(());
                                }
                            >
                                {move || if test_action.pending().get() { "Testing..." } else { "Test" }}
                            </Button>
                            <Button
                                disabled=Signal::derive(move || !is_admin || save_action.pending().get())
                                on:click=move |_| {
                                    save_action.dispatch(());
                                }
                            >
                                {move || if save_action.pending().get() { "Saving..." } else { "Save" }}
                            </Button>
                        </div>
                        <p class="text-xs text-muted-foreground">
                            "Stored encrypted. All workspace members automatically use this key for AI requests."
                        </p>
                        {move || match test_result.get() {
                            Some(Ok(msg)) => view! {
                                <div class="flex items-center gap-2 text-sm text-success-foreground">
                                    <Icon icon=icondata_lu::LuCheck width="16" height="16"/>
                                    <span>{msg}</span>
                                </div>
                            }.into_any(),
                            Some(Err(msg)) => view! {
                                <div class="flex items-center gap-2 text-sm text-error-foreground">
                                    <Icon icon=icondata_lu::LuCircleX width="16" height="16"/>
                                    <span>{msg}</span>
                                </div>
                            }.into_any(),
                            None => view! { <span class="hidden"></span> }.into_any(),
                        }}
                    </div>

                    // ── Model ────────────────────────────────────────────
                    <div class="space-y-2">
                        <Label>"Model"</Label>
                        <Suspense fallback=move || view! {
                            <select
                                class=SELECT_CLASS
                                style=CHEVRON_STYLE
                                disabled=true
                            >
                                <option>"Loading models\u{2026}"</option>
                            </select>
                        }>
                            {move || {
                                let resource_value = models_resource.get();
                                let (models, fetch_error): (Vec<AiModelInfo>, Option<String>) =
                                    match resource_value {
                                        Some(Ok(list)) => (list, None),
                                        Some(Err(e)) => (Vec::new(), Some(e.to_string())),
                                        None => (Vec::new(), None),
                                    };

                                let current = model_choice.get();
                                // If the current selection isn't in the fetched
                                // list (and isn't custom/empty), inject a synthetic
                                // option so the dropdown isn't blank during the
                                // pre-reconciliation flicker window.
                                let needs_synthetic = !current.is_empty()
                                    && current != CUSTOM_MODEL_SENTINEL
                                    && !models.iter().any(|m| m.id == current);
                                let synthetic_id = current.clone();

                                view! {
                                    <select
                                        class=SELECT_CLASS
                                        style=CHEVRON_STYLE
                                        disabled=!is_admin
                                        prop:value=move || model_choice.get()
                                        on:change=move |ev| set_model_choice.set(event_target_value(&ev))
                                    >
                                        <option value="">"Select a model..."</option>
                                        {needs_synthetic.then(|| view! {
                                            <option value=synthetic_id.clone()>{synthetic_id.clone()}</option>
                                        })}
                                        {models.into_iter().map(|m| view! {
                                            <option value=m.id.clone()>{m.label}</option>
                                        }).collect_view()}
                                        <option value=CUSTOM_MODEL_SENTINEL>"Custom model ID\u{2026}"</option>
                                    </select>
                                    {fetch_error.map(|msg| view! {
                                        <p class="text-xs text-error-foreground">
                                            "Couldn\u{2019}t load models: " {msg}
                                        </p>
                                    })}
                                }
                            }}
                        </Suspense>
                        <Show when=move || model_choice.get() == CUSTOM_MODEL_SENTINEL>
                            <input
                                type="text"
                                class=format!("{INPUT_CLASS} font-mono tabular-nums")
                                disabled=!is_admin
                                placeholder="provider-specific-model-id"
                                prop:value=move || custom_model.get()
                                on:input=move |ev| set_custom_model.set(event_target_value(&ev))
                            />
                        </Show>
                    </div>

                    // ── Advanced disclosure ──────────────────────────────
                    <div>
                        <button
                            type="button"
                            class="text-sm text-muted-foreground hover:text-foreground flex items-center gap-1"
                            on:click=move |_| set_show_advanced.update(|v| *v = !*v)
                        >
                            {move || if show_advanced.get() { "Advanced \u{25BE}" } else { "Advanced \u{25B8}" }}
                        </button>
                        <Show when=move || show_advanced.get()>
                            <div class="space-y-2 mt-2">
                                <Label>"Base URL override"</Label>
                                <input
                                    type="text"
                                    class=format!("{INPUT_CLASS} font-mono tabular-nums")
                                    disabled=!is_admin
                                    placeholder="https://api.anthropic.com"
                                    prop:value=move || base_url.get()
                                    on:input=move |ev| set_base_url.set(event_target_value(&ev))
                                />
                                <p class="text-xs text-muted-foreground">
                                    "Override the API base URL for proxies or custom endpoints."
                                </p>
                            </div>
                        </Show>
                    </div>
                </div>
            </CardContent>
        </Card>
    }
}
