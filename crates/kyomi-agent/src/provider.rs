// SPDX-License-Identifier: AGPL-3.0-or-later

//! LLM provider abstraction layer.
//!
//! Defines the [`LLMProvider`] trait that decouples the agent from any specific
//! LLM vendor. Concrete implementations (Anthropic, OpenAI, Gemini, etc.) live
//! behind this trait so self-hosters can swap providers via configuration.
//!
//! # Factory
//!
//! Use [`create_provider`] to instantiate a provider from [`LLMProviderConfig`],
//! or [`resolve_provider_config`] to build the config from application settings.
//!
//! For workspace-scoped (BYOK) routing, use
//! [`create_provider_from_workspace`] which takes a
//! [`kyomi_auth::workspace_ai_config::WorkspaceAiConfig`] and falls back to
//! server env keys when the workspace is in Kyomi-managed mode.

use std::collections::HashMap;
use std::fmt;

use async_trait::async_trait;

use crate::anthropic::AnthropicClient;
use crate::gemini::GeminiProvider;
use crate::openai::OpenAIProvider;
use crate::types::{LLMResponse, Message, Tool};

// ---------------------------------------------------------------------------
// Trait
// ---------------------------------------------------------------------------

/// Trait that all LLM providers must implement.
///
/// The signature mirrors `AnthropicClient::complete` — provider implementations
/// are responsible for translating Kyomi's internal message/tool types into the
/// vendor-specific API format and back.
#[async_trait]
pub trait LLMProvider: Send + Sync {
    /// Send a completion request to the LLM.
    async fn complete(
        &self,
        messages: &[Message],
        tools: &[Tool],
        temperature: Option<f32>,
        max_tokens: u32,
        user_names: &HashMap<String, String>,
    ) -> kyomi_core::Result<LLMResponse>;

    /// Return the model identifier (e.g., "claude-haiku-4-5-20251001").
    fn model(&self) -> &str;
}

// ---------------------------------------------------------------------------
// ProviderBase — shared fields for all LLM provider structs
// ---------------------------------------------------------------------------

/// Shared fields common to all LLM provider implementations.
///
/// Each concrete provider embeds this as `base: ProviderBase` and delegates
/// the `model()` accessor to it. Constructed via [`ProviderBase::new`] (which
/// uses the shared TLS-aware HTTP client from `kyomi_datasource_server`) or
/// [`ProviderBase::with_base_url`] (for testing with a plain `reqwest::Client`).
pub struct ProviderBase {
    pub(crate) client: reqwest::Client,
    pub(crate) api_key: String,
    pub(crate) model: String,
    pub(crate) base_url: String,
}

impl ProviderBase {
    /// Create a new `ProviderBase` using the shared TLS-aware HTTP client.
    ///
    /// # Arguments
    /// * `api_key` - Provider API key.
    /// * `model` - Model override; `default_model` is used when `None`.
    /// * `default_model` - Fallback model identifier.
    /// * `base_url` - URL override; `default_base_url` is used when `None`.
    /// * `default_base_url` - Fallback API endpoint URL.
    pub fn new(
        api_key: String,
        model: Option<String>,
        default_model: &str,
        base_url: Option<String>,
        default_base_url: &str,
    ) -> kyomi_core::Result<Self> {
        Ok(Self {
            client: kyomi_datasource_server::http_client()?,
            api_key,
            model: model.unwrap_or_else(|| default_model.to_string()),
            base_url: base_url.unwrap_or_else(|| default_base_url.to_string()),
        })
    }

    /// Create a `ProviderBase` pointing at a custom API URL.
    ///
    /// Uses the shared TLS-aware HTTP client from `kyomi_datasource_server`.
    ///
    /// # Arguments
    /// * `api_key` - Provider API key.
    /// * `model` - Model override; `default_model` is used when `None`.
    /// * `default_model` - Fallback model identifier.
    /// * `base_url` - Custom API endpoint URL.
    pub fn with_base_url(
        api_key: String,
        model: Option<String>,
        default_model: &str,
        base_url: String,
    ) -> kyomi_core::Result<Self> {
        Ok(Self {
            client: kyomi_datasource_server::http_client()?,
            api_key,
            model: model.unwrap_or_else(|| default_model.to_string()),
            base_url,
        })
    }

    /// Return the model name this base is configured with.
    pub fn model(&self) -> &str {
        &self.model
    }
}

