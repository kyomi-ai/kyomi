// SPDX-License-Identifier: AGPL-3.0-or-later

//! Resolve the temperature / reasoning effort one LLM call actually sends
//! (KYO-972, simplified by KYO-974: unset sends nothing, set is passed
//! through as configured). The pure precedence rules live in
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
/// `title_model_configured` is whether a dedicated
/// title model (workspace `title_model` or `LLM_TITLE_MODEL`) is in effect,
/// which switches the background-call effort default to `low`.
pub fn resolve_for_call(
    profile: LlmProfile,
    ws_config: &WorkspaceAiConfig,
    app_config: &Config,
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
        title_model_configured,
    )
}

/// Whether a dedicated title model is configured for this workspace.
pub fn title_model_configured(ws_config: &WorkspaceAiConfig, app_config: &Config) -> bool {
    // Mirrors what the title/summary/tag callers request: the workspace title model, else
    // `LLM_TITLE_MODEL`, on every setup.
    ws_config.title_model.is_some() || app_config.llm_title_model.is_some()
}

/// Request options for a short background call (title, summary, tagging).
pub fn background_options(
    ws_config: &WorkspaceAiConfig,
    app_config: &Config,
    max_tokens: u32,
) -> RequestOptions {
    let resolved = resolve_for_call(
        LlmProfile::Title,
        ws_config,
        app_config,
        title_model_configured(ws_config, app_config),
    );
    RequestOptions::new(max_tokens)
        .with_temperature(resolved.temperature)
        .with_reasoning_effort(resolved.reasoning_effort)
}

#[cfg(test)]
mod tests {
    use super::*;
    use kyomi_auth::workspace_ai_config::WorkspaceAiProvider;
    use serde_json::json;

    fn effort(v: &str) -> Option<String> {
        Some(v.into())
    }

    fn ws(sampling: LlmSamplingSettings, title_model: Option<&str>) -> WorkspaceAiConfig {
        WorkspaceAiConfig {
            provider: WorkspaceAiProvider::Kyomi,
            model: None,
            api_key: None,
            base_url: None,
            title_model: title_model.map(str::to_string),
            context_window: 0,
            sampling,
            sampling_warnings: Vec::new(),
            watch_llm: Default::default(),
            watch_llm_warnings: Vec::new(),
        }
    }

    fn config(self_hosted: bool) -> Config {
        let mut c = Config::test_config();
        c.self_hosted = self_hosted;
        c.llm_sampling = LlmSamplingSettings {
            reasoning_effort: effort("env"),
            temperature: Some(json!(0.5)),
            ..Default::default()
        };
        c
    }

    fn workspace_override() -> LlmSamplingSettings {
        LlmSamplingSettings {
            reasoning_effort: effort("workspace"),
            temperature: Some(json!(1.0)),
            ..Default::default()
        }
    }

    #[test]
    fn self_hosted_workspace_override_beats_the_instance_value() {
        let r = resolve_for_call(
            LlmProfile::Chat,
            &ws(workspace_override(), None),
            &config(true),
            false,
        );
        assert_eq!(r.reasoning_effort.as_deref(), Some("workspace"));
        assert_eq!(r.temperature, Some(json!(1.0)), "workspace value wins over env 0.5");
    }

    #[test]
    fn saas_ignores_workspace_overrides_entirely() {
        let r = resolve_for_call(
            LlmProfile::Chat,
            &ws(workspace_override(), None),
            &config(false),
            false,
        );
        assert_eq!(r.reasoning_effort.as_deref(), Some("env"));
        assert_eq!(r.temperature, Some(json!(0.5)));
    }

    #[test]
    fn title_model_configured_follows_the_model_the_callers_request() {
        let mut c = config(true);
        assert!(title_model_configured(&ws(Default::default(), Some("m")), &c));
        assert!(!title_model_configured(&ws(Default::default(), None), &c));
        // The instance title model counts too.
        c.llm_title_model = Some("env-title".into());
        assert!(title_model_configured(&ws(Default::default(), None), &c));
        // On SaaS the workspace title model is requested too, so it counts.
        let saas = config(false);
        assert!(title_model_configured(&ws(Default::default(), Some("m")), &saas));
        assert!(!title_model_configured(&ws(Default::default(), None), &saas));
    }

    #[test]
    fn background_options_use_the_title_profile_and_default_low_once_effort_is_configured() {
        let mut c = config(true);
        c.llm_title_model = Some("cheap".into());
        let o = background_options(&ws(Default::default(), None), &c, 512);
        assert_eq!(o.reasoning_effort.as_deref(), Some("low"), "chat effort env is set");
        assert_eq!(o.temperature, None, "title never inherits the chat temperature");
        assert_eq!(o.max_tokens, 512);

        // Existing install: title model but no effort anywhere sends nothing.
        let mut bare = Config::test_config();
        bare.self_hosted = true;
        bare.llm_title_model = Some("cheap".into());
        let o = background_options(&ws(Default::default(), None), &bare, 512);
        assert_eq!(o.reasoning_effort, None);
        assert_eq!(o.temperature, None, "unset sends nothing");
    }
}
