// SPDX-License-Identifier: AGPL-3.0-or-later

//! Choose the provider connection a watch run uses (KYO-973).
//!
//! The pure resolution rules live in [`kyomi_core::watch_llm`]; this binds
//! them to the instance [`Config`] and a workspace's AI configuration and
//! produces an [`LLMProviderConfig`] for the existing provider factory, so a
//! watch run builds its own client instead of using the workspace default.

use kyomi_auth::workspace_ai_config::WorkspaceAiConfig;
use kyomi_core::watch_llm::{resolve_watch_llm, ResolvedWatchLlm, WatchLlmSettings};
use kyomi_core::Config;

use crate::provider::{resolve_workspace_provider_config, LLMProviderConfig, ProviderKind};

/// The connection and options one watch run uses.
#[derive(Debug, Clone)]
pub struct WatchLlmSelection {
    /// Config for [`crate::provider::create_provider`].
    pub provider_config: LLMProviderConfig,
    /// The resolved watch settings (mode, window, failure policy).
    pub resolved: ResolvedWatchLlm,
    /// Whether usage is billed to the workspace's own key rather than to
    /// Kyomi credits.
    pub workspace_billed: bool,
}

/// Resolve the watch LLM for a workspace.
///
/// Workspace overrides apply only on self-hosted deployments, like the main
/// provider and the KYO-972 sampling overrides: in SaaS the server controls
/// provider and model entirely.
pub fn resolve_watch_selection(
    ws_config: &WorkspaceAiConfig,
    app_config: &Config,
) -> kyomi_core::Result<WatchLlmSelection> {
    let none = WatchLlmSettings::default();
    let workspace = if app_config.self_hosted { &ws_config.watch_llm } else { &none };
    let resolved = resolve_watch_llm(workspace, &app_config.llm_watch);

    let (mut provider_config, workspace_billed) = match &resolved.connection {
        Some(c) => (
            LLMProviderConfig {
                provider: ProviderKind::parse(&c.provider)?,
                api_key: c.api_key.clone(),
                model: None,
                base_url: c.base_url.clone(),
                context_window: 0,
            },
            resolved.connection_from_workspace,
        ),
        None => (
            resolve_workspace_provider_config(ws_config, app_config)?,
            ws_config.is_byok() && app_config.self_hosted,
        ),
    };
    // A model belongs to the connection it was configured with; without a
    // watch model the connection's own default applies. For the main
    // connection that default is the main model, so the watch model only
    // overrides when set.
    if resolved.model.is_some() || resolved.connection.is_some() {
        provider_config.model = resolved.model.clone();
    }
    Ok(WatchLlmSelection { provider_config, resolved, workspace_billed })
}

#[cfg(test)]
mod tests {
    use super::*;
    use kyomi_auth::workspace_ai_config::WorkspaceAiProvider;
    use kyomi_core::watch_llm::WatchLlmMode;

    fn ws(watch_llm: WatchLlmSettings) -> WorkspaceAiConfig {
        WorkspaceAiConfig {
            provider: WorkspaceAiProvider::Kyomi,
            model: None,
            api_key: None,
            base_url: None,
            title_model: None,
            context_window: 0,
            sampling: Default::default(),
            sampling_warnings: Vec::new(),
            watch_llm,
            watch_llm_warnings: Vec::new(),
        }
    }

    fn config(self_hosted: bool) -> Config {
        let mut c = Config::test_config();
        c.self_hosted = self_hosted;
        c.llm_provider = Some("anthropic".into());
        c.llm_api_key = Some("main-key".into());
        c.llm_model = Some("main-model".into());
        c
    }

    fn connection(provider: &str, key: &str) -> WatchLlmSettings {
        WatchLlmSettings {
            provider: Some(provider.into()),
            api_key: Some(key.into()),
            ..Default::default()
        }
    }

    #[test]
    fn unset_watch_config_uses_the_main_connection_and_model() {
        let s = resolve_watch_selection(&ws(Default::default()), &config(true)).unwrap();
        assert_eq!(s.provider_config.provider, ProviderKind::Anthropic);
        assert_eq!(s.provider_config.api_key, "main-key");
        assert_eq!(s.provider_config.model.as_deref(), Some("main-model"));
        assert_eq!(s.resolved.mode, WatchLlmMode::Realtime);
        assert!(!s.workspace_billed);
    }