// ---------------------------------------------------------------------------
// Provider kind
// ---------------------------------------------------------------------------

/// Supported LLM provider backends.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderKind {
    Anthropic,
    OpenAI,
    Gemini,
}

impl fmt::Display for ProviderKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Anthropic => write!(f, "anthropic"),
            Self::OpenAI => write!(f, "openai"),
            Self::Gemini => write!(f, "gemini"),
        }
    }
}

impl ProviderKind {
    /// Parse a provider name string (case-insensitive).
    pub fn parse(s: &str) -> kyomi_core::Result<Self> {
        match s.to_lowercase().as_str() {
            "anthropic" => Ok(Self::Anthropic),
            "openai" => Ok(Self::OpenAI),
            "gemini" => Ok(Self::Gemini),
            other => Err(kyomi_core::Error::Internal(format!(
                "unknown LLM provider: {other:?}. Supported: anthropic, openai, gemini"
            ))),
        }
    }

    /// Returns the cheapest known model for background/low-stakes tasks
    /// (e.g. title generation, classification).
    ///
    /// For standard API endpoints this returns a hardcoded cheap model.
    /// Callers should NOT use this when the provider has a custom `base_url` —
    /// we cannot know which models a custom endpoint supports.
    pub fn cheapest_model(&self) -> &'static str {
        match self {
            Self::Anthropic => "claude-haiku-4-5-20251001",
            Self::OpenAI => "gpt-4.1-mini",
            Self::Gemini => "gemini-2.0-flash",
        }
    }
}

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

/// Configuration needed to instantiate an [`LLMProvider`].
#[derive(Debug, Clone)]
pub struct LLMProviderConfig {
    /// Which provider backend to use.
    pub provider: ProviderKind,
    /// API key for the provider.
    pub api_key: String,
    /// Model override (provider default used when `None`).
    pub model: Option<String>,
    /// Custom base URL (e.g., for proxies or OpenAI-compatible APIs).
    pub base_url: Option<String>,
}

// ---------------------------------------------------------------------------
// Trait impl for AnthropicClient
// ---------------------------------------------------------------------------

#[async_trait]
impl LLMProvider for AnthropicClient {
    async fn complete(
        &self,
        messages: &[Message],
        tools: &[Tool],
        temperature: Option<f32>,
        max_tokens: u32,
        user_names: &HashMap<String, String>,
    ) -> kyomi_core::Result<LLMResponse> {
        // Delegate to the existing concrete method.
        AnthropicClient::complete(self, messages, tools, temperature, max_tokens, user_names).await
    }

    fn model(&self) -> &str {
        AnthropicClient::model(self)
    }
}

#[async_trait]
impl LLMProvider for OpenAIProvider {
    async fn complete(
        &self,
        messages: &[Message],
        tools: &[Tool],
        temperature: Option<f32>,
        max_tokens: u32,
        user_names: &HashMap<String, String>,
    ) -> kyomi_core::Result<LLMResponse> {
        OpenAIProvider::complete(self, messages, tools, temperature, max_tokens, user_names).await
    }

    fn model(&self) -> &str {
        OpenAIProvider::model(self)
    }
}

#[async_trait]
impl LLMProvider for GeminiProvider {
    async fn complete(
        &self,
        messages: &[Message],
        tools: &[Tool],
        temperature: Option<f32>,
        max_tokens: u32,
        user_names: &HashMap<String, String>,
    ) -> kyomi_core::Result<LLMResponse> {
        GeminiProvider::complete(self, messages, tools, temperature, max_tokens, user_names).await
    }

    fn model(&self) -> &str {
        GeminiProvider::model(self)
    }
}

// ---------------------------------------------------------------------------
// Factory
// ---------------------------------------------------------------------------

/// Create an [`LLMProvider`] from the given configuration.
///
/// Supports Anthropic, OpenAI, and Gemini providers. The provider kind
/// determines which concrete implementation is instantiated.
pub fn create_provider(
    config: LLMProviderConfig,
) -> kyomi_core::Result<Box<dyn LLMProvider>> {
    match config.provider {
        ProviderKind::Anthropic => {
            let client = if let Some(base_url) = config.base_url {
                AnthropicClient::with_base_url(config.api_key, config.model, base_url)?
            } else {
                AnthropicClient::new(config.api_key, config.model)?
            };
            Ok(Box::new(client))
        }
        ProviderKind::OpenAI => {
            let client =
                crate::openai::OpenAIProvider::new(config.api_key, config.model, config.base_url)?;
            Ok(Box::new(client))
        }
        ProviderKind::Gemini => {
            let client = crate::gemini::GeminiProvider::new(
                config.api_key,
                config.model,
                config.base_url,
            )?;
            Ok(Box::new(client))
        }
    }
}

