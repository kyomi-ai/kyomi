// SPDX-License-Identifier: AGPL-3.0-or-later

//! Workspace AI configuration server functions (Workspace BYOK).
//!
//! Backs the Settings → AI page. Reads and writes the per-workspace provider
//! choice (`kyomi` | `anthropic` | `openai` | `gemini`), optional API key
//! (AES-GCM encrypted at rest via [`kyomi_auth::workspace_secrets`]), optional
//! base URL, and default model.
//!
//! Semantics:
//!
//! * [`get_workspace_ai_config`] — any workspace member may read. Never
//!   returns the plaintext API key; callers get `has_api_key: bool` instead.
//! * [`update_workspace_ai_config`] — admin/owner only. Passing `api_key =
//!   None` for a BYOK provider preserves the existing encrypted key (used for
//!   model-only updates).
//! * [`test_workspace_ai_config`] — admin/owner only. Performs a free
//!   auth-check GET against the upstream provider using the candidate
//!   credentials. Never writes to the DB and never logs the key. The 10s
//!   timeout prevents a slow upstream from tying up a server thread.
//! * [`get_resolved_ai_provider`] — admin/owner only. Reports what is
//!   *actually* resolved and in effect right now — the workspace's stored
//!   BYOK credentials when configured, otherwise whatever the server's env
//!   vars resolve to — rather than just naming the relevant env vars. Never
//!   writes to the DB and never returns the plaintext API key.
//!
//! **SaaS vs self-hosted gating**:
//!
//! * [`get_workspace_ai_config`] — SaaS only (self-hosted uses env config).
//! * [`update_workspace_ai_config`] — SaaS only; rejects non-Kyomi providers
//!   (BYOK is disabled in SaaS mode).
//! * [`test_workspace_ai_config`] — self-hosted only (no BYOK in SaaS).
//! * [`list_workspace_ai_models`] — self-hosted only (no BYOK in SaaS).
//! * [`get_resolved_ai_provider`] — self-hosted only. SaaS always uses
//!   Kyomi-managed AI, so there is no per-tenant resolution to report.

use leptos::prelude::*;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Sanitised view of a workspace's AI configuration.
///
/// Never contains the plaintext API key — callers get `has_api_key` to drive
/// "Key saved ✓" UI instead.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WorkspaceAiConfigView {
    /// `"kyomi" | "anthropic" | "openai" | "gemini"`.
    pub provider: String,
    /// Workspace default model name (from `settings.custom_settings.default_model`).
    pub model: Option<String>,
    /// `true` iff the workspace is in BYOK mode AND has a stored encrypted
    /// key. Always `false` in Kyomi mode.
    pub has_api_key: bool,
    /// Optional base URL override. Only set in BYOK mode.
    pub base_url: Option<String>,
    /// Remaining balance in the workspace's AI token bundle, in USD
    /// (`ai_bundle_balance_usd - ai_credits_used_usd`, clamped to zero).
    ///
    /// `None` when the balance could not be read (e.g. self-hosted mode, where
    /// this server fn is rejected before reaching this view). Any authenticated
    /// workspace member may see this value — same visibility as the billing
    /// page's balance display.
    pub ai_bundle_balance_usd: Option<f64>,
    /// Optional model used specifically for session title generation
    /// (from `settings.custom_settings.title_model`).
    ///
    /// When `None`, title generation falls back to the cheapest model for the
    /// configured provider. When `Some`, that model is used verbatim.
    #[serde(default)]
    pub title_model: Option<String>,
}

/// One model entry returned by model-listing functions.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct AiModelInfo {
    pub id: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_cost_per_token: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completion_cost_per_token: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_length: Option<u64>,
    /// Reasoning-effort values the model accepts, when the listing source
    /// reports them (OpenRouter `reasoning.supported_efforts`). Used only to
    /// populate suggestions; the effort setting itself is free text.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub supported_efforts: Vec<String>,
    /// The model's own default effort (OpenRouter `reasoning.default_effort`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_effort: Option<String>,
}

/// The six LLM tuning fields as the user typed them (KYO-972). An empty
/// string means "not set here" (fall through to the next layer); anything
/// else is a value sent exactly as written.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct LlmTuningValues {
    pub reasoning_effort: String,
    pub title_reasoning_effort: String,
    pub watch_reasoning_effort: String,
    pub temperature: String,
    pub title_temperature: String,
    pub watch_temperature: String,
}

/// Self-hosted LLM tuning as stored for the workspace and as set by the
/// instance (`LLM_*` env), so the page can show where a value comes from.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct LlmTuningView {
    /// Resolved provider (`anthropic | openai | gemini`), for suggestions.
    pub provider: String,
    pub workspace: LlmTuningValues,
    pub instance: LlmTuningValues,
    /// Stored workspace values that are invalid and therefore ignored, as
    /// `key: reason`. Shown so an admin can fix or clear them.
    #[serde(default)]
    pub warnings: Vec<String>,
}

/// Watch LLM override fields as text, for the Settings form (KYO-973).
/// Empty means "not set" (the instance value or the main LLM applies).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WatchLlmValues {
    pub provider: String,
    pub model: String,
    pub base_url: String,
    /// `realtime | batch`, or empty.
    pub mode: String,
    pub batch_window_minutes: String,
    /// `realtime_retry | error`, or empty.
    pub batch_failure: String,
    /// Write-only: a new API key to store. Never returned by the server.
    #[serde(default)]
    pub new_api_key: String,
    /// Write-only: remove the stored API key.
    #[serde(default)]
    pub clear_api_key: bool,
}

/// The watch LLM override as stored for the workspace and as set by the
/// instance (`LLM_WATCH_*` env), so the page can show where a value comes from.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WatchLlmView {
    pub workspace: WatchLlmValues,
    pub instance: WatchLlmValues,
    pub has_workspace_api_key: bool,
    pub has_instance_api_key: bool,
    /// Stored workspace values that are invalid and therefore ignored.
    #[serde(default)]
    pub warnings: Vec<String>,
}

/// Result of a candidate-config connectivity test.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TestAiConfigResult {
    /// `true` on any 2xx response from the upstream auth-check endpoint.
    pub ok: bool,
    /// Short human-readable message. `"Connection OK"` on success; a
    /// sanitised error summary on failure. **Never contains the API key.**
    pub message: String,
}

/// What AI provider/model configuration is actually resolved and in effect
/// for a self-hosted workspace right now — as opposed to just naming the
/// relevant environment variables.
///
/// **Never contains the plaintext API key, a prefix/suffix of it, its
/// length, or any other derivative** — `has_api_key: bool` is the absolute
/// maximum exposed. See [`resolve_effective_ai_provider`] for how this is
/// computed and why BYOK and env resolution can't both be shown at once.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ResolvedAiProviderView {
    /// A provider resolved successfully and is in effect.
    Resolved {
        /// `"anthropic" | "openai" | "gemini"`.
        provider: String,
        /// `Some(m)` when a model was explicitly configured (workspace BYOK
        /// model, or `LLM_MODEL` in Kyomi/env mode). `None` means the
        /// provider's own built-in default model is in effect.
        model: Option<String>,
        /// Custom API base URL override, when one is in effect.
        base_url: Option<String>,
        /// Whether a non-empty API key backs this resolution.
        has_api_key: bool,
        /// Effective title-generation model (workspace `title_model`
        /// setting, falling back to `LLM_TITLE_MODEL`), when either is set.
        /// `None` means title generation uses the cheapest model for the
        /// resolved provider.
        title_model: Option<String>,
    },
    /// No provider could be resolved. `reason` is the same message
    /// [`kyomi_agent::resolve_provider_config`] or
    /// [`kyomi_agent::create_provider_from_workspace`] would raise at
    /// request time — safe to show verbatim, never contains key material.
    Unconfigured { reason: String },
}

// ---------------------------------------------------------------------------
// LLM tuning (reasoning effort / temperature) — self-hosted, KYO-972
// ---------------------------------------------------------------------------

#[cfg(feature = "ssr")]
fn tuning_values(s: &kyomi_core::llm_sampling::LlmSamplingSettings) -> LlmTuningValues {
    use kyomi_core::llm_sampling::{effort_text, temperature_text};
    LlmTuningValues {
        reasoning_effort: effort_text(s.reasoning_effort.as_ref()),
        title_reasoning_effort: effort_text(s.title_reasoning_effort.as_ref()),
        watch_reasoning_effort: effort_text(s.watch_reasoning_effort.as_ref()),
        temperature: temperature_text(s.temperature.as_ref()),
        title_temperature: temperature_text(s.title_temperature.as_ref()),
        watch_temperature: temperature_text(s.watch_temperature.as_ref()),
    }
}

/// Read the workspace and instance LLM tuning. Self-hosted admins only.
#[server(prefix = "/leptos-api", client = crate::server_fns::paywall_client::PaywallAwareClient)]
pub async fn get_llm_tuning() -> Result<LlmTuningView, ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;
    if !ac.ctx.config.self_hosted {
        return Err(ServerFnError::new(
            "LLM tuning is only configurable on self-hosted deployments.",
        ));
    }
    ac.require(Permission::ManageAiConfig, "Workspace admin access required")?;

    let ws_config = kyomi_auth::workspace_ai_config::load(ac.db(), &ac.ws_id)
        .await
        .into_sfn()?;
    let provider = match resolve_effective_ai_provider(&ws_config, &ac.ctx.config) {
        ResolvedAiProviderView::Resolved { provider, .. } => provider,
        ResolvedAiProviderView::Unconfigured { .. } => String::new(),
    };
    Ok(LlmTuningView {
        provider,
        workspace: tuning_values(&ws_config.sampling),
        instance: tuning_values(&ac.ctx.config.llm_sampling),
        warnings: ws_config.sampling_warnings.clone(),
    })
}

