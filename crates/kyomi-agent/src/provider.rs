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

/// Per-request generation options shared by every provider.
///
/// `temperature` and `reasoning_effort` are sent only when `Some`; `None`
/// omits the field from the request body entirely (KYO-972). `reasoning_effort`
/// is an opaque string passed to the provider unchanged — each adapter decides
/// the wire field and whether a numeric value is sent as a number.
/// `temperature` is passed through exactly as configured (a JSON number, or a
/// string left for the provider to reject; KYO-974).
#[derive(Debug, Clone, PartialEq)]
pub struct RequestOptions {
    /// Sampling temperature as configured; `None` omits the field.
    pub temperature: Option<serde_json::Value>,
    /// Reasoning effort / thinking level; `None` omits the field.
    pub reasoning_effort: Option<String>,
    /// Maximum tokens to generate (reasoning tokens count against it).
    pub max_tokens: u32,
    /// Provider-executed server tools to add to the request (web search /
    /// fetch, KYO-969), already in the provider's wire shape. Empty for every
    /// ordinary call. Adapters that cannot run them ignore the field; startup
    /// validation prevents pairing such a provider with a server-tool backend.
    pub server_tools: Vec<serde_json::Value>,
}

impl RequestOptions {
    /// Options with no temperature and no reasoning effort.
    pub fn new(max_tokens: u32) -> Self {
        Self { temperature: None, reasoning_effort: None, max_tokens, server_tools: Vec::new() }
    }

    /// Builder: set the temperature.
    pub fn with_temperature(mut self, temperature: Option<serde_json::Value>) -> Self {
        self.temperature = temperature;
        self
    }

    /// Builder: set the provider-executed server tools.
    pub fn with_server_tools(mut self, server_tools: Vec<serde_json::Value>) -> Self {
        self.server_tools = server_tools;
        self
    }

    /// Builder: set the reasoning effort.
    pub fn with_reasoning_effort(mut self, effort: Option<String>) -> Self {
        self.reasoning_effort = effort;
        self
    }
}

/// Whether a response stopped on the token limit with nothing usable in it:
/// no text and no tool calls. With reasoning enabled the hidden reasoning
/// tokens count against `max_tokens`, so a small budget can be consumed
/// entirely before any answer is produced.
pub fn reasoning_exhausted_budget(response: &LLMResponse) -> bool {
    matches!(response.finish_reason.as_str(), "length" | "max_tokens")
        && response.content.trim().is_empty()
        && response.tool_calls.as_ref().is_none_or(|c| c.is_empty())
}

/// Log per-call reasoning usage so effort can be tuned from real data, and
/// warn when reasoning consumed the whole `max_tokens` budget (KYO-972).
pub fn log_reasoning_usage(
    provider: &str,
    model: &str,
    options: &RequestOptions,
    response: &LLMResponse,
) {
    tracing::info!(
        provider,
        model,
        reasoning_effort = ?options.reasoning_effort,
        temperature = ?options.temperature,
        output_tokens = response.usage.output_tokens,
        reasoning_tokens = response.usage.reasoning_tokens,
        max_tokens = options.max_tokens,
        "LLM call reasoning usage"
    );
    if reasoning_exhausted_budget(response) {
        tracing::warn!(
            provider,
            model,
            reasoning_effort = ?options.reasoning_effort,
            reasoning_tokens = response.usage.reasoning_tokens,
            max_tokens = options.max_tokens,
            finish_reason = %response.finish_reason,
            "LLM response is empty and hit max_tokens: reasoning likely consumed the whole token budget; \
             raise max_tokens or lower the reasoning effort"
        );
    }
}

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
        options: &RequestOptions,
        user_names: &HashMap<String, String>,
    ) -> kyomi_core::Result<LLMResponse>;

    /// Return the model identifier (e.g., "claude-haiku-4-5-20251001").
    fn model(&self) -> &str;

    /// Return the model's context window size in tokens (0 = unknown).
    fn context_window(&self) -> u32 { 0 }

    /// The provider's batch API, when it has one (KYO-973). `None` means
    /// batch mode is unsupported for this provider and endpoint.
    fn batch(&self) -> Option<&dyn crate::batch::BatchLLMProvider> {
        None
    }
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
    /// Context window size in tokens (0 = unknown, use hardcoded lookup).
    pub context_window: u32,
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
        options: &RequestOptions,
        user_names: &HashMap<String, String>,
    ) -> kyomi_core::Result<LLMResponse> {
        AnthropicClient::complete(self, messages, tools, options, user_names).await
    }

    fn model(&self) -> &str {
        AnthropicClient::model(self)
    }

    fn context_window(&self) -> u32 {
        AnthropicClient::context_window(self)
    }

    fn batch(&self) -> Option<&dyn crate::batch::BatchLLMProvider> {
        // The adapter exists for any Anthropic client; whether a custom base
        // URL (a proxy) may be used for batches is a configuration decision
        // enforced by `batch::batch_support`.
        Some(self)
    }
}