// ---------------------------------------------------------------------------
// Workspace-scoped factory (BYOK)
// ---------------------------------------------------------------------------

/// Build an [`LLMProvider`] from a workspace AI config.
///
/// * **Kyomi mode** (`ws_config.provider == WorkspaceAiProvider::Kyomi`):
///   falls back to `fallback_config` — the server's env-configured keys
///   (`ANTHROPIC_API_KEY` / `LLM_API_KEY`). Matches the legacy behaviour of
///   [`create_provider`] + [`resolve_provider_config`] exactly so that
///   existing Kyomi-hosted tenants see no change.
///   * Model override: `ws_config.model` wins over `fallback_config.llm_model`.
///   * Base URL: inherited from `fallback_config.llm_base_url` (workspaces
///     in Kyomi mode never set their own base URL).
///
/// * **BYOK mode** (anthropic / openai / gemini): uses the workspace's
///   decrypted API key, model, and optional base URL. Returns
///   [`kyomi_core::Error::Internal`] if the workspace is BYOK but has no
///   stored API key (can only happen if Track 2's [`update`] invariant was
///   bypassed via direct DB write).
///
/// Prefer this over [`create_provider`] anywhere a workspace ID is in scope.
/// [`create_provider`] remains for anonymous entry points (trial chat) that
/// have no workspace context.
pub fn create_provider_from_workspace(
    ws_config: &kyomi_auth::workspace_ai_config::WorkspaceAiConfig,
    fallback_config: &kyomi_core::Config,
) -> kyomi_core::Result<Box<dyn LLMProvider>> {
    use kyomi_auth::workspace_ai_config::WorkspaceAiProvider;

    let llm_config = match ws_config.provider {
        WorkspaceAiProvider::Kyomi => {
            // Server-side keys: reuse the existing resolver so Kyomi-mode
            // workspaces get identical behaviour to the old global path.
            let mut resolved = resolve_provider_config(fallback_config)?;
            if let Some(ref model) = ws_config.model {
                resolved.model = Some(model.clone());
            }
            resolved
        }
        WorkspaceAiProvider::Anthropic
        | WorkspaceAiProvider::OpenAI
        | WorkspaceAiProvider::Gemini => {
            let api_key = ws_config.api_key.clone().ok_or_else(|| {
                kyomi_core::Error::Internal(format!(
                    "workspace BYOK provider {} has no stored API key",
                    ws_config.provider.as_str()
                ))
            })?;
            let provider = match ws_config.provider {
                WorkspaceAiProvider::Anthropic => ProviderKind::Anthropic,
                WorkspaceAiProvider::OpenAI => ProviderKind::OpenAI,
                WorkspaceAiProvider::Gemini => ProviderKind::Gemini,
                WorkspaceAiProvider::Kyomi => unreachable!("handled above"),
            };
            LLMProviderConfig {
                provider,
                api_key,
                model: ws_config.model.clone(),
                base_url: ws_config.base_url.clone(),
            }
        }
    };

    create_provider(llm_config)
}

// ---------------------------------------------------------------------------
// Config resolver
// ---------------------------------------------------------------------------