/// Validate and store the workspace LLM tuning (reasoning effort and
/// temperature per call profile). Self-hosted admins only.
///
/// Each empty field clears its key so the instance `LLM_*` value or the
/// built-in default applies again. Every non-empty value is validated with
/// the same parsers the env and the stored-settings reader use, and the whole
/// request is rejected if any field is invalid (nothing is partially saved).
/// The write is an atomic edit of `settings` (see
/// `workspace_service::update_workspace_settings_with`), so it cannot clobber
/// a concurrent change to another settings key.
#[server(prefix = "/leptos-api", client = crate::server_fns::paywall_client::PaywallAwareClient)]
pub async fn update_llm_tuning(values: LlmTuningValues) -> Result<(), ServerFnError> {
    use kyomi_core::llm_sampling::{
        stored_effort, stored_temperature, WORKSPACE_EFFORT_KEYS, WORKSPACE_TEMPERATURE_KEYS,
    };

    let ac = AuthenticatedContext::extract().await?;
    if !ac.ctx.config.self_hosted {
        return Err(ServerFnError::new(
            "LLM tuning is only configurable on self-hosted deployments.",
        ));
    }
    ac.require(Permission::ManageAiConfig, "Workspace admin access required")?;

    // Validate everything first; `None` clears the key.
    let [e0, e1, e2] = [&values.reasoning_effort, &values.title_reasoning_effort, &values.watch_reasoning_effort];
    let [t0, t1, t2] = [&values.temperature, &values.title_temperature, &values.watch_temperature];
    let mut edits: Vec<(&'static str, Option<serde_json::Value>)> = Vec::new();
    for (key, raw) in WORKSPACE_EFFORT_KEYS.into_iter().zip([e0, e1, e2]) {
        let raw = raw.trim();
        let stored = if raw.is_empty() {
            None
        } else {
            Some(stored_effort(raw).map_err(|e| ServerFnError::new(format!("{key}: {e}")))?)
        };
        edits.push((key, stored));
    }
    for (key, raw) in WORKSPACE_TEMPERATURE_KEYS.into_iter().zip([t0, t1, t2]) {
        let raw = raw.trim();
        let stored = if raw.is_empty() {
            None
        } else {
            Some(stored_temperature(raw))
        };
        edits.push((key, stored));
    }

    super::workspace::edit_workspace_settings(&ac, |current| {
        let mut settings = current.clone();
        for (key, stored) in &edits {
            settings = Some(match stored {
                Some(v) => super::workspace::merge_custom_settings(&settings, key, v.clone()),
                None => super::workspace::clear_custom_settings_key(&settings, key),
            });
        }
        settings.unwrap_or_else(|| serde_json::json!({}))
    })
    .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Conversation compaction (KYO-975) — self-hosted
// ---------------------------------------------------------------------------

/// The two compaction fields as the user typed them. Empty means "not set at
/// this layer" and falls through to the instance value or the default.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CompactionValues {
    pub fraction: String,
    pub threshold: String,
}

/// Workspace and instance compaction settings, plus invalid stored values that
/// are being ignored so an admin can fix them.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CompactionSettingsView {
    pub workspace: CompactionValues,
    pub instance: CompactionValues,
    #[serde(default)]
    pub warnings: Vec<String>,
}

#[cfg(feature = "ssr")]
fn compaction_values(
    s: &kyomi_core::llm_compaction::LlmCompactionSettings,
) -> CompactionValues {
    use kyomi_core::llm_compaction::{fraction_text, threshold_text};
    CompactionValues {
        fraction: fraction_text(s.fraction),
        threshold: threshold_text(s.threshold_tokens),
    }
}

/// Read the workspace and instance conversation-compaction settings.
/// Self-hosted admins only.
#[server(prefix = "/leptos-api", client = crate::server_fns::paywall_client::PaywallAwareClient)]
pub async fn get_compaction_settings() -> Result<CompactionSettingsView, ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;
    if !ac.ctx.config.self_hosted {
        return Err(ServerFnError::new(
            "Conversation compaction is only configurable on self-hosted deployments.",
        ));
    }
    ac.require(Permission::ManageAiConfig, "Workspace admin access required")?;

    let ws_config = kyomi_auth::workspace_ai_config::load(ac.db(), &ac.ws_id)
        .await
        .into_sfn()?;
    Ok(CompactionSettingsView {
        workspace: compaction_values(&ws_config.compaction),
        instance: compaction_values(&ac.ctx.config.llm_compaction),
        warnings: ws_config.compaction_warnings.clone(),
    })
}

/// Validate and store the workspace conversation-compaction settings.
/// Self-hosted admins only.
///
/// An empty field clears its key so the instance value or the default applies.
/// Every non-empty value is validated with the same parsers the env and the
/// stored-settings reader use, and the whole request is rejected if either is
/// invalid (nothing is partially saved).
#[server(prefix = "/leptos-api", client = crate::server_fns::paywall_client::PaywallAwareClient)]
pub async fn update_compaction_settings(values: CompactionValues) -> Result<(), ServerFnError> {
    use kyomi_core::llm_compaction::{
        stored_fraction, stored_threshold, WORKSPACE_FRACTION_KEY, WORKSPACE_THRESHOLD_KEY,
    };

    let ac = AuthenticatedContext::extract().await?;
    if !ac.ctx.config.self_hosted {
        return Err(ServerFnError::new(
            "Conversation compaction is only configurable on self-hosted deployments.",
        ));
    }
    ac.require(Permission::ManageAiConfig, "Workspace admin access required")?;

    let fraction = values.fraction.trim();
    let fraction = if fraction.is_empty() {
        None
    } else {
        Some(stored_fraction(fraction).map_err(|e| ServerFnError::new(format!("fraction: {e}")))?)
    };
    let threshold = values.threshold.trim();
    let threshold = if threshold.is_empty() {
        None
    } else {
        Some(stored_threshold(threshold).map_err(|e| ServerFnError::new(format!("threshold: {e}")))?)
    };

    super::workspace::edit_workspace_settings(&ac, |current| {
        let mut settings = current.clone();
        settings = Some(match &fraction {
            Some(v) => super::workspace::merge_custom_settings(&settings, WORKSPACE_FRACTION_KEY, v.clone()),
            None => super::workspace::clear_custom_settings_key(&settings, WORKSPACE_FRACTION_KEY),
        });
        settings = Some(match &threshold {
            Some(v) => super::workspace::merge_custom_settings(&settings, WORKSPACE_THRESHOLD_KEY, v.clone()),
            None => super::workspace::clear_custom_settings_key(&settings, WORKSPACE_THRESHOLD_KEY),
        });
        settings.unwrap_or_else(|| serde_json::json!({}))
    })
    .await?;
    Ok(())
}

#[cfg(feature = "ssr")]
fn watch_llm_values(s: &kyomi_core::watch_llm::WatchLlmSettings) -> WatchLlmValues {
    WatchLlmValues {
        provider: s.provider.clone().unwrap_or_default(),
        model: s.model.clone().unwrap_or_default(),
        base_url: s.base_url.clone().unwrap_or_default(),
        mode: s.mode.map(|m| m.as_str().to_string()).unwrap_or_default(),
        batch_window_minutes: s.batch_window_minutes.map(|n| n.to_string()).unwrap_or_default(),
        batch_failure: s.batch_failure.map(|p| p.as_str().to_string()).unwrap_or_default(),
        new_api_key: String::new(),
        clear_api_key: false,
    }
}

/// Read the workspace and instance watch LLM configuration. Self-hosted
/// admins only. API keys are never returned, only whether one is stored.
#[server(prefix = "/leptos-api", client = crate::server_fns::paywall_client::PaywallAwareClient)]
pub async fn get_watch_llm() -> Result<WatchLlmView, ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;
    if !ac.ctx.config.self_hosted {
        return Err(ServerFnError::new(
            "Watch LLM settings are only configurable on self-hosted deployments.",
        ));
    }
    ac.require(Permission::ManageAiConfig, "Workspace admin access required")?;

    let ws_config = kyomi_auth::workspace_ai_config::load(ac.db(), &ac.ws_id)
        .await
        .into_sfn()?;
    Ok(WatchLlmView {
        workspace: watch_llm_values(&ws_config.watch_llm),
        instance: watch_llm_values(&ac.ctx.config.llm_watch),
        has_workspace_api_key: ws_config.watch_llm.api_key.is_some(),
        has_instance_api_key: ac.ctx.config.llm_watch.api_key.is_some(),
        warnings: ws_config.watch_llm_warnings.clone(),
    })
}