    #[test]
    fn a_watch_model_overrides_only_the_model_on_the_main_connection() {
        let mut c = config(true);
        c.llm_watch.model = Some("cheap".into());
        let s = resolve_watch_selection(&ws(Default::default()), &c).unwrap();
        assert_eq!(s.provider_config.api_key, "main-key");
        assert_eq!(s.provider_config.model.as_deref(), Some("cheap"));
    }

    #[test]
    fn instance_watch_connection_replaces_the_whole_connection_and_never_inherits_the_main_model() {
        let mut c = config(true);
        c.llm_watch = connection("openai", "watch-key");
        let s = resolve_watch_selection(&ws(Default::default()), &c).unwrap();
        assert_eq!(s.provider_config.provider, ProviderKind::OpenAI);
        assert_eq!(s.provider_config.api_key, "watch-key");
        assert_eq!(s.provider_config.model, None, "main-model is for the main provider");
        assert!(!s.workspace_billed);
    }

    #[test]
    fn workspace_override_wins_on_self_hosted_and_is_workspace_billed() {
        let mut c = config(true);
        c.llm_watch = connection("openai", "env-key");
        let mut w = connection("gemini", "ws-key");
        w.model = Some("ws-model".into());
        let s = resolve_watch_selection(&ws(w), &c).unwrap();
        assert_eq!(s.provider_config.provider, ProviderKind::Gemini);
        assert_eq!(s.provider_config.api_key, "ws-key");
        assert_eq!(s.provider_config.model.as_deref(), Some("ws-model"));
        assert!(s.workspace_billed);
    }

    #[test]
    fn saas_ignores_the_workspace_override() {
        let mut c = config(false);
        c.llm_watch = connection("openai", "env-key");
        let s = resolve_watch_selection(&ws(connection("gemini", "ws-key")), &c).unwrap();
        assert_eq!(s.provider_config.provider, ProviderKind::OpenAI);
        assert_eq!(s.provider_config.api_key, "env-key");
        assert!(!s.workspace_billed);
    }

    #[test]
    fn an_unknown_watch_provider_is_an_error() {
        let mut c = config(true);
        c.llm_watch = connection("nonsense", "k");
        assert!(resolve_watch_selection(&ws(Default::default()), &c).is_err());
    }

    // -- End-to-end: which provider / model / effort / temperature a watch
    // -- turn actually sends, for the main run and the JSON-correction retry.

    use crate::execution::{execute_agent_chat, AgentExecutionConfig, AgentExecutionEnv};
    use crate::test_support::{anthropic_text_reply, FakeHttpServer};
    use crate::watch_execution::{build_watch_agent_config, build_watch_retry_config};
    use crate::UserMessagePersistence;
    use kyomi_core::llm_sampling::{LlmSamplingSettings, Setting};

    const VALID: &str = r#"{"should_alert": false, "reason": "ok"}"#;

    async fn run_turn(cfg: AgentExecutionConfig, app_config: Config) {
        let db = crate::test_support::test_pool().await;
        crate::test_support::seed_user_and_workspace(&db).await;
        kyomi_auth::chat_service::create_session_with_id(
            &db, "user-a", "ws-1", &cfg.session_id, Some("Watch: t"), "watch_execution", None,
        )
        .await
        .unwrap();
        let kv = kyomi_core::kv_store_memory::InMemoryKVStore::new_pool();
        let key = std::sync::Arc::new([7u8; 32]);
        let embedding = kyomi_embed::LazyEmbedding::new();
        let ws_manager = kyomi_auth::websocket::WebSocketManager::new(None, db.clone());
        let app_config = std::sync::Arc::new(app_config);
        let result = execute_agent_chat(
            cfg,
            AgentExecutionEnv {
                db: &db,
                kv: &kv,
                encryption_key: &key,
                embedding: &embedding,
                ws_manager: &ws_manager,
                app_config: &app_config,
                connect_registry: None,
                platforms: std::sync::Arc::new(kyomi_core::platform::PlatformRegistry::new()),
            },
        )
        .await
        .unwrap();
        assert_eq!(result.response_text, VALID);
    }

    fn main_turn() -> AgentExecutionConfig {
        build_watch_agent_config(
            &uuid::Uuid::new_v4().to_string(),
            "user-a",
            "ws-1",
            "check revenue".into(),
            "sys".into(),
            Vec::new(),
            UserMessagePersistence::AdapterPersists(None),
        )
    }