/// Build an [`LLMProviderConfig`] from application-level [`Config`](kyomi_core::Config).
///
/// Resolution order:
/// 1. If `LLM_PROVIDER` + `LLM_API_KEY` are set, use those explicitly.
/// 2. Else if `ANTHROPIC_API_KEY` is set, default to `ProviderKind::Anthropic`.
/// 3. Otherwise, return an error.
///
/// `LLM_MODEL` and `LLM_BASE_URL` are used as overrides when present.
pub fn resolve_provider_config(
    config: &kyomi_core::Config,
) -> kyomi_core::Result<LLMProviderConfig> {
    // Explicit provider configuration takes priority.
    if let (Some(provider_str), Some(api_key)) =
        (config.llm_provider.as_deref(), config.llm_api_key.as_deref())
    {
        let provider = ProviderKind::parse(provider_str)?;
        return Ok(LLMProviderConfig {
            provider,
            api_key: api_key.to_string(),
            model: config.llm_model.clone(),
            base_url: config.llm_base_url.clone(),
        });
    }

    // Warn about partial config: LLM_PROVIDER set without LLM_API_KEY.
    if config.llm_provider.is_some() && config.llm_api_key.is_none() {
        return Err(kyomi_core::Error::Internal(
            "LLM_PROVIDER is set but LLM_API_KEY is missing. \
             Both must be provided together."
                .into(),
        ));
    }

    // Fall back to the legacy ANTHROPIC_API_KEY.
    if let Some(api_key) = config.anthropic_api_key.as_deref() {
        return Ok(LLMProviderConfig {
            provider: ProviderKind::Anthropic,
            api_key: api_key.to_string(),
            model: config.llm_model.clone(),
            base_url: config.llm_base_url.clone(),
        });
    }

    Err(kyomi_core::Error::Internal(
        "no LLM provider configured: set LLM_PROVIDER + LLM_API_KEY, \
         or ANTHROPIC_API_KEY"
            .into(),
    ))
}

// ---------------------------------------------------------------------------
// LLM debug logging (shared across all providers)
// ---------------------------------------------------------------------------