/// Validate and store the workspace watch LLM override. Self-hosted admins
/// only.
///
/// Empty fields clear their key so the instance `LLM_WATCH_*` value or the
/// main LLM applies again. The whole request is rejected, and nothing saved,
/// when any value is invalid, when a provider is chosen without an API key,
/// or when `mode = batch` resolves to a provider without a batch API.
#[server(prefix = "/leptos-api", client = crate::server_fns::paywall_client::PaywallAwareClient)]
pub async fn update_watch_llm(values: WatchLlmValues) -> Result<(), ServerFnError> {
    use kyomi_auth::workspace_ai_config::WatchKeyChange;
    use kyomi_core::watch_llm::{
        parse_batch_window_minutes, BatchFailurePolicy, WatchLlmMode, WatchLlmSettings,
    };

    let ac = AuthenticatedContext::extract().await?;
    if !ac.ctx.config.self_hosted {
        return Err(ServerFnError::new(
            "Watch LLM settings are only configurable on self-hosted deployments.",
        ));
    }
    ac.require(Permission::ManageAiConfig, "Workspace admin access required")?;

    let text = |v: &str| Some(v.trim().to_string()).filter(|v| !v.is_empty());
    let bad = |field: &str, e: kyomi_core::llm_sampling::InvalidSetting| {
        ServerFnError::new(format!("{field}: {e}"))
    };
    let provider = text(&values.provider);
    if let Some(p) = &provider {
        kyomi_agent::ProviderKind::parse(p).map_err(|e| ServerFnError::new(e.to_string()))?;
    }
    let settings = WatchLlmSettings {
        provider,
        model: text(&values.model),
        base_url: text(&values.base_url),
        api_key: None,
        mode: text(&values.mode).map(|m| WatchLlmMode::parse(&m)).transpose().map_err(|e| bad("mode", e))?,
        batch_window_minutes: text(&values.batch_window_minutes)
            .map(|w| parse_batch_window_minutes(&w))
            .transpose()
            .map_err(|e| bad("batch window", e))?,
        batch_failure: text(&values.batch_failure)
            .map(|p| BatchFailurePolicy::parse(&p))
            .transpose()
            .map_err(|e| bad("batch failure policy", e))?,
    };

    let key = if values.clear_api_key {
        WatchKeyChange::Clear
    } else if let Some(k) = text(&values.new_api_key) {
        WatchKeyChange::Set(k)
    } else {
        WatchKeyChange::Keep
    };
    let will_have_key = match &key {
        WatchKeyChange::Set(_) => true,
        WatchKeyChange::Clear => false,
        WatchKeyChange::Keep => kyomi_auth::workspace_ai_config::has_watch_api_key(ac.db(), &ac.ws_id)
            .await
            .into_sfn()?,
    };

    // Validate the configuration that would result, exactly as a run resolves it.
    let mut candidate = kyomi_auth::workspace_ai_config::load(ac.db(), &ac.ws_id)
        .await
        .into_sfn()?;
    // Only the *presence* of a key matters to validation; the stored value is
    // never loaded here.
    const KEY_PRESENT_FOR_VALIDATION: &str = "<key present>";
    candidate.watch_llm = WatchLlmSettings {
        api_key: will_have_key.then(|| KEY_PRESENT_FOR_VALIDATION.to_string()),
        ..settings.clone()
    };
    candidate.watch_llm.validate().map_err(|e| bad("watch LLM", e))?;
    kyomi_agent::watch_llm::validate_selection(&candidate, &ac.ctx.config)
        .map_err(ServerFnError::new)?;

    kyomi_auth::workspace_ai_config::update_watch_llm(ac.db(), &ac.ws_id, &settings, &key)
        .await
        .into_sfn()?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers (server-only)
// ---------------------------------------------------------------------------

/// Reject self-hosted deployments. Workspace AI config management is a SaaS UI surface.
#[cfg(feature = "ssr")]
fn require_saas(ctx: &super::ServerContext) -> Result<(), ServerFnError> {
    if ctx.config.self_hosted {
        return Err(ServerFnError::new(
            "Workspace BYOK is only available in SaaS mode.",
        ));
    }
    Ok(())
}

/// Build a [`WorkspaceAiConfigView`] from the authoritative config record.
#[cfg(feature = "ssr")]
fn view_from_config(
    cfg: &kyomi_auth::workspace_ai_config::WorkspaceAiConfig,
    ai_bundle_balance_usd: Option<f64>,
    title_model: Option<String>,
) -> WorkspaceAiConfigView {
    WorkspaceAiConfigView {
        provider: cfg.provider.as_str().to_string(),
        model: cfg.model.clone(),
        has_api_key: cfg.is_byok() && cfg.api_key.is_some(),
        base_url: cfg.base_url.clone(),
        ai_bundle_balance_usd,
        title_model,
    }
}

/// Read the live remaining AI token bundle balance for a workspace, in USD.
///
/// Delegates to
/// [`kyomi_auth::billing_service::BillingService::get_bundle_remaining_usd`]
/// which computes the value from authoritative live usage records rather than
/// the stale `ai_credits_used_usd` cache column. Returns `Ok(None)` when the
/// billing service errors (e.g. workspace missing) so the UI omits the balance
/// clause rather than surfacing "$0.00".
#[cfg(feature = "ssr")]
async fn load_ai_bundle_remaining_usd(
    db: &kyomi_core::DbPool,
    ws_id: &str,
) -> Result<Option<f64>, ServerFnError> {
    let service = kyomi_auth::billing_service::BillingService::new();
    Ok(service.get_bundle_remaining_usd(db, ws_id).await.ok())
}

// ---------------------------------------------------------------------------
// get
// ---------------------------------------------------------------------------

/// Read the current workspace AI configuration. Any authenticated workspace
/// member may call this.
#[server(prefix = "/leptos-api", client = crate::server_fns::paywall_client::PaywallAwareClient)]
pub async fn get_workspace_ai_config() -> Result<WorkspaceAiConfigView, ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;
    require_saas(&ac.ctx)?;

    let cfg = kyomi_auth::workspace_ai_config::load(ac.db(), &ac.ws_id)
        .await
        .into_sfn()?;
    let balance = load_ai_bundle_remaining_usd(ac.db(), &ac.ws_id).await?;

    Ok(view_from_config(&cfg, balance, cfg.title_model.clone()))
}

// ---------------------------------------------------------------------------
// update
// ---------------------------------------------------------------------------

/// Update the workspace AI configuration. Admin/owner only.
///
/// * `provider` — one of `"kyomi" | "anthropic" | "openai" | "gemini"`.
/// * `api_key` — `None` means "don't change the stored key"; `Some(k)` means
///   encrypt and store `k`. Ignored when `provider == "kyomi"` (the stored
///   key is always cleared in that case).
/// * `base_url` — optional base URL override for BYOK providers.
/// * `model` — optional default model name.
#[server(prefix = "/leptos-api", client = crate::server_fns::paywall_client::PaywallAwareClient)]
pub async fn update_workspace_ai_config(
    provider: String,
    api_key: Option<String>,
    base_url: Option<String>,
    model: Option<String>,
) -> Result<WorkspaceAiConfigView, ServerFnError> {
    use std::str::FromStr;

    let ac = AuthenticatedContext::extract().await?;
    require_saas(&ac.ctx)?;
    ac.require(Permission::ManageAiConfig, "Workspace admin access required")?;

    let parsed_provider =
        kyomi_auth::workspace_ai_config::WorkspaceAiProvider::from_str(&provider)
            .into_sfn()?;

    // SaaS mode: only Kyomi-managed provider is allowed — BYOK is disabled.
    if !ac.ctx.config.self_hosted
        && parsed_provider != kyomi_auth::workspace_ai_config::WorkspaceAiProvider::Kyomi
    {
        return Err(ServerFnError::new(
            "Bring-your-own-key is not available in SaaS mode. AI is included in your plan.",
        ));
    }

    let input = kyomi_auth::workspace_ai_config::UpdateWorkspaceAiConfigInput {
        provider: parsed_provider,
        api_key,
        base_url,
        model,
    };

    kyomi_auth::workspace_ai_config::update(ac.db(), &ac.ws_id, input)
        .await
        .into_sfn()?;

    // Re-load so the returned view reflects the committed state.
    let cfg = kyomi_auth::workspace_ai_config::load(ac.db(), &ac.ws_id)
        .await
        .into_sfn()?;
    let balance = load_ai_bundle_remaining_usd(ac.db(), &ac.ws_id).await?;

    Ok(view_from_config(&cfg, balance, cfg.title_model.clone()))
}

// ---------------------------------------------------------------------------
// test
// ---------------------------------------------------------------------------

/// Test a candidate AI configuration without persisting anything. Admin/owner
/// only. Uses free, auth-check endpoints for each provider:
///
/// * **Anthropic** — `GET /v1/models` with `x-api-key` + `anthropic-version`
///   headers.
/// * **OpenAI** — `GET /v1/models` with `Authorization: Bearer <key>`.
/// * **Gemini** — `GET /v1beta/models?key=<key>` (no auth header; key is
///   passed as a query parameter per Google's API convention).
///
/// Never writes to the DB, never logs the key, and never echoes the key into
/// the returned error message.
#[server(prefix = "/leptos-api", client = crate::server_fns::paywall_client::PaywallAwareClient)]
pub async fn test_workspace_ai_config(
    provider: String,
    api_key: String,
    base_url: Option<String>,
    model: Option<String>,
) -> Result<TestAiConfigResult, ServerFnError> {
    let auth = extract_auth().await?;
    let ctx = extract_context()?;
    // BYOK key testing is only available in self-hosted mode.
    if !ctx.config.self_hosted {
        return Err(ServerFnError::new(
            "API key testing is not available in SaaS mode. AI is included in your plan.",
        ));
    }
    require_permission(&auth, Permission::ManageAiConfig, "Workspace admin access required")?;

    // `model` is accepted for forward-compat (and to match the update shape)
    // but the free auth-check endpoints don't need it.
    let _ = model;

    let key = api_key.trim();
    if key.is_empty() {
        return Ok(TestAiConfigResult {
            ok: false,
            message: "API key is required.".to_string(),
        });
    }

    let base_url = base_url
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);

    test_provider(&provider, key, base_url.as_deref()).await
}

// ---------------------------------------------------------------------------
// get_resolved_ai_provider
// ---------------------------------------------------------------------------

/// Report what AI provider/model configuration is actually resolved and in
/// effect for this workspace right now — not just the names of the relevant
/// environment variables. Self-hosted only; SaaS always uses Kyomi-managed
/// AI, so there is nothing per-tenant to resolve. Admin/owner only, same
/// permission as every other AI config server fn in this file.
///
/// See [`resolve_effective_ai_provider`] for the BYOK-vs-env precedence this
/// mirrors, and [`ResolvedAiProviderView`] for the security guarantee on
/// what may appear in the response.
#[server(prefix = "/leptos-api", client = crate::server_fns::paywall_client::PaywallAwareClient)]
pub async fn get_resolved_ai_provider() -> Result<ResolvedAiProviderView, ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;
    // Resolution reporting is only meaningful for self-hosted deployments —
    // SaaS workspaces always run Kyomi-managed AI.
    if !ac.ctx.config.self_hosted {
        return Err(ServerFnError::new(
            "AI provider resolution is only reported for self-hosted deployments.",
        ));
    }
    ac.require(Permission::ManageAiConfig, "Workspace admin access required")?;

    let ws_config = kyomi_auth::workspace_ai_config::load(ac.db(), &ac.ws_id)
        .await
        .into_sfn()?;

    Ok(resolve_effective_ai_provider(&ws_config, &ac.ctx.config))
}