#[async_trait]
impl LLMProvider for OpenAIProvider {
    async fn complete(
        &self,
        messages: &[Message],
        tools: &[Tool],
        options: &RequestOptions,
        user_names: &HashMap<String, String>,
    ) -> kyomi_core::Result<LLMResponse> {
        OpenAIProvider::complete(self, messages, tools, options, user_names).await
    }

    fn model(&self) -> &str {
        OpenAIProvider::model(self)
    }

    fn context_window(&self) -> u32 {
        self.context_window()
    }

    fn batch(&self) -> Option<&dyn crate::batch::BatchLLMProvider> {
        crate::batch::is_openrouter(self.base_url())
            .then_some(self as &dyn crate::batch::BatchLLMProvider)
    }
}

#[async_trait]
impl LLMProvider for GeminiProvider {
    async fn complete(
        &self,
        messages: &[Message],
        tools: &[Tool],
        options: &RequestOptions,
        user_names: &HashMap<String, String>,
    ) -> kyomi_core::Result<LLMResponse> {
        GeminiProvider::complete(self, messages, tools, options, user_names).await
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
pub(crate) fn create_provider(
    config: LLMProviderConfig,
) -> kyomi_core::Result<Box<dyn LLMProvider>> {
    match config.provider {
        ProviderKind::Anthropic => {
            let mut client = if let Some(base_url) = config.base_url {
                AnthropicClient::with_base_url(config.api_key, config.model, base_url)?
            } else {
                AnthropicClient::new(config.api_key, config.model)?
            };
            client.set_context_window(config.context_window);
            Ok(Box::new(client))
        }
        ProviderKind::OpenAI => {
            let mut client =
                crate::openai::OpenAIProvider::new(config.api_key, config.model, config.base_url)?;
            if config.context_window > 0 {
                client.set_context_window(config.context_window);
            }
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

/// Build the metered [`LLMProvider`] for a workspace AI config. Every response
/// it returns is recorded for billing (see [`crate::usage_ledger`]).
///
/// * **Kyomi mode** (`ws_config.provider == WorkspaceAiProvider::Kyomi`):
///   uses the server's env-configured keys and model (`LLM_PROVIDER`,
///   `LLM_API_KEY`, `LLM_MODEL`, `LLM_BASE_URL`). The workspace's
///   `default_model` is ignored — the server controls the model entirely.
///
/// * **BYOK mode** (anthropic / openai / gemini): uses the workspace's
///   decrypted API key, model, and optional base URL.
///
/// `requested_model` is a model the caller explicitly asks for (the title model for titles,
/// summaries and tags). It is requested as given on every setup, the Kyomi-managed and SaaS
/// ones included, and the usage row records it; `None` keeps the setup's own model. A
/// workspace's stored default model is still ignored on the managed setup (it may name a
/// model from an earlier BYOK configuration), which is why an explicit request is separate.
pub fn create_provider_from_workspace(
    ws_config: &kyomi_auth::workspace_ai_config::WorkspaceAiConfig,
    fallback_config: &kyomi_core::Config,
    requested_model: Option<&str>,
) -> kyomi_core::Result<crate::usage_ledger::MeteredClient> {
    build_provider_from_workspace(ws_config, fallback_config, requested_model)
        .map(crate::usage_ledger::MeteredClient::new)
}

fn build_provider_from_workspace(
    ws_config: &kyomi_auth::workspace_ai_config::WorkspaceAiConfig,
    fallback_config: &kyomi_core::Config,
    requested_model: Option<&str>,
) -> kyomi_core::Result<Box<dyn LLMProvider>> {
    create_provider(resolve_workspace_provider_config_for(ws_config, fallback_config, requested_model)?)
}

/// The provider configuration [`create_provider_from_workspace`] builds the
/// client from. Public so other code that depends on *which* provider and base
/// URL are in effect for a workspace asks the same function instead of
/// re-deriving the rule: whether a provider-executed web backend can run, and
/// watches, which override only the model on the same connection (KYO-973)
/// rather than re-deriving the SaaS / Kyomi / BYOK connection rules.
pub fn resolve_workspace_provider_config(
    ws_config: &kyomi_auth::workspace_ai_config::WorkspaceAiConfig,
    fallback_config: &kyomi_core::Config,
) -> kyomi_core::Result<LLMProviderConfig> {
    resolve_workspace_provider_config_for(ws_config, fallback_config, None)
}

/// [`resolve_workspace_provider_config`] with an explicitly requested model, as
/// [`create_provider_from_workspace`] documents.
fn resolve_workspace_provider_config_for(
    ws_config: &kyomi_auth::workspace_ai_config::WorkspaceAiConfig,
    fallback_config: &kyomi_core::Config,
    requested_model: Option<&str>,
) -> kyomi_core::Result<LLMProviderConfig> {
    use kyomi_auth::workspace_ai_config::WorkspaceAiProvider;

    // SaaS mode: always use Kyomi-managed credentials and model.
    // Workspace-level BYOK configs and default_model are preserved in the
    // database but ignored at runtime — the server controls everything.
    if !fallback_config.self_hosted {
        let mut config = resolve_provider_config(fallback_config)?;
        if let Some(model) = requested_model {
            config.model = Some(model.to_string());
        }
        return Ok(config);
    }

    match ws_config.provider {
        WorkspaceAiProvider::Kyomi => {
            // Server-side keys: server controls the model entirely.
            // Workspace default_model is ignored — it may reference a
            // BYOK-only model from a previous provider configuration.
            let mut config = resolve_provider_config(fallback_config)?;
            if let Some(model) = requested_model {
                config.model = Some(model.to_string());
            }
            Ok(config)
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
            Ok(LLMProviderConfig {
                provider,
                api_key,
                model: requested_model.map(str::to_string).or_else(|| ws_config.model.clone()),
                base_url: ws_config.base_url.clone(),
                context_window: ws_config.context_window,
            })
        }
    }
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
            context_window: config.llm_context_window,
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
            context_window: config.llm_context_window,
        });
    }

    Err(kyomi_core::Error::Internal(
        "no LLM provider configured: set LLM_PROVIDER + LLM_API_KEY, \
         or ANTHROPIC_API_KEY"
            .into(),
    ))
}

// ---------------------------------------------------------------------------
// Shared formatting helpers
// ---------------------------------------------------------------------------

/// Format a user message with sender attribution.
///
/// If the message has a `user_id` and a matching name is found in `user_names`,
/// formats as `[Name (last8chars)]: content`. Otherwise formats as
/// `[User (last8chars)]: content`.
pub(crate) fn format_user_message(msg: &Message, user_names: &HashMap<String, String>) -> String {
    let Some(user_id) = &msg.user_id else {
        return msg.content.clone();
    };

    let id_short = if user_id.len() >= 8 {
        &user_id[user_id.len() - 8..]
    } else {
        user_id.as_str()
    };

    if let Some(name) = user_names.get(user_id.as_str()) {
        format!("[{name} ({id_short})]: {}", msg.content)
    } else {
        format!("[User ({id_short})]: {}", msg.content)
    }
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
    fn create_provider_openai_succeeds() {
        let config = LLMProviderConfig {
            provider: ProviderKind::OpenAI,
            api_key: "sk-test".into(),
            model: None,
            base_url: None,
            context_window: 0,
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
            context_window: 0,
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
            context_window: 0,
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
            context_window: 0,
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
            title_model: None,
            context_window: 0,
            sampling: Default::default(),
            sampling_warnings: Vec::new(),
            watch_llm: Default::default(),
            watch_llm_warnings: Vec::new(),
        };

        let provider = create_provider_from_workspace(&ws, &fallback, None)
            .expect("Kyomi-mode dispatch should succeed with server env key");
        // Server default model flows through.
        assert_eq!(provider.model(), "claude-sonnet-4-20250514");
    }

    /// An explicitly requested model (the title model) is honoured on the Kyomi-managed setups
    /// (SaaS and self-hosted Kyomi mode); with none requested the setup's own model is kept, and a
    /// workspace's stored default model is still ignored (KYO-974).
    #[test]
    fn workspace_factory_honours_an_explicitly_requested_model_on_managed_setups() {
        use kyomi_auth::workspace_ai_config::{WorkspaceAiConfig, WorkspaceAiProvider};
        let ws = WorkspaceAiConfig {
            provider: WorkspaceAiProvider::Kyomi,
            model: Some("claude-opus-4-5".into()),
            api_key: None,
            base_url: None,
            title_model: None,
            context_window: 0,
            sampling: Default::default(),
            sampling_warnings: Vec::new(),
            watch_llm: Default::default(),
            watch_llm_warnings: Vec::new(),
        };
        for self_hosted in [false, true] {
            let mut fallback = kyomi_core::Config::test_config();
            fallback.self_hosted = self_hosted;
            fallback.anthropic_api_key = Some("sk-ant-server".into());
            fallback.llm_model = Some("claude-sonnet-5-5".into());
            let chat = create_provider_from_workspace(&ws, &fallback, None).unwrap();
            assert_eq!(chat.model(), "claude-sonnet-5-5", "self_hosted={self_hosted}: setup model, stored default ignored");
            let title = create_provider_from_workspace(&ws, &fallback, Some("claude-haiku-5-5")).unwrap();
            assert_eq!(title.model(), "claude-haiku-5-5", "self_hosted={self_hosted}: requested model wins");
        }
    }

    #[test]
    fn workspace_factory_kyomi_mode_ignores_workspace_model() {
        use kyomi_auth::workspace_ai_config::{WorkspaceAiConfig, WorkspaceAiProvider};

        let mut fallback = kyomi_core::Config::test_config();
        fallback.anthropic_api_key = Some("sk-ant-server".into());
        fallback.llm_model = Some("claude-sonnet-4-20250514".into());

        let ws = WorkspaceAiConfig {
            provider: WorkspaceAiProvider::Kyomi,
            model: Some("claude-haiku-4-5-20251001".into()),
            api_key: None,
            base_url: None,
            title_model: None,
            context_window: 0,
            sampling: Default::default(),
            sampling_warnings: Vec::new(),
            watch_llm: Default::default(),
            watch_llm_warnings: Vec::new(),
        };

        let provider = create_provider_from_workspace(&ws, &fallback, None).unwrap();
        // Kyomi mode: server controls the model, workspace model is ignored.
        assert_eq!(provider.model(), "claude-sonnet-4-20250514");
    }

    #[test]
    fn workspace_factory_byok_openai_uses_workspace_key() {
        use kyomi_auth::workspace_ai_config::{WorkspaceAiConfig, WorkspaceAiProvider};

        // Deliberately empty fallback — BYOK must not read from it.
        // self_hosted = true: BYOK is only available in self-hosted mode.
        let mut fallback = kyomi_core::Config::test_config();
        fallback.self_hosted = true;
        fallback.llm_provider = None;
        fallback.llm_api_key = None;
        fallback.anthropic_api_key = None;

        let ws = WorkspaceAiConfig {
            provider: WorkspaceAiProvider::OpenAI,
            model: Some("gpt-4o".into()),
            api_key: Some("sk-ws-byok".into()),
            base_url: None,
            title_model: None,
            context_window: 0,
            sampling: Default::default(),
            sampling_warnings: Vec::new(),
            watch_llm: Default::default(),
            watch_llm_warnings: Vec::new(),
        };

        let provider = create_provider_from_workspace(&ws, &fallback, None)
            .expect("BYOK OpenAI dispatch should not need fallback keys");
        assert_eq!(provider.model(), "gpt-4o");
    }

    #[test]
    fn workspace_factory_byok_gemini_default_model() {
        use kyomi_auth::workspace_ai_config::{WorkspaceAiConfig, WorkspaceAiProvider};

        // self_hosted = true: BYOK is only available in self-hosted mode.
        let mut fallback = kyomi_core::Config::test_config();
        fallback.self_hosted = true;
        let ws = WorkspaceAiConfig {
            provider: WorkspaceAiProvider::Gemini,
            model: None,
            api_key: Some("AIza-test".into()),
            base_url: None,
            title_model: None,
            context_window: 0,
            sampling: Default::default(),
            sampling_warnings: Vec::new(),
            watch_llm: Default::default(),
            watch_llm_warnings: Vec::new(),
        };

        let provider = create_provider_from_workspace(&ws, &fallback, None).unwrap();
        assert_eq!(provider.model(), crate::gemini::DEFAULT_MODEL);
    }

    #[test]
    fn workspace_factory_byok_without_key_errors() {
        use kyomi_auth::workspace_ai_config::{WorkspaceAiConfig, WorkspaceAiProvider};

        // self_hosted = true: BYOK is only available in self-hosted mode.
        let mut fallback = kyomi_core::Config::test_config();
        fallback.self_hosted = true;
        let ws = WorkspaceAiConfig {
            provider: WorkspaceAiProvider::Anthropic,
            model: None,
            api_key: None,
            base_url: None,
            title_model: None,
            context_window: 0,
            sampling: Default::default(),
            sampling_warnings: Vec::new(),
            watch_llm: Default::default(),
            watch_llm_warnings: Vec::new(),
        };

        let err = match create_provider_from_workspace(&ws, &fallback, None) {
            Ok(_) => panic!("expected BYOK-without-key to error"),
            Err(e) => e,
        };
        assert!(
            err.to_string().contains("no stored API key"),
            "expected missing-key error, got: {err}"
        );
    }

    #[test]
    fn workspace_factory_saas_mode_forces_kyomi_managed() {
        use kyomi_auth::workspace_ai_config::{WorkspaceAiConfig, WorkspaceAiProvider};

        // SaaS mode: self_hosted = false (default in test_config)
        let mut fallback = kyomi_core::Config::test_config();
        fallback.self_hosted = false;
        fallback.anthropic_api_key = Some("sk-ant-server".into());
        fallback.llm_model = Some("claude-sonnet-4-20250514".into());

        // Workspace has BYOK config (OpenAI with its own key)
        let ws = WorkspaceAiConfig {
            provider: WorkspaceAiProvider::OpenAI,
            model: Some("gpt-4o".into()),
            api_key: Some("sk-ws-byok-key".into()),
            base_url: None,
            title_model: None,
            context_window: 0,
            sampling: Default::default(),
            sampling_warnings: Vec::new(),
            watch_llm: Default::default(),
            watch_llm_warnings: Vec::new(),
        };

        // SaaS mode should ignore BYOK and use server config
        let provider = create_provider_from_workspace(&ws, &fallback, None)
            .expect("SaaS mode should use server-managed provider");
        // Server config resolves to Anthropic with claude-sonnet-4-20250514
        assert_eq!(provider.model(), "claude-sonnet-4-20250514");
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

    // -- User attribution formatting -----------------------------------------

    #[test]
    fn format_user_message_no_user_id() {
        let msg = Message::user("hello");
        let names = HashMap::new();
        let result = format_user_message(&msg, &names);
        assert_eq!(result, "hello");
    }

    #[test]
    fn format_user_message_with_known_user() {
        let msg = Message::user_with_id("hello", "user-abcd-1234-efgh");
        let mut names = HashMap::new();
        names.insert("user-abcd-1234-efgh".to_string(), "Jason Adams".to_string());
        let result = format_user_message(&msg, &names);
        assert_eq!(result, "[Jason Adams (234-efgh)]: hello");
    }

    #[test]
    fn format_user_message_with_unknown_user() {
        let msg = Message::user_with_id("hello", "user-abcd-1234-efgh");
        let names = HashMap::new();
        let result = format_user_message(&msg, &names);
        assert_eq!(result, "[User (234-efgh)]: hello");
    }

    #[test]
    fn format_user_message_short_user_id() {
        let msg = Message::user_with_id("hello", "abc");
        let names = HashMap::new();
        let result = format_user_message(&msg, &names);
        assert_eq!(result, "[User (abc)]: hello");
    }
}

#[cfg(test)]
mod reasoning_usage_tests {
    use super::*;
    use crate::types::AgentTokenUsage;

    fn response(content: &str, finish: &str, tool_calls: Option<Vec<crate::types::ToolCall>>) -> LLMResponse {
        LLMResponse {
            raw_response: serde_json::Value::Null,
            content: content.into(),
            finish_reason: finish.into(),
            usage: AgentTokenUsage::default(),
            tool_calls,
            cost: None,
            thinking_content: None,
            call_detail: None,
        }
    }

    #[test]
    fn empty_length_stop_means_reasoning_ate_the_budget() {
        assert!(reasoning_exhausted_budget(&response("", "length", None)));
        assert!(reasoning_exhausted_budget(&response("  ", "max_tokens", None)));
    }

    #[test]
    fn normal_or_partial_answers_are_not_flagged() {
        assert!(!reasoning_exhausted_budget(&response("", "stop", None)));
        assert!(!reasoning_exhausted_budget(&response("partial", "length", None)));
        assert!(!reasoning_exhausted_budget(&response("", "end_turn", None)));
    }

}
