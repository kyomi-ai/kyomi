// SPDX-License-Identifier: AGPL-3.0-or-later

//! Resolve the temperature / reasoning effort one LLM call actually sends
//! (KYO-972). The pure precedence rules live in
//! [`kyomi_core::llm_sampling`]; this binds them to the instance [`Config`]
//! and a workspace's AI configuration.

use kyomi_auth::workspace_ai_config::WorkspaceAiConfig;
use kyomi_core::llm_sampling::{resolve_sampling, LlmProfile, LlmSamplingSettings, ResolvedSampling};
use kyomi_core::Config;

use crate::provider::RequestOptions;

/// Resolve sampling for `profile`.
///
/// Workspace overrides apply only on self-hosted deployments: in SaaS the
/// server controls provider and model entirely (see
/// `create_provider_from_workspace`), so a workspace value chosen for some
/// other model must not leak into Kyomi-managed calls.
///
/// `call_default_temperature` is the call site's historical value, used when
/// nothing is configured. `title_model_configured` is whether a dedicated
/// title model (workspace `title_model` or `LLM_TITLE_MODEL`) is in effect,
/// which switches the background-call effort default to `low`.
pub fn resolve_for_call(
    profile: LlmProfile,
    ws_config: &WorkspaceAiConfig,
    app_config: &Config,
    call_default_temperature: Option<f32>,
    title_model_configured: bool,
) -> ResolvedSampling {
    let none = LlmSamplingSettings::default();
    let workspace = if app_config.self_hosted {
        &ws_config.sampling
    } else {
        &none
    };
    resolve_sampling(
        profile,
        workspace,
        &app_config.llm_sampling,
        call_default_temperature,
        title_model_configured,
    )
}

/// Whether a dedicated title model is configured for this workspace.
pub fn title_model_configured(ws_config: &WorkspaceAiConfig, app_config: &Config) -> bool {
    let ws_title = app_config.self_hosted && ws_config.title_model.is_some();
    ws_title || app_config.llm_title_model.is_some()
}

/// Request options for a short background call (title, summary, tagging).
pub fn background_options(
    ws_config: &WorkspaceAiConfig,
    app_config: &Config,
    call_default_temperature: f32,
    max_tokens: u32,
) -> RequestOptions {
    let resolved = resolve_for_call(
        LlmProfile::Title,
        ws_config,
        app_config,
        Some(call_default_temperature),
        title_model_configured(ws_config, app_config),
    );
    RequestOptions::new(max_tokens)
        .with_temperature(resolved.temperature)
        .with_reasoning_effort(resolved.reasoning_effort)
}