/// Compute what AI provider/model configuration is actually resolved and in
/// effect for a workspace, mirroring the decision
/// [`kyomi_agent::create_provider_from_workspace`] makes at request time:
///
/// * If the workspace has a BYOK provider configured (`provider != Kyomi`),
///   its own stored credentials govern — the server's env config is never
///   consulted, exactly like the real factory.
/// * Otherwise (`provider == Kyomi`), the server's env-configured
///   `LLM_PROVIDER`/`LLM_API_KEY` (or the legacy `ANTHROPIC_API_KEY`) govern,
///   via [`kyomi_agent::resolve_provider_config`] — the same function the
///   real factory calls for Kyomi-mode workspaces.
///
/// These two paths cannot both be "in effect" for a single workspace at
/// once, so there is nothing to reconcile — reporting whichever one
/// `ws_config.provider` selects *is* reporting the real answer.
///
/// Two deliberate narrowings versus the real factory, both safe only
/// because [`get_resolved_ai_provider`] is the sole caller and rejects the
/// request unless `self_hosted`:
///
/// * The factory short-circuits every SaaS workspace to Kyomi mode before
///   looking at `ws_config.provider`; this function has no such branch, so
///   it mirrors the factory faithfully **only when `self_hosted == true`**.
/// * A BYOK row whose stored key is present but blank is reported
///   `Unconfigured` here, where the factory checks only `Option::is_none()`.
///   No shipped flow can create that row — `update_workspace_ai_config`,
///   the only writer, is `require_saas`-gated — so this is defence in depth
///   against direct DB edits, not a behavioural divergence in practice.
///
/// Pure and network-free: never constructs an HTTP client or an
/// `LLMProvider` instance, so it's cheap enough to call on every page load.
/// Never puts the plaintext API key — or any derivative of it — into the
/// returned view; only `has_api_key: bool` crosses that boundary.
#[cfg(feature = "ssr")]
fn resolve_effective_ai_provider(
    ws_config: &kyomi_auth::workspace_ai_config::WorkspaceAiConfig,
    config: &kyomi_core::Config,
) -> ResolvedAiProviderView {
    use kyomi_auth::workspace_ai_config::WorkspaceAiProvider;

    // Workspace-level title_model wins over the env default — matches the
    // precedence `kyomi_agent::execution::generate_title_inner` applies.
    let title_model = ws_config
        .title_model
        .clone()
        .or_else(|| config.llm_title_model.clone());

    if ws_config.provider != WorkspaceAiProvider::Kyomi {
        // BYOK: the workspace's own stored credentials govern, not the env.
        let has_api_key = ws_config
            .api_key
            .as_deref()
            .is_some_and(|k| !k.trim().is_empty());

        if !has_api_key {
            // Mirrors the exact error `create_provider_from_workspace` raises
            // for this case (`kyomi_agent::provider`).
            return ResolvedAiProviderView::Unconfigured {
                reason: format!(
                    "workspace BYOK provider {} has no stored API key",
                    ws_config.provider.as_str()
                ),
            };
        }

        return ResolvedAiProviderView::Resolved {
            provider: ws_config.provider.as_str().to_string(),
            model: ws_config.model.clone(),
            base_url: ws_config.base_url.clone(),
            has_api_key: true,
            title_model,
        };
    }

    // Kyomi mode: the server's env-configured keys govern.
    match kyomi_agent::resolve_provider_config(config) {
        Ok(resolved) => ResolvedAiProviderView::Resolved {
            provider: resolved.provider.to_string(),
            model: resolved.model,
            base_url: resolved.base_url,
            // Guards against a pathological `ANTHROPIC_API_KEY=""` env var,
            // which `resolve_provider_config` treats as "set" (it only
            // checks `is_some`, not emptiness) but which is not a usable key.
            has_api_key: !resolved.api_key.trim().is_empty(),
            title_model,
        },
        // user_message() (KYO-448) — Display would leak the variant tag
        // (e.g. "internal: no LLM provider configured: ...").
        Err(e) => ResolvedAiProviderView::Unconfigured {
            reason: e.user_message().to_string(),
        },
    }
}

// ---------------------------------------------------------------------------
// Free auth-check HTTP helpers (server-only)
// ---------------------------------------------------------------------------

/// Default upstream base URL for each supported provider.
///
/// Kept as a separate function so unit tests can assert URL construction
/// without touching the network.
#[cfg(feature = "ssr")]
fn default_base_url(provider: &str) -> Option<&'static str> {
    match provider {
        "anthropic" => Some("https://api.anthropic.com"),
        "openai" => Some("https://api.openai.com/v1"),
        "gemini" => Some("https://generativelanguage.googleapis.com/v1beta"),
        _ => None,
    }
}

/// Build the free auth-check URL for a provider. Gemini's key goes in the
/// query string; callers are responsible for URL-encoding the key when
/// sending it over the wire — this helper does not embed the key so tests
/// can run offline.
#[cfg(feature = "ssr")]
fn auth_check_url(provider: &str, base_url: Option<&str>) -> Option<String> {
    let base = base_url
        .map(|s| s.trim_end_matches('/').to_string())
        .or_else(|| default_base_url(provider).map(str::to_string))?;

    match provider {
        "anthropic" => Some(format!("{base}/v1/models")),
        "openai" => Some(format!("{base}/models")),
        "gemini" => Some(format!("{base}/models")),
        _ => None,
    }
}

/// Dispatch to the correct upstream provider's auth-check endpoint.
#[cfg(feature = "ssr")]
async fn test_provider(
    provider: &str,
    api_key: &str,
    base_url: Option<&str>,
) -> Result<TestAiConfigResult, ServerFnError> {
    use std::time::Duration;

    let url = match auth_check_url(provider, base_url) {
        Some(u) => u,
        None => {
            return Ok(TestAiConfigResult {
                ok: false,
                message: format!("Unknown provider: {provider}"),
            });
        }
    };

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|e| ServerFnError::new(format!("Failed to create HTTP client: {e}")))?;

    let req = match provider {
        "anthropic" => client
            .get(&url)
            .header("x-api-key", api_key)
            .header("anthropic-version", "2023-06-01"),
        "openai" => client
            .get(&url)
            .header("Authorization", format!("Bearer {api_key}")),
        "gemini" => client.get(&url).query(&[("key", api_key)]),
        _ => {
            return Ok(TestAiConfigResult {
                ok: false,
                message: format!("Unknown provider: {provider}"),
            });
        }
    };

    let resp = match req.send().await {
        Ok(r) => r,
        Err(e) => {
            // Transport error — stringify the reqwest error directly. reqwest
            // never embeds request bodies or headers into its Display output,
            // so the API key cannot leak here.
            return Ok(TestAiConfigResult {
                ok: false,
                message: sanitise_error(&e.to_string(), api_key),
            });
        }
    };

    let status = resp.status();
    if status.is_success() {
        return Ok(TestAiConfigResult {
            ok: true,
            message: "Connection OK".to_string(),
        });
    }

    let body = match resp.text().await {
        Ok(text) => text,
        Err(e) => {
            return Ok(TestAiConfigResult {
                ok: false,
                message: sanitise_error(
                    &format!("HTTP {status}; failed to read response body: {e}"),
                    api_key,
                ),
            });
        }
    };
    let detail = extract_error_message(&body).unwrap_or_else(|| format!("HTTP {status}"));
    Ok(TestAiConfigResult {
        ok: false,
        message: sanitise_error(&detail, api_key),
    })
}

/// Defence-in-depth: strip the API key from any error string before returning
/// it to the client. Providers should never echo the key back, but we don't
/// trust upstream to be polite with their error messages.
#[cfg(feature = "ssr")]
fn sanitise_error(msg: &str, api_key: &str) -> String {
    if api_key.is_empty() {
        return msg.to_string();
    }
    msg.replace(api_key, "<redacted>")
}

/// Try to extract a human-readable error message from a JSON error response.
///
/// Most AI providers use one of these shapes:
/// * `{"error": {"message": "..."}}` (OpenAI, Anthropic, Gemini)
/// * `{"error": "string"}`
#[cfg(feature = "ssr")]
fn extract_error_message(body: &str) -> Option<String> {
    let parsed: serde_json::Value = serde_json::from_str(body).ok()?;

    if let Some(msg) = parsed
        .get("error")
        .and_then(|e| e.get("message"))
        .and_then(|m| m.as_str())
    {
        return Some(msg.to_string());
    }

    if let Some(msg) = parsed.get("error").and_then(|e| e.as_str()) {
        return Some(msg.to_string());
    }

    None
}

// ---------------------------------------------------------------------------
// list_workspace_ai_models
// ---------------------------------------------------------------------------

/// List models available from a BYOK provider, filtered to chat-completion
/// capable entries. Admin/owner only. Self-hosted only.
///
/// API key resolution:
/// * If `api_key` is `Some(non-empty)` after trimming, that candidate key is
///   used (lets the UI refresh the list right after a successful test, before
///   the user has hit Save).
/// * Otherwise the stored config is loaded; if it matches the requested
///   provider AND has a stored key, that key is used.
/// * Otherwise returns `Ok(vec![])` — the UI degrades to the custom-model
///   input. This is a normal pre-save state, not an error.
///
/// `kyomi` is rejected: Kyomi-credits mode uses the OpenRouter model list
/// (via [`list_openrouter_models`]) rather than a per-provider live fetch.
#[server(prefix = "/leptos-api", client = crate::server_fns::paywall_client::PaywallAwareClient)]
pub async fn list_workspace_ai_models(
    provider: String,
    api_key: Option<String>,
    base_url: Option<String>,
) -> Result<Vec<AiModelInfo>, ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;
    // BYOK model listing is only available in self-hosted mode.
    if !ac.ctx.config.self_hosted {
        return Err(ServerFnError::new(
            "Custom provider model listing is not available in SaaS mode.",
        ));
    }
    ac.require(Permission::ManageAiConfig, "Workspace admin access required")?;

    if provider == "kyomi" {
        return Err(ServerFnError::new(
            "Kyomi mode uses a curated model list; live model listing is not supported.",
        ));
    }
    if !matches!(provider.as_str(), "anthropic" | "openai" | "gemini") {
        return Err(ServerFnError::new(format!("Unknown provider: {provider}")));
    }

    // Resolve the candidate key.
    let candidate_key = api_key
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);

    // Resolve the candidate key AND stored base_url together: when we fall
    // back to the stored key, we must also honour the workspace's stored
    // base_url override so users with custom proxies (corporate Anthropic
    // proxy, Azure OpenAI, etc.) list models from their configured endpoint
    // rather than silently hitting the default one.
    let (resolved_key, stored_base_url): (String, Option<String>) = match candidate_key {
        Some(k) => (k, None),
        None => {
            let cfg = kyomi_auth::workspace_ai_config::load(ac.db(), &ac.ws_id)
                .await
                .into_sfn()?;
            // Cross-provider guard: never use a stored key intended for
            // provider X to list models from provider Y.
            if cfg.provider.as_str() != provider {
                return Ok(Vec::new());
            }
            let stored_base = cfg.base_url.clone();
            match cfg.api_key {
                Some(k) if !k.trim().is_empty() => (k, stored_base),
                _ => return Ok(Vec::new()),
            }
        }
    };

    // Resolve base URL: caller-supplied override → stored workspace config → default.
    let base = base_url
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.trim_end_matches('/').to_string())
        .or_else(|| {
            stored_base_url
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(|s| s.trim_end_matches('/').to_string())
        })
        .or_else(|| default_base_url(&provider).map(str::to_string))
        .ok_or_else(|| ServerFnError::new(format!("No base URL for provider: {provider}")))?;

    fetch_provider_models(&provider, &resolved_key, &base).await
}