/// Log an LLM API request or response to a JSON file for diagnostics.
///
/// Gated behind `LOG_LLM_CONTEXT=true` (same env var as the Python backend).
/// Files are written to `logs/llm_context/` relative to the working directory.
///
/// `provider_name` is used in the filename (e.g. "anthropic", "openai").
pub fn maybe_log_llm(provider_name: &str, label: &str, payload: &serde_json::Value) {
    use std::sync::OnceLock;
    static ENABLED: OnceLock<bool> = OnceLock::new();

    let enabled = *ENABLED.get_or_init(|| {
        std::env::var("LOG_LLM_CONTEXT")
            .map(|v| v.eq_ignore_ascii_case("true"))
            .unwrap_or(false)
    });

    if !enabled {
        return;
    }

    let log_dir = "logs/llm_context";
    if let Err(e) = std::fs::create_dir_all(log_dir) {
        tracing::warn!(error = %e, "Failed to create LLM log directory");
        return;
    }

    let now = chrono::Utc::now();
    let filename = format!(
        "{}/{}_{}_{}_{label}.json",
        log_dir,
        now.format("%Y%m%d_%H%M%S"),
        now.timestamp_subsec_micros(),
        provider_name,
    );

    match std::fs::File::create(&filename) {
        Ok(file) => {
            if let Err(e) = serde_json::to_writer_pretty(file, payload) {
                tracing::warn!(error = %e, "Failed to write LLM log");
            } else {
                tracing::info!(path = %filename, "Logged {provider_name} {label}");
            }
        }
        Err(e) => {
            tracing::warn!(error = %e, path = %filename, "Failed to create LLM log file");
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

pub(crate) fn extract_error_message(body: &str) -> Option<String> {
    let json: serde_json::Value = serde_json::from_str(body).ok()?;
    json.get("error")
        .and_then(|e| e.get("message"))
        .and_then(|m| m.as_str())
        .map(String::from)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_kind_parse_case_insensitive() {
        assert_eq!(ProviderKind::parse("Anthropic").unwrap(), ProviderKind::Anthropic);
        assert_eq!(ProviderKind::parse("OPENAI").unwrap(), ProviderKind::OpenAI);
        assert_eq!(ProviderKind::parse("gemini").unwrap(), ProviderKind::Gemini);
    }

    #[test]
    fn provider_kind_parse_unknown_errors() {
        assert!(ProviderKind::parse("llama").is_err());
    }

    #[test]
    fn provider_kind_display() {
        assert_eq!(ProviderKind::Anthropic.to_string(), "anthropic");
        assert_eq!(ProviderKind::OpenAI.to_string(), "openai");
        assert_eq!(ProviderKind::Gemini.to_string(), "gemini");
    }

    #[test]
    fn cheapest_model_returns_expected_models() {
        assert_eq!(ProviderKind::Anthropic.cheapest_model(), "claude-haiku-4-5-20251001");
        assert_eq!(ProviderKind::OpenAI.cheapest_model(), "gpt-4.1-mini");
        assert_eq!(ProviderKind::Gemini.cheapest_model(), "gemini-2.0-flash");
    }

    #[test]
    fn create_provider_openai_succeeds() {
        let config = LLMProviderConfig {
            provider: ProviderKind::OpenAI,
            api_key: "sk-test".into(),
            model: None,
            base_url: None,
        };
        let provider = create_provider(config).expect("should create OpenAI provider");
        assert_eq!(provider.model(), crate::openai::DEFAULT_MODEL);
    }

    #[test]
    fn create_provider_openai_with_custom_model() {
        let config = LLMProviderConfig {
            provider: ProviderKind::OpenAI,
            api_key: "sk-test".into(),
            model: Some("gpt-4o".into()),
            base_url: None,
        };
        let provider = create_provider(config).expect("should create OpenAI provider");
        assert_eq!(provider.model(), "gpt-4o");
    }

    #[test]
    fn create_provider_gemini_succeeds() {
        let config = LLMProviderConfig {
            provider: ProviderKind::Gemini,
            api_key: "test-key".into(),
            model: None,
            base_url: None,
        };
        let provider = create_provider(config).expect("should create Gemini provider");
        assert_eq!(provider.model(), crate::gemini::DEFAULT_MODEL);
    }

    #[test]
    fn create_provider_gemini_with_custom_model() {
        let config = LLMProviderConfig {
            provider: ProviderKind::Gemini,
            api_key: "test-key".into(),
            model: Some("gemini-2.0-pro".into()),
            base_url: None,
        };
        let provider = create_provider(config).expect("should create Gemini provider");
        assert_eq!(provider.model(), "gemini-2.0-pro");
    }

    #[test]
    fn resolve_config_explicit_provider() {
        let mut config = kyomi_core::Config::test_config();
        config.llm_provider = Some("openai".into());
        config.llm_api_key = Some("sk-test".into());
        config.llm_model = Some("gpt-4o".into());
        config.llm_base_url = Some("https://custom.api".into());

        let resolved = resolve_provider_config(&config).unwrap();
        assert_eq!(resolved.provider, ProviderKind::OpenAI);
        assert_eq!(resolved.api_key, "sk-test");
        assert_eq!(resolved.model.as_deref(), Some("gpt-4o"));
        assert_eq!(resolved.base_url.as_deref(), Some("https://custom.api"));
    }

    #[test]
    fn resolve_config_falls_back_to_anthropic_key() {
        let mut config = kyomi_core::Config::test_config();
        config.llm_provider = None;
        config.llm_api_key = None;
        config.anthropic_api_key = Some("sk-ant-test".into());

        let resolved = resolve_provider_config(&config).unwrap();
        assert_eq!(resolved.provider, ProviderKind::Anthropic);
        assert_eq!(resolved.api_key, "sk-ant-test");
    }

    #[test]
    fn resolve_config_no_keys_errors() {
        let mut config = kyomi_core::Config::test_config();
        config.llm_provider = None;
        config.llm_api_key = None;
        config.anthropic_api_key = None;

        assert!(resolve_provider_config(&config).is_err());
    }

    #[test]
    fn resolve_config_partial_provider_without_key_errors() {
        let mut config = kyomi_core::Config::test_config();
        config.llm_provider = Some("openai".into());
        config.llm_api_key = None;
        config.anthropic_api_key = None;

        let err = resolve_provider_config(&config).unwrap_err();
        assert!(
            err.to_string().contains("LLM_API_KEY is missing"),
            "expected partial config error, got: {err}"
        );
    }

    #[test]
    fn workspace_factory_kyomi_mode_uses_server_env_keys() {
        use kyomi_auth::workspace_ai_config::{WorkspaceAiConfig, WorkspaceAiProvider};

        let mut fallback = kyomi_core::Config::test_config();
        fallback.llm_provider = None;
        fallback.llm_api_key = None;
        fallback.anthropic_api_key = Some("sk-ant-server".into());
        fallback.llm_model = Some("claude-sonnet-4-20250514".into());

        let ws = WorkspaceAiConfig {
            provider: WorkspaceAiProvider::Kyomi,
            model: None,
            api_key: None,
            base_url: None,
        };

        let provider = create_provider_from_workspace(&ws, &fallback)
            .expect("Kyomi-mode dispatch should succeed with server env key");
        // Server default model flows through.
        assert_eq!(provider.model(), "claude-sonnet-4-20250514");
    }

    #[test]
    fn workspace_factory_kyomi_mode_workspace_model_overrides_server() {
        use kyomi_auth::workspace_ai_config::{WorkspaceAiConfig, WorkspaceAiProvider};

        let mut fallback = kyomi_core::Config::test_config();
        fallback.anthropic_api_key = Some("sk-ant-server".into());
        fallback.llm_model = Some("claude-sonnet-4-20250514".into());

        let ws = WorkspaceAiConfig {
            provider: WorkspaceAiProvider::Kyomi,
            model: Some("claude-haiku-4-5-20251001".into()),
            api_key: None,
            base_url: None,
        };

        let provider = create_provider_from_workspace(&ws, &fallback).unwrap();
        assert_eq!(provider.model(), "claude-haiku-4-5-20251001");
    }

    #[test]
    fn workspace_factory_byok_openai_uses_workspace_key() {
        use kyomi_auth::workspace_ai_config::{WorkspaceAiConfig, WorkspaceAiProvider};

        // Deliberately empty fallback — BYOK must not read from it.
        let mut fallback = kyomi_core::Config::test_config();
        fallback.llm_provider = None;
        fallback.llm_api_key = None;
        fallback.anthropic_api_key = None;

        let ws = WorkspaceAiConfig {
            provider: WorkspaceAiProvider::OpenAI,
            model: Some("gpt-4o".into()),
            api_key: Some("sk-ws-byok".into()),
            base_url: None,
        };

        let provider = create_provider_from_workspace(&ws, &fallback)
            .expect("BYOK OpenAI dispatch should not need fallback keys");
        assert_eq!(provider.model(), "gpt-4o");
    }

    #[test]
    fn workspace_factory_byok_gemini_default_model() {
        use kyomi_auth::workspace_ai_config::{WorkspaceAiConfig, WorkspaceAiProvider};

        let fallback = kyomi_core::Config::test_config();
        let ws = WorkspaceAiConfig {
            provider: WorkspaceAiProvider::Gemini,
            model: None,
            api_key: Some("AIza-test".into()),
            base_url: None,
        };

        let provider = create_provider_from_workspace(&ws, &fallback).unwrap();
        assert_eq!(provider.model(), crate::gemini::DEFAULT_MODEL);
    }

    #[test]
    fn workspace_factory_byok_without_key_errors() {
        use kyomi_auth::workspace_ai_config::{WorkspaceAiConfig, WorkspaceAiProvider};

        let fallback = kyomi_core::Config::test_config();
        let ws = WorkspaceAiConfig {
            provider: WorkspaceAiProvider::Anthropic,
            model: None,
            api_key: None,
            base_url: None,
        };

        let err = match create_provider_from_workspace(&ws, &fallback) {
            Ok(_) => panic!("expected BYOK-without-key to error"),
            Err(e) => e,
        };
        assert!(
            err.to_string().contains("no stored API key"),
            "expected missing-key error, got: {err}"
        );
    }

    #[test]
    fn resolve_config_explicit_provider_no_model() {
        let mut config = kyomi_core::Config::test_config();
        config.llm_provider = Some("anthropic".into());
        config.llm_api_key = Some("sk-test".into());
        config.llm_model = None;
        config.llm_base_url = None;

        let resolved = resolve_provider_config(&config).unwrap();
        assert_eq!(resolved.provider, ProviderKind::Anthropic);
        assert!(resolved.model.is_none());
        assert!(resolved.base_url.is_none());
    }

    // -- Error message extraction tests -------------------------------------

    #[test]
    fn extract_error_message_valid_json() {
        let body = r#"{"error": {"type": "invalid_request_error", "message": "max_tokens is required"}}"#;
        assert_eq!(
            extract_error_message(body),
            Some("max_tokens is required".to_string())
        );
    }

    #[test]
    fn extract_error_message_no_message_field() {
        let body = r#"{"error": {"type": "server_error"}}"#;
        assert_eq!(extract_error_message(body), None);
    }

    #[test]
    fn extract_error_message_invalid_json() {
        let body = "not json";
        assert_eq!(extract_error_message(body), None);
    }

    #[test]
    fn extract_error_message_empty_body() {
        assert_eq!(extract_error_message(""), None);
    }
}