    fn retry_turn() -> AgentExecutionConfig {
        build_watch_retry_config(
            &uuid::Uuid::new_v4().to_string(),
            "user-a",
            "ws-1",
            "sys".into(),
            Vec::new(),
            "fix your JSON".into(),
        )
    }

    async fn servers() -> (FakeHttpServer, FakeHttpServer) {
        let reply = || move |_: &crate::test_support::RecordedRequest| (200, anthropic_text_reply(VALID));
        (FakeHttpServer::start(reply()).await, FakeHttpServer::start(reply()).await)
    }

    fn configured(main: &FakeHttpServer, watch: &FakeHttpServer) -> Config {
        let mut c = config(true);
        c.llm_base_url = Some(main.base_url.clone());
        c.llm_watch = WatchLlmSettings {
            provider: Some("anthropic".into()),
            api_key: Some("watch-key".into()),
            base_url: Some(watch.base_url.clone()),
            model: Some("watch-model".into()),
            ..Default::default()
        };
        c.llm_sampling = LlmSamplingSettings {
            reasoning_effort: Some(Setting::Value("high".into())),
            watch_reasoning_effort: Some(Setting::Value("low".into())),
            watch_temperature: Some(Setting::Value(0.2)),
            ..Default::default()
        };
        c
    }

    #[tokio::test]
    async fn the_main_watch_run_sends_watch_provider_model_effort_and_temperature() {
        let (main, watch) = servers().await;
        run_turn(main_turn(), configured(&main, &watch)).await;
        assert!(main.requests().is_empty(), "the main LLM must not be called");
        let reqs = watch.requests();
        assert_eq!(reqs.len(), 1);
        assert_eq!(reqs[0].headers.get("x-api-key").map(String::as_str), Some("watch-key"));
        let body = reqs[0].json();
        assert_eq!(body["model"], "watch-model");
        assert_eq!(body["output_config"]["effort"], "low");
        assert_eq!(body["temperature"], 0.2f32 as f64);
    }

    #[tokio::test]
    async fn the_json_correction_retry_uses_the_same_watch_configuration() {
        let (main, watch) = servers().await;
        run_turn(retry_turn(), configured(&main, &watch)).await;
        assert!(main.requests().is_empty(), "the retry must not fall back to the main LLM");
        let body = watch.requests()[0].json();
        assert_eq!(watch.requests()[0].headers.get("x-api-key").map(String::as_str), Some("watch-key"));
        assert_eq!(body["model"], "watch-model");
        assert_eq!(body["output_config"]["effort"], "low");
        assert_eq!(body["temperature"], 0.2f32 as f64);
    }

    #[tokio::test]
    async fn watch_runs_use_the_main_llm_and_chat_tuning_when_no_watch_config_is_set() {
        let main = FakeHttpServer::start(|_| (200, anthropic_text_reply(VALID))).await;
        let mut c = config(true);
        c.llm_base_url = Some(main.base_url.clone());
        c.llm_sampling = LlmSamplingSettings {
            reasoning_effort: Some(Setting::Value("high".into())),
            ..Default::default()
        };
        run_turn(main_turn(), c.clone()).await;
        run_turn(retry_turn(), c).await;
        let reqs = main.requests();
        assert_eq!(reqs.len(), 2);
        for r in reqs {
            assert_eq!(r.headers.get("x-api-key").map(String::as_str), Some("main-key"));
            let body = r.json();
            assert_eq!(body["model"], "main-model");
            assert_eq!(body["output_config"]["effort"], "high", "watch inherits the chat effort");
            assert_eq!(body["temperature"], 0.7f32 as f64);
        }
    }

    #[tokio::test]
    async fn a_watch_model_alone_changes_the_model_on_the_main_connection() {
        let main = FakeHttpServer::start(|_| (200, anthropic_text_reply(VALID))).await;
        let mut c = config(true);
        c.llm_base_url = Some(main.base_url.clone());
        c.llm_watch.model = Some("cheap-model".into());
        run_turn(main_turn(), c).await;
        let r = &main.requests()[0];
        assert_eq!(r.headers.get("x-api-key").map(String::as_str), Some("main-key"));
        assert_eq!(r.json()["model"], "cheap-model");
    }
}