/// Fetch the raw model-list response body from a provider's API.
///
/// Shared by both `fetch_provider_models` (BYOK) and
/// `fetch_openrouter_models_live` (Kyomi credits). Each caller parses the
/// body with its own parser to produce the appropriate return type.
#[cfg(feature = "ssr")]
async fn fetch_models_raw(
    provider: &str,
    api_key: &str,
    base: &str,
) -> Result<String, ServerFnError> {
    use std::time::Duration;

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|e| ServerFnError::new(format!("Failed to create HTTP client: {e}")))?;

    let url = match provider {
        "anthropic" => format!("{base}/v1/models"),
        "openai" => format!("{base}/models"),
        "gemini" => format!("{base}/models"),
        _ => return Err(ServerFnError::new(format!("Unknown provider: {provider}"))),
    };

    let req = match provider {
        "anthropic" => client
            .get(&url)
            .header("x-api-key", api_key)
            .header("anthropic-version", "2023-06-01"),
        "openai" => client
            .get(&url)
            .header("Authorization", format!("Bearer {api_key}")),
        "gemini" => client
            .get(&url)
            .query(&[("key", api_key), ("pageSize", "1000")]),
        _ => unreachable!(),
    };

    let resp = match req.send().await {
        Ok(r) => r,
        Err(e) => {
            return Err(ServerFnError::new(sanitise_error(&e.to_string(), api_key)));
        }
    };

    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        let detail = extract_error_message(&body).unwrap_or_else(|| format!("HTTP {status}"));
        return Err(ServerFnError::new(sanitise_error(&detail, api_key)));
    }

    resp.text()
        .await
        .map_err(|e| ServerFnError::new(sanitise_error(&e.to_string(), api_key)))
}

/// Fetch and parse the model list for a BYOK provider.
#[cfg(feature = "ssr")]
async fn fetch_provider_models(
    provider: &str,
    api_key: &str,
    base: &str,
) -> Result<Vec<AiModelInfo>, ServerFnError> {
    let body = fetch_models_raw(provider, api_key, base).await?;

    let parsed = match provider {
        "anthropic" => parse_anthropic_models(&body),
        "openai" => parse_openai_models(&body),
        "gemini" => parse_gemini_models(&body),
        _ => unreachable!(),
    };

    parsed.map_err(|e| {
        ServerFnError::new(sanitise_error(
            &format!("Failed to parse provider response: {e}"),
            api_key,
        ))
    })
}

// ---------------------------------------------------------------------------
// Provider parsers (pure CPU — unit-testable without network)
// ---------------------------------------------------------------------------

/// Parse Anthropic `GET /v1/models` response.
///
/// Shape: `{"data":[{"id":"claude-...","display_name":"Claude ...","type":"model","created_at":"2024-..."}]}`.
/// All entries with `type == "model"` are kept; label prefers `display_name`,
/// sorted by `created_at` descending (newest first).
#[cfg(feature = "ssr")]
fn parse_anthropic_models(body: &str) -> Result<Vec<AiModelInfo>, serde_json::Error> {
    #[derive(Deserialize)]
    struct Resp {
        data: Vec<Entry>,
    }
    #[derive(Deserialize)]
    struct Entry {
        id: String,
        #[serde(default)]
        display_name: Option<String>,
        #[serde(default, rename = "type")]
        kind: Option<String>,
        #[serde(default)]
        created_at: Option<String>,
    }

    let parsed: Resp = serde_json::from_str(body)?;
    let mut entries: Vec<(Option<String>, AiModelInfo)> = parsed
        .data
        .into_iter()
        .filter(|e| e.kind.as_deref().unwrap_or("model") == "model")
        .map(|e| {
            let label = e.display_name.clone().unwrap_or_else(|| e.id.clone());
            (
                e.created_at,
                AiModelInfo {
                    id: e.id,
                    label,
                    ..Default::default()
                },
            )
        })
        .collect();

    // Sort by created_at desc; entries missing created_at fall to the end and
    // sort reverse-alphabetically among themselves.
    entries.sort_by(|a, b| match (&a.0, &b.0) {
        (Some(x), Some(y)) => y.cmp(x),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => b.1.id.cmp(&a.1.id),
    });

    Ok(entries.into_iter().map(|(_, m)| m).collect())
}

/// Parse an OpenAI-compatible `GET /v1/models` response.
///
/// Shape: `{"data":[{"id":"gpt-4o","created":1234567890,"object":"model"}]}`.
/// Returns all models from the endpoint — no filtering. Works for OpenAI,
/// OpenRouter, vLLM, Ollama, and any other OpenAI-compatible API.
/// Sorted by `created` desc, then alphabetically by id.
#[cfg(feature = "ssr")]
fn parse_openai_models(body: &str) -> Result<Vec<AiModelInfo>, serde_json::Error> {
    #[derive(Deserialize)]
    struct Resp {
        data: Vec<Entry>,
    }
    #[derive(Deserialize)]
    struct Entry {
        id: String,
        #[serde(default)]
        created: Option<i64>,
        #[serde(default)]
        context_length: Option<u64>,
    }

    let parsed: Resp = serde_json::from_str(body)?;

    let mut entries: Vec<(Option<i64>, AiModelInfo)> = parsed
        .data
        .into_iter()
        .map(|e| {
            (
                e.created,
                AiModelInfo {
                    label: e.id.clone(),
                    id: e.id,
                    context_length: e.context_length,
                    ..Default::default()
                },
            )
        })
        .collect();

    entries.sort_by(|a, b| match (a.0, b.0) {
        (Some(x), Some(y)) => y.cmp(&x),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => b.1.id.cmp(&a.1.id),
    });

    Ok(entries.into_iter().map(|(_, m)| m).collect())
}

/// Parse Gemini `GET /v1beta/models` response.
///
/// Shape: `{"models":[{"name":"models/gemini-1.5-pro","displayName":"Gemini 1.5 Pro","supportedGenerationMethods":["generateContent"]}]}`.
/// Requires `supportedGenerationMethods` to contain `generateContent`. Strips
/// the `models/` prefix from the id. Sorted reverse-alphabetically by id so
/// newer model families surface first (`gemini-2.5-pro` > `gemini-1.5-pro`).
#[cfg(feature = "ssr")]
fn parse_gemini_models(body: &str) -> Result<Vec<AiModelInfo>, serde_json::Error> {
    #[derive(Deserialize)]
    struct Resp {
        #[serde(default)]
        models: Vec<Entry>,
    }
    #[derive(Deserialize)]
    struct Entry {
        name: String,
        #[serde(default, rename = "displayName")]
        display_name: Option<String>,
        #[serde(default, rename = "supportedGenerationMethods")]
        supported_generation_methods: Vec<String>,
    }

    let parsed: Resp = serde_json::from_str(body)?;

    const DENY_SUBSTRINGS: &[&str] = &[
        "embedding",
        "aqa",
        "imagen",
        "text-bison",
        "chat-bison",
        "-tuning",
    ];

    let mut entries: Vec<AiModelInfo> = parsed
        .models
        .into_iter()
        .filter(|e| {
            e.supported_generation_methods
                .iter()
                .any(|m| m == "generateContent")
        })
        .filter_map(|e| {
            let id = e.name.strip_prefix("models/").unwrap_or(&e.name).to_string();
            if DENY_SUBSTRINGS.iter().any(|s| id.contains(s)) {
                return None;
            }
            let label = e.display_name.unwrap_or_else(|| id.clone());
            Some(AiModelInfo { id, label, ..Default::default() })
        })
        .collect();

    entries.sort_by(|a, b| b.id.cmp(&a.id));
    Ok(entries)
}

// ---------------------------------------------------------------------------
// list_openrouter_models
// ---------------------------------------------------------------------------

/// List models available from OpenRouter.
///
/// Resolves credentials in priority order:
/// 1. Workspace BYOK config — used when the workspace has its own OpenRouter
///    base URL (host `openrouter.ai` or a subdomain) and stored API key.
/// 2. Server-level env config — used for Kyomi-credits mode workspaces where
///    the LLM key is supplied via the `LLM_API_KEY` / `LLM_BASE_URL` env
///    variables.
///
/// Returns an empty list when neither source provides a valid OpenRouter
/// configuration so the caller can degrade gracefully (e.g. show a plain
/// text input instead of a dropdown).
///
/// Results are cached per-process for one hour to avoid hammering the
/// OpenRouter API on every settings page load. The cache is invalidated
/// whenever `force_refresh = true` is passed (e.g. after the user changes
/// their API key).
///
/// Unlike [`list_workspace_ai_models`] this function is not restricted to
/// SaaS mode — OpenRouter works in self-hosted deployments too.
#[server(prefix = "/leptos-api", client = crate::server_fns::paywall_client::PaywallAwareClient)]
pub async fn list_openrouter_models(
    force_refresh: bool,
) -> Result<Vec<AiModelInfo>, ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;
    ac.require(Permission::ManageAiConfig, "Workspace admin access required")?;

    let cfg = kyomi_auth::workspace_ai_config::load(ac.db(), &ac.ws_id)
        .await
        .into_sfn()?;

    // Resolution order:
    // 1. Workspace BYOK config — used when the workspace has its own OpenRouter key.
    // 2. Server-level env config — used for Kyomi-mode workspaces where the LLM
    //    key lives in `LLM_API_KEY` / `LLM_BASE_URL` env vars.
    //
    // Neither path proceeds unless the resolved base URL's host is
    // openrouter.ai (or a subdomain of it), ensuring we never call the
    // OpenRouter model list with a gateway key whose URL merely mentions
    // openrouter.ai in a path or query.
    match openrouter_models_api_key(
        cfg.base_url.as_deref(),
        cfg.api_key.as_deref(),
        ac.ctx.config.llm_base_url.as_deref(),
        ac.ctx.config.llm_api_key.as_deref(),
    ) {
        Some(api_key) => fetch_openrouter_models_cached(api_key, force_refresh).await,
        None => Ok(Vec::new()),
    }
}

/// The API key to list OpenRouter models with, if either configured base URL
/// is OpenRouter at all.
///
/// A base URL is OpenRouter only when its host is `openrouter.ai` or a
/// subdomain of it, via [`kyomi_core::web_tools::is_openrouter_base_url`] — a
/// gateway whose URL merely contains `openrouter.ai` in a path or query is
/// not, so its key is never sent to OpenRouter's model-list endpoint.
///
/// The workspace's stored config wins over the server-level `LLM_*` env
/// config; a workspace base URL that is OpenRouter but has no usable key
/// selects nothing (the caller returns an empty list) rather than falling
/// through to the server key.
#[cfg(feature = "ssr")]
fn openrouter_models_api_key<'a>(
    workspace_base_url: Option<&str>,
    workspace_key: Option<&'a str>,
    server_base_url: Option<&str>,
    server_key: Option<&'a str>,
) -> Option<&'a str> {
    let usable = |key: Option<&'a str>| key.filter(|k| !k.trim().is_empty());
    if kyomi_core::web_tools::is_openrouter_base_url(workspace_base_url) {
        return usable(workspace_key);
    }
    if kyomi_core::web_tools::is_openrouter_base_url(server_base_url) {
        return usable(server_key);
    }
    None
}

/// Compute a short fingerprint for an API key used as the per-workspace cache
/// key.  Uses [`std::collections::hash_map::DefaultHasher`] so no external
/// crate is required.  The full key is never stored — only the 16-hex-digit
/// hash.
#[cfg(feature = "ssr")]
fn cache_key_for_api_key(api_key: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    api_key.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

/// Fetch OpenRouter models, serving from cache when the cached value is fresh.
///
/// Cache TTL: 1 hour.  The cache is a `HashMap` keyed by a fingerprint of the
/// workspace's API key so that distinct workspaces (each with their own
/// OpenRouter key) never receive another workspace's cached model list.
#[cfg(feature = "ssr")]
async fn fetch_openrouter_models_cached(
    api_key: &str,
    force_refresh: bool,
) -> Result<Vec<AiModelInfo>, ServerFnError> {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    use std::time::{Duration, Instant};

    struct CacheEntry {
        models: Vec<AiModelInfo>,
        fetched_at: Instant,
    }

    static CACHE: OnceLock<Mutex<HashMap<String, CacheEntry>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));

    const TTL: Duration = Duration::from_secs(3600);

    let key = cache_key_for_api_key(api_key);

    // Check whether we can serve from cache.
    if !force_refresh {
        let guard = cache.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(entry) = guard.get(&key)
            && entry.fetched_at.elapsed() < TTL
        {
            return Ok(entry.models.clone());
        }
    }

    // Cache miss or forced refresh — fetch from upstream.
    let models = fetch_openrouter_models_live(api_key).await?;

    let mut guard = cache.lock().unwrap_or_else(|e| e.into_inner());
    guard.insert(
        key,
        CacheEntry {
            models: models.clone(),
            fetched_at: Instant::now(),
        },
    );

    Ok(models)
}

/// Fetch and parse OpenRouter models. Uses the shared `fetch_models_raw`
/// (OpenRouter is OpenAI-compatible) and applies the OpenRouter-specific
/// parser that extracts pricing and context length.
#[cfg(feature = "ssr")]
async fn fetch_openrouter_models_live(
    api_key: &str,
) -> Result<Vec<AiModelInfo>, ServerFnError> {
    let body =
        fetch_models_raw("openai", api_key, "https://openrouter.ai/api/v1").await?;

    parse_openrouter_models(&body)
        .map_err(|e| ServerFnError::new(format!("Failed to parse OpenRouter response: {e}")))
}

/// Parse `GET https://openrouter.ai/api/v1/models` response.
///
/// Shape:
/// ```json
/// {
///   "data": [
///     {
///       "id": "openai/gpt-4o",
///       "name": "GPT-4o",
///       "pricing": { "prompt": "0.0000025", "completion": "0.00001" },
///       "context_length": 128000
///     }
///   ]
/// }
/// ```
///
/// Pricing strings are per-token USD amounts. We parse them as `f64` and
/// default to `0.0` on parse failure so a single malformed entry does not
/// discard the whole list.
#[cfg(feature = "ssr")]
fn parse_openrouter_models(body: &str) -> Result<Vec<AiModelInfo>, serde_json::Error> {
    #[derive(Deserialize)]
    struct Resp {
        data: Vec<Entry>,
    }
    #[derive(Deserialize)]
    struct Entry {
        id: String,
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        pricing: Option<Pricing>,
        #[serde(default)]
        context_length: Option<u64>,
        #[serde(default)]
        reasoning: Option<Reasoning>,
    }
    #[derive(Deserialize)]
    struct Reasoning {
        #[serde(default)]
        supported_efforts: Vec<String>,
        #[serde(default)]
        default_effort: Option<String>,
    }
    #[derive(Deserialize)]
    struct Pricing {
        #[serde(default)]
        prompt: Option<serde_json::Value>,
        #[serde(default)]
        completion: Option<serde_json::Value>,
    }

    let parsed: Resp = serde_json::from_str(body)?;

    let mut models: Vec<AiModelInfo> = parsed
        .data
        .into_iter()
        .map(|e| {
            let name = e.name.unwrap_or_else(|| e.id.clone());
            let (prompt_cost, completion_cost) = e
                .pricing
                .map(|p| {
                    (
                        parse_cost_value(p.prompt.as_ref()),
                        parse_cost_value(p.completion.as_ref()),
                    )
                })
                .unwrap_or((0.0, 0.0));
            AiModelInfo {
                id: e.id,
                label: name,
                prompt_cost_per_token: Some(prompt_cost),
                completion_cost_per_token: Some(completion_cost),
                context_length: e.context_length,
                supported_efforts: e
                    .reasoning
                    .as_ref()
                    .map(|r| r.supported_efforts.clone())
                    .unwrap_or_default(),
                default_effort: e.reasoning.and_then(|r| r.default_effort),
            }
        })
        .collect();

    // Sort alphabetically by id so the list is stable and easy to scan.
    models.sort_by(|a, b| a.id.cmp(&b.id));

    Ok(models)
}

/// Parse an OpenRouter pricing value, which may be a JSON string (`"0.0000025"`)
/// or a JSON number (`0.0000025`).  Returns `0.0` on any parse failure.
#[cfg(feature = "ssr")]
fn parse_cost_value(v: Option<&serde_json::Value>) -> f64 {
    match v {
        Some(serde_json::Value::String(s)) => s.parse::<f64>().unwrap_or(0.0),
        Some(serde_json::Value::Number(n)) => n.as_f64().unwrap_or(0.0),
        _ => 0.0,
    }
}

// ---------------------------------------------------------------------------
// Extractor re-exports
// ---------------------------------------------------------------------------

#[cfg(feature = "ssr")]
use super::{extract_auth, extract_context, require_permission, AuthenticatedContext, IntoServerFnError};
#[cfg(feature = "ssr")]
use kyomi_types::Permission;

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(all(test, feature = "ssr"))]
mod tests {
    use super::*;

    // ---- auth_check_url --------------------------------------------------

    #[test]
    fn anthropic_default_url_uses_v1_models() {
        assert_eq!(
            auth_check_url("anthropic", None).as_deref(),
            Some("https://api.anthropic.com/v1/models"),
        );
    }

    #[test]
    fn openai_default_url_uses_models() {
        assert_eq!(
            auth_check_url("openai", None).as_deref(),
            Some("https://api.openai.com/v1/models"),
        );
    }

    #[test]
    fn gemini_default_url_uses_v1beta_models() {
        assert_eq!(
            auth_check_url("gemini", None).as_deref(),
            Some("https://generativelanguage.googleapis.com/v1beta/models"),
        );
    }

    #[test]
    fn base_url_override_strips_trailing_slash() {
        assert_eq!(
            auth_check_url("openai", Some("https://proxy.example.com/v1/")).as_deref(),
            Some("https://proxy.example.com/v1/models"),
        );
    }

    #[test]
    fn base_url_override_works_for_anthropic() {
        assert_eq!(
            auth_check_url("anthropic", Some("https://anthropic.proxy.example.com")).as_deref(),
            Some("https://anthropic.proxy.example.com/v1/models"),
        );
    }

    #[test]
    fn unknown_provider_has_no_url() {
        assert_eq!(auth_check_url("bedrock", None), None);
    }

    // ---- sanitise_error --------------------------------------------------

    #[test]
    fn sanitise_error_redacts_key_when_present() {
        let key = "sk-ant-api03-supersecret";
        let msg = format!("Invalid key supplied: {key}");
        let cleaned = sanitise_error(&msg, key);
        assert!(!cleaned.contains(key), "key leaked: {cleaned}");
        assert!(cleaned.contains("<redacted>"));
    }

    #[test]
    fn sanitise_error_noop_when_key_absent() {
        let cleaned = sanitise_error("HTTP 401", "sk-xyz");
        assert_eq!(cleaned, "HTTP 401");
    }

    #[test]
    fn sanitise_error_ignores_empty_key() {
        // Must not replace every empty substring (which would blow up length).
        let cleaned = sanitise_error("HTTP 401", "");
        assert_eq!(cleaned, "HTTP 401");
    }

    // ---- extract_error_message ------------------------------------------

    #[test]
    fn extract_error_message_reads_openai_shape() {
        let body = r#"{"error": {"message": "Incorrect API key provided", "type": "invalid_request_error"}}"#;
        assert_eq!(
            extract_error_message(body).as_deref(),
            Some("Incorrect API key provided")
        );
    }

    #[test]
    fn extract_error_message_reads_string_shape() {
        let body = r#"{"error": "Unauthorized"}"#;
        assert_eq!(extract_error_message(body).as_deref(), Some("Unauthorized"));
    }

    #[test]
    fn extract_error_message_none_for_non_json() {
        assert_eq!(extract_error_message("<html>500</html>"), None);
    }

    // ---- parse_anthropic_models -----------------------------------------

    #[test]
    fn parse_anthropic_models_keeps_all_and_uses_display_name() {
        let body = r#"{
            "data": [
                {"id":"claude-sonnet-4-5-20250929","display_name":"Claude Sonnet 4.5","type":"model","created_at":"2025-09-29T00:00:00Z"},
                {"id":"claude-haiku-4-5-20251001","display_name":"Claude Haiku 4.5","type":"model","created_at":"2025-10-01T00:00:00Z"}
            ]
        }"#;
        let parsed = parse_anthropic_models(body).unwrap();
        assert_eq!(parsed.len(), 2);
        // Newest first (haiku 2025-10-01 > sonnet 2025-09-29)
        assert_eq!(parsed[0].id, "claude-haiku-4-5-20251001");
        assert_eq!(parsed[0].label, "Claude Haiku 4.5");
        assert_eq!(parsed[1].id, "claude-sonnet-4-5-20250929");
        assert_eq!(parsed[1].label, "Claude Sonnet 4.5");
    }

    #[test]
    fn parse_anthropic_models_sorts_by_created_at_desc() {
        let body = r#"{
            "data": [
                {"id":"claude-old","display_name":"Old","type":"model","created_at":"2023-01-01T00:00:00Z"},
                {"id":"claude-new","display_name":"New","type":"model","created_at":"2025-01-01T00:00:00Z"},
                {"id":"claude-mid","display_name":"Mid","type":"model","created_at":"2024-06-01T00:00:00Z"}
            ]
        }"#;
        let parsed = parse_anthropic_models(body).unwrap();
        assert_eq!(
            parsed.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
            vec!["claude-new", "claude-mid", "claude-old"],
        );
    }

    #[test]
    fn parse_anthropic_models_falls_back_to_id_when_display_name_missing() {
        let body = r#"{
            "data": [
                {"id":"claude-bare","type":"model"}
            ]
        }"#;
        let parsed = parse_anthropic_models(body).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].label, "claude-bare");
    }

    // ---- parse_openai_models --------------------------------------------

    #[test]
    fn parse_openai_models_returns_all_models() {
        let body = r#"{
            "data": [
                {"id":"gpt-4o","created":1715000000,"object":"model"},
                {"id":"text-embedding-3-large","created":1700000000,"object":"model"},
                {"id":"whisper-1","created":1690000000,"object":"model"},
                {"id":"dall-e-3","created":1695000000,"object":"model"},
                {"id":"tts-1","created":1690000000,"object":"model"},
                {"id":"omni-moderation-latest","created":1690000000,"object":"model"},
                {"id":"babbage-002","created":1690000000,"object":"model"},
                {"id":"davinci-002","created":1690000000,"object":"model"},
                {"id":"gpt-3.5-turbo-instruct","created":1690000000,"object":"model"}
            ]
        }"#;
        let parsed = parse_openai_models(body).unwrap();
        assert_eq!(parsed.len(), 9);
        assert_eq!(parsed[0].id, "gpt-4o");
    }

    #[test]
    fn parse_openai_models_keeps_gpt4o_and_o1() {
        let body = r#"{
            "data": [
                {"id":"gpt-4o","created":1715000000,"object":"model"},
                {"id":"gpt-4o-mini","created":1716000000,"object":"model"},
                {"id":"o1-preview","created":1720000000,"object":"model"},
                {"id":"o3-mini","created":1735000000,"object":"model"},
                {"id":"chatgpt-4o-latest","created":1730000000,"object":"model"}
            ]
        }"#;
        let parsed = parse_openai_models(body).unwrap();
        let ids: Vec<&str> = parsed.iter().map(|m| m.id.as_str()).collect();
        // Sorted by created desc.
        assert_eq!(
            ids,
            vec![
                "o3-mini",
                "chatgpt-4o-latest",
                "o1-preview",
                "gpt-4o-mini",
                "gpt-4o",
            ]
        );
    }

    #[test]
    fn parse_openai_models_includes_all_variants() {
        let body = r#"{
            "data": [
                {"id":"gpt-4o-audio-preview","created":1720000000,"object":"model"},
                {"id":"gpt-4o-realtime-preview","created":1720000000,"object":"model"},
                {"id":"gpt-4o-search-preview","created":1720000000,"object":"model"},
                {"id":"gpt-4o","created":1715000000,"object":"model"}
            ]
        }"#;
        let parsed = parse_openai_models(body).unwrap();
        assert_eq!(parsed.len(), 4);
    }

    // ---- parse_gemini_models --------------------------------------------

    #[test]
    fn parse_gemini_models_requires_generate_content() {
        let body = r#"{
            "models": [
                {"name":"models/gemini-2.5-pro","displayName":"Gemini 2.5 Pro","supportedGenerationMethods":["generateContent","countTokens"]},
                {"name":"models/text-embedding-004","displayName":"Text Embedding 004","supportedGenerationMethods":["embedContent"]}
            ]
        }"#;
        let parsed = parse_gemini_models(body).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].id, "gemini-2.5-pro");
        assert_eq!(parsed[0].label, "Gemini 2.5 Pro");
    }

    #[test]
    fn parse_gemini_models_excludes_embeddings() {
        let body = r#"{
            "models": [
                {"name":"models/gemini-2.5-pro","displayName":"Gemini 2.5 Pro","supportedGenerationMethods":["generateContent"]},
                {"name":"models/gemini-embedding-001","displayName":"Gemini Embedding","supportedGenerationMethods":["generateContent"]},
                {"name":"models/aqa","displayName":"Attributed QA","supportedGenerationMethods":["generateContent"]},
                {"name":"models/imagen-3.0","displayName":"Imagen","supportedGenerationMethods":["generateContent"]}
            ]
        }"#;
        let parsed = parse_gemini_models(body).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].id, "gemini-2.5-pro");
    }

    #[test]
    fn parse_gemini_models_sorts_reverse_alpha() {
        let body = r#"{
            "models": [
                {"name":"models/gemini-1.5-pro","displayName":"Gemini 1.5 Pro","supportedGenerationMethods":["generateContent"]},
                {"name":"models/gemini-2.5-pro","displayName":"Gemini 2.5 Pro","supportedGenerationMethods":["generateContent"]},
                {"name":"models/gemini-2.0-flash","displayName":"Gemini 2.0 Flash","supportedGenerationMethods":["generateContent"]}
            ]
        }"#;
        let parsed = parse_gemini_models(body).unwrap();
        assert_eq!(
            parsed.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
            vec!["gemini-2.5-pro", "gemini-2.0-flash", "gemini-1.5-pro"],
        );
    }

    // ---- openrouter_models_api_key --------------------------------------

    #[test]
    fn openrouter_key_accepts_a_real_openrouter_host() {
        for base in [
            "https://openrouter.ai/api/v1",
            "https://eu.openrouter.ai/api/v1",
            "https://OpenRouter.ai:443/api/v1",
        ] {
            assert_eq!(
                openrouter_models_api_key(Some(base), Some("ws-key"), None, None),
                Some("ws-key"),
                "{base}",
            );
        }
    }

    /// A gateway whose URL merely mentions `openrouter.ai` in a path or query
    /// is not OpenRouter: selecting no key is what keeps the gateway's key off
    /// OpenRouter's model-list endpoint.
    #[test]
    fn openrouter_key_is_never_selected_for_a_gateway_that_merely_mentions_the_name() {
        for base in [
            "https://gateway.example/openrouter.ai/v1",
            "https://gateway.example/v1?target=openrouter.ai",
            "https://openrouter.ai.evil.example/v1",
            "https://notopenrouter.ai/v1",
        ] {
            assert_eq!(
                openrouter_models_api_key(Some(base), Some("ws-key"), None, None),
                None,
                "workspace key for {base}",
            );
            assert_eq!(
                openrouter_models_api_key(None, None, Some(base), Some("env-key")),
                None,
                "server key for {base}",
            );
        }
    }

    #[test]
    fn openrouter_key_uses_the_server_config_only_when_the_workspace_is_not_openrouter() {
        assert_eq!(
            openrouter_models_api_key(
                Some("https://proxy.example/v1"),
                Some("ws-key"),
                Some("https://openrouter.ai/api/v1"),
                Some("env-key"),
            ),
            Some("env-key"),
        );
        // A workspace gateway URL does not claim OpenRouter, so the server's
        // real OpenRouter config is still usable.
        assert_eq!(
            openrouter_models_api_key(
                Some("https://gateway.example/openrouter.ai/v1"),
                Some("ws-key"),
                Some("https://openrouter.ai/api/v1"),
                Some("env-key"),
            ),
            Some("env-key"),
        );
        // A workspace OpenRouter base URL with a missing or blank key selects
        // nothing instead of borrowing the server key.
        for key in [None, Some("   ")] {
            assert_eq!(
                openrouter_models_api_key(
                    Some("https://openrouter.ai/api/v1"),
                    key,
                    Some("https://openrouter.ai/api/v1"),
                    Some("env-key"),
                ),
                None,
            );
        }
    }

    // ---- parse_openrouter_models ----------------------------------------

    #[test]
    fn parse_openrouter_models_extracts_reasoning_efforts_when_present() {
        let body = r#"{"data": [
            {"id": "deepseek/deepseek-v4.1-flash-1017",
             "reasoning": {"mandatory": false, "default_enabled": true,
                           "supported_efforts": ["max", "high", "low", "none"],
                           "default_effort": "high"}},
            {"id": "plain/model"}
        ]}"#;
        let models = parse_openrouter_models(body).unwrap();
        let ds = models.iter().find(|m| m.id.starts_with("deepseek")).unwrap();
        assert_eq!(ds.supported_efforts, ["max", "high", "low", "none"]);
        assert_eq!(ds.default_effort.as_deref(), Some("high"));
        let plain = models.iter().find(|m| m.id == "plain/model").unwrap();
        assert!(plain.supported_efforts.is_empty());
        assert_eq!(plain.default_effort, None);
    }

    #[test]
    fn parse_openrouter_models_extracts_id_name_pricing() {
        let body = r#"{
            "data": [
                {
                    "id": "openai/gpt-4o",
                    "name": "GPT-4o",
                    "pricing": { "prompt": "0.0000025", "completion": "0.00001" },
                    "context_length": 128000
                },
                {
                    "id": "anthropic/claude-sonnet-4-5",
                    "name": "Claude Sonnet 4.5",
                    "pricing": { "prompt": "0.000003", "completion": "0.000015" },
                    "context_length": 200000
                }
            ]
        }"#;
        let parsed = parse_openrouter_models(body).unwrap();
        assert_eq!(parsed.len(), 2);
        // Sorted alphabetically by id.
        assert_eq!(parsed[0].id, "anthropic/claude-sonnet-4-5");
        assert_eq!(parsed[0].label, "Claude Sonnet 4.5");
        assert!((parsed[0].prompt_cost_per_token.unwrap() - 0.000003).abs() < 1e-10);
        assert!((parsed[0].completion_cost_per_token.unwrap() - 0.000015).abs() < 1e-10);
        assert_eq!(parsed[0].context_length, Some(200000));
        assert_eq!(parsed[1].id, "openai/gpt-4o");
    }

    #[test]
    fn parse_openrouter_models_handles_missing_name() {
        let body = r#"{
            "data": [
                {
                    "id": "some/model",
                    "context_length": 4096
                }
            ]
        }"#;
        let parsed = parse_openrouter_models(body).unwrap();
        assert_eq!(parsed.len(), 1);
        // Label falls back to id when absent.
        assert_eq!(parsed[0].label, "some/model");
        assert_eq!(parsed[0].prompt_cost_per_token, Some(0.0));
        assert_eq!(parsed[0].completion_cost_per_token, Some(0.0));
    }

    #[test]
    fn parse_openrouter_models_handles_numeric_pricing() {
        // Some OpenRouter entries return pricing as JSON numbers, not strings.
        let body = r#"{
            "data": [
                {
                    "id": "free/model",
                    "name": "Free Model",
                    "pricing": { "prompt": 0.0, "completion": 0.0 },
                    "context_length": 8192
                }
            ]
        }"#;
        let parsed = parse_openrouter_models(body).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].prompt_cost_per_token, Some(0.0));
    }

    #[test]
    fn parse_cost_value_handles_string_and_number() {
        assert!((parse_cost_value(Some(&serde_json::json!("0.0000025"))) - 0.0000025).abs() < 1e-15);
        assert!((parse_cost_value(Some(&serde_json::json!(0.0000025))) - 0.0000025).abs() < 1e-15);
        assert_eq!(parse_cost_value(None), 0.0);
        assert_eq!(parse_cost_value(Some(&serde_json::json!("not-a-number"))), 0.0);
    }

    // ---- resolve_effective_ai_provider ------------------------------------

    fn byok_ws_config(
        provider: kyomi_auth::workspace_ai_config::WorkspaceAiProvider,
        api_key: Option<&str>,
    ) -> kyomi_auth::workspace_ai_config::WorkspaceAiConfig {
        kyomi_auth::workspace_ai_config::WorkspaceAiConfig {
            provider,
            model: None,
            api_key: api_key.map(str::to_string),
            base_url: None,
            title_model: None,
            context_window: 0,
            sampling: Default::default(),
            sampling_warnings: Vec::new(),
            watch_llm: Default::default(),
            watch_llm_warnings: Vec::new(),
            compaction: Default::default(),
            compaction_warnings: Vec::new(),
        }
    }

    fn kyomi_ws_config() -> kyomi_auth::workspace_ai_config::WorkspaceAiConfig {
        byok_ws_config(kyomi_auth::workspace_ai_config::WorkspaceAiProvider::Kyomi, None)
    }

    #[test]
    fn resolve_kyomi_mode_env_resolves_successfully() {
        let mut config = kyomi_core::Config::test_config();
        config.self_hosted = true;
        config.llm_provider = Some("openai".into());
        config.llm_api_key = Some("sk-server-key".into());
        config.llm_model = Some("gpt-4o".into());
        config.llm_base_url = Some("https://proxy.example.com".into());

        let view = resolve_effective_ai_provider(&kyomi_ws_config(), &config);
        assert_eq!(
            view,
            ResolvedAiProviderView::Resolved {
                provider: "openai".to_string(),
                model: Some("gpt-4o".to_string()),
                base_url: Some("https://proxy.example.com".to_string()),
                has_api_key: true,
                title_model: None,
            }
        );
    }

    #[test]
    fn resolve_kyomi_mode_no_model_means_provider_default() {
        let mut config = kyomi_core::Config::test_config();
        config.self_hosted = true;
        config.llm_provider = None;
        config.llm_api_key = None;
        config.anthropic_api_key = Some("sk-ant-server".into());
        config.llm_model = None;

        let view = resolve_effective_ai_provider(&kyomi_ws_config(), &config);
        assert_eq!(
            view,
            ResolvedAiProviderView::Resolved {
                provider: "anthropic".to_string(),
                model: None,
                base_url: None,
                has_api_key: true,
                title_model: None,
            }
        );
    }

    #[test]
    fn resolve_kyomi_mode_no_keys_is_unconfigured() {
        let mut config = kyomi_core::Config::test_config();
        config.self_hosted = true;
        config.llm_provider = None;
        config.llm_api_key = None;
        config.anthropic_api_key = None;

        let view = resolve_effective_ai_provider(&kyomi_ws_config(), &config);
        match view {
            ResolvedAiProviderView::Unconfigured { reason } => {
                assert!(reason.contains("no LLM provider configured"), "got: {reason}");
            }
            other => panic!("expected Unconfigured, got {other:?}"),
        }
    }

    #[test]
    fn resolve_kyomi_mode_partial_env_is_unconfigured_with_partial_reason() {
        let mut config = kyomi_core::Config::test_config();
        config.self_hosted = true;
        config.llm_provider = Some("openai".into());
        config.llm_api_key = None;
        config.anthropic_api_key = None;

        let view = resolve_effective_ai_provider(&kyomi_ws_config(), &config);
        match view {
            ResolvedAiProviderView::Unconfigured { reason } => {
                assert!(reason.contains("LLM_API_KEY is missing"), "got: {reason}");
            }
            other => panic!("expected Unconfigured, got {other:?}"),
        }
    }

    #[test]
    fn resolve_byok_with_stored_key_uses_workspace_credentials_not_env() {
        let mut ws = byok_ws_config(
            kyomi_auth::workspace_ai_config::WorkspaceAiProvider::Gemini,
            Some("AIza-workspace-key"),
        );
        ws.model = Some("gemini-2.5-pro".to_string());
        ws.base_url = Some("https://gemini.proxy.example.com".to_string());

        // Deliberately different from the workspace config — proves the env
        // is not consulted at all when BYOK is configured.
        let mut config = kyomi_core::Config::test_config();
        config.self_hosted = true;
        config.llm_provider = Some("openai".into());
        config.llm_api_key = Some("sk-server-key-should-not-appear".into());
        config.llm_model = Some("gpt-4o".into());

        let view = resolve_effective_ai_provider(&ws, &config);
        assert_eq!(
            view,
            ResolvedAiProviderView::Resolved {
                provider: "gemini".to_string(),
                model: Some("gemini-2.5-pro".to_string()),
                base_url: Some("https://gemini.proxy.example.com".to_string()),
                has_api_key: true,
                title_model: None,
            }
        );
    }

    #[test]
    fn resolve_byok_without_stored_key_is_unconfigured() {
        let ws = byok_ws_config(
            kyomi_auth::workspace_ai_config::WorkspaceAiProvider::Anthropic,
            None,
        );
        let config = kyomi_core::Config::test_config();

        let view = resolve_effective_ai_provider(&ws, &config);
        match view {
            ResolvedAiProviderView::Unconfigured { reason } => {
                assert!(reason.contains("no stored API key"), "got: {reason}");
                assert!(reason.contains("anthropic"), "got: {reason}");
            }
            other => panic!("expected Unconfigured, got {other:?}"),
        }
    }

    #[test]
    fn resolve_byok_blank_stored_key_is_unconfigured() {
        // Whitespace-only key must not be treated as "has a key".
        let ws = byok_ws_config(
            kyomi_auth::workspace_ai_config::WorkspaceAiProvider::OpenAI,
            Some("   "),
        );
        let config = kyomi_core::Config::test_config();

        let view = resolve_effective_ai_provider(&ws, &config);
        assert!(matches!(view, ResolvedAiProviderView::Unconfigured { .. }));
    }

    #[test]
    fn resolve_title_model_prefers_workspace_setting_over_env() {
        let mut ws = kyomi_ws_config();
        ws.title_model = Some("claude-haiku-4-5-20251001".to_string());

        let mut config = kyomi_core::Config::test_config();
        config.self_hosted = true;
        config.anthropic_api_key = Some("sk-ant-server".into());
        config.llm_title_model = Some("gpt-4o-mini".into());

        let view = resolve_effective_ai_provider(&ws, &config);
        match view {
            ResolvedAiProviderView::Resolved { title_model, .. } => {
                assert_eq!(title_model.as_deref(), Some("claude-haiku-4-5-20251001"));
            }
            other => panic!("expected Resolved, got {other:?}"),
        }
    }

    #[test]
    fn resolve_title_model_falls_back_to_env_when_workspace_unset() {
        let ws = kyomi_ws_config();

        let mut config = kyomi_core::Config::test_config();
        config.self_hosted = true;
        config.anthropic_api_key = Some("sk-ant-server".into());
        config.llm_title_model = Some("gpt-4o-mini".into());

        let view = resolve_effective_ai_provider(&ws, &config);
        match view {
            ResolvedAiProviderView::Resolved { title_model, .. } => {
                assert_eq!(title_model.as_deref(), Some("gpt-4o-mini"));
            }
            other => panic!("expected Resolved, got {other:?}"),
        }
    }

    /// Security-critical: the exact assertion KYO-265 requires — serialise
    /// the view produced from a config carrying a real-looking API key and
    /// confirm the key never appears in the wire JSON, for both the BYOK
    /// path and the Kyomi/env path.
    #[test]
    fn resolved_view_json_never_contains_the_api_key() {
        let ws = byok_ws_config(
            kyomi_auth::workspace_ai_config::WorkspaceAiProvider::OpenAI,
            Some("sk-THIS-MUST-NEVER-LEAK-99887766"),
        );
        let config = kyomi_core::Config::test_config();

        let view = resolve_effective_ai_provider(&ws, &config);
        let json = serde_json::to_string(&view).expect("view must serialise");
        assert!(
            !json.contains("THIS-MUST-NEVER-LEAK"),
            "API key material leaked into serialised view: {json}"
        );

        let mut config2 = kyomi_core::Config::test_config();
        config2.self_hosted = true;
        config2.llm_provider = Some("anthropic".into());
        config2.llm_api_key = Some("sk-ANOTHER-SECRET-SHOULD-NOT-LEAK-11223344".into());
        let view2 = resolve_effective_ai_provider(&kyomi_ws_config(), &config2);
        let json2 = serde_json::to_string(&view2).expect("view must serialise");
        assert!(
            !json2.contains("ANOTHER-SECRET-SHOULD-NOT-LEAK"),
            "API key material leaked into serialised view: {json2}"
        );
    }
}
