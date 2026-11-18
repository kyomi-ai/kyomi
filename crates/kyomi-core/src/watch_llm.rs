// SPDX-License-Identifier: AGPL-3.0-or-later

//! Separate LLM configuration for watches (KYO-973).
//!
//! Watches are unattended and latency-tolerant, so they can use a cheaper
//! model, a different provider, or a provider batch API. Every field is
//! optional and falls back to the main LLM configuration when unset.
//!
//! Reasoning effort and temperature are **not** part of this block: they keep
//! the KYO-972 `watch` profile ([`crate::llm_sampling`], `LLM_WATCH_*_EFFORT`
//! / `LLM_WATCH_TEMPERATURE` and the workspace `watch_*` keys), which already
//! falls back to the chat values. This module adds only what KYO-972 lacked:
//! provider, model, base URL, API key and the realtime/batch mode.
//!
//! Layers, highest first: workspace override (self-hosted only), instance env.
//!
//! The *connection* (provider, API key, base URL) is resolved as one unit from
//! the highest layer that sets any of them, so a workspace that points watches
//! at another provider can never be sent the instance key meant for a
//! different one. The model follows the same rule: a model configured for the
//! main LLM is never sent to a watch-specific provider.

use std::fmt;

use crate::llm_sampling::InvalidSetting;

/// Default expected completion window for a batch, in minutes.
///
/// Both vendors document "most batches finish within an hour" (see the
/// KYO-973 ticket); a watch scheduled more tightly than this runs realtime.
pub const DEFAULT_BATCH_WINDOW_MINUTES: u32 = 60;

/// Longest accepted batch window: both vendors expire a batch after 24 hours.
pub const MAX_BATCH_WINDOW_MINUTES: u32 = 24 * 60;

/// How a watch run talks to the LLM.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WatchLlmMode {
    /// One request per iteration, answered immediately. The default.
    #[default]
    Realtime,
    /// One provider batch per iteration (about half price, minutes to hours).
    Batch,
}

impl WatchLlmMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Realtime => "realtime",
            Self::Batch => "batch",
        }
    }

    pub fn parse(raw: &str) -> Result<Self, InvalidSetting> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "realtime" => Ok(Self::Realtime),
            "batch" => Ok(Self::Batch),
            other => Err(InvalidSetting(format!(
                "invalid watch LLM mode {other:?}: use \"realtime\" or \"batch\""
            ))),
        }
    }
}

impl fmt::Display for WatchLlmMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What to do when a batch expires, fails or returns an unusable result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BatchFailurePolicy {
    /// Retry the affected iteration once in realtime, then fail through the
    /// existing watch error path. The default.
    #[default]
    RealtimeRetry,
    /// Fail the run through the existing watch error path immediately.
    Error,
}

impl BatchFailurePolicy {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::RealtimeRetry => "realtime_retry",
            Self::Error => "error",
        }
    }

    pub fn parse(raw: &str) -> Result<Self, InvalidSetting> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "realtime_retry" => Ok(Self::RealtimeRetry),
            "error" => Ok(Self::Error),
            other => Err(InvalidSetting(format!(
                "invalid batch failure policy {other:?}: use \"realtime_retry\" or \"error\""
            ))),
        }
    }
}

/// Parse a batch window in minutes (`1..=MAX_BATCH_WINDOW_MINUTES`).
pub fn parse_batch_window_minutes(raw: &str) -> Result<u32, InvalidSetting> {
    let n: u32 = raw.trim().parse().map_err(|_| {
        InvalidSetting(format!(
            "invalid batch window {raw:?}: use a whole number of minutes"
        ))
    })?;
    if n == 0 || n > MAX_BATCH_WINDOW_MINUTES {
        return Err(InvalidSetting(format!(
            "invalid batch window {n}: must be between 1 and {MAX_BATCH_WINDOW_MINUTES} minutes"
        )));
    }
    Ok(n)
}

/// Settings from one layer (instance env or workspace). `None` = not set.
///
/// `Debug` is implemented by hand so the API key can never reach a log line.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct WatchLlmSettings {
    pub provider: Option<String>,
    pub model: Option<String>,
    pub base_url: Option<String>,
    pub api_key: Option<String>,
    pub mode: Option<WatchLlmMode>,
    pub batch_window_minutes: Option<u32>,
    pub batch_failure: Option<BatchFailurePolicy>,
}

impl WatchLlmSettings {
    /// Whether this layer names its own connection (provider, key or URL).
    fn sets_connection(&self) -> bool {
        self.provider.is_some() || self.api_key.is_some() || self.base_url.is_some()
    }

    /// Structural validation of one layer, independent of provider support.
    ///
    /// A watch-specific connection needs both a provider and a key: a base URL
    /// or key alone would be combined with the *main* provider, which is the
    /// cross-provider key leak this module exists to prevent.
    pub fn validate(&self) -> Result<(), InvalidSetting> {
        if self.sets_connection() && (self.provider.is_none() || self.api_key.is_none()) {
            return Err(InvalidSetting(
                "a watch LLM provider and API key must be set together: both are required \
                 when either, or a base URL, is configured"
                    .into(),
            ));
        }
        Ok(())
    }
}

/// The watch LLM connection chosen by [`resolve_watch_llm`].
#[derive(Debug, Clone, PartialEq)]
pub struct WatchConnection {
    pub provider: String,
    pub api_key: String,
    pub base_url: Option<String>,
}

/// What a watch run should use.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedWatchLlm {
    /// `Some` when the watches have their own provider; `None` = use the main
    /// LLM connection.
    pub connection: Option<WatchConnection>,
    /// Whether `connection` came from the workspace override (the workspace's
    /// own key, billed to the workspace) rather than the instance env.
    pub connection_from_workspace: bool,
    /// Model for watches. With no `connection` and `None`, the main model.
    pub model: Option<String>,
    pub mode: WatchLlmMode,
    pub batch_window_minutes: u32,
    pub batch_failure: BatchFailurePolicy,
}

/// Resolve the layers. `workspace` must be [`WatchLlmSettings::default`] on
/// deployments that ignore workspace overrides (SaaS).
///
/// Invalid layers are the caller's to reject at write time; a layer that fails
/// [`WatchLlmSettings::validate`] here contributes no connection.
pub fn resolve_watch_llm(
    workspace: &WatchLlmSettings,
    env: &WatchLlmSettings,
) -> ResolvedWatchLlm {
    let connection_layer = [workspace, env]
        .into_iter()
        .find(|l| l.sets_connection() && l.validate().is_ok());
    let connection = connection_layer.map(|l| WatchConnection {
        provider: l.provider.clone().unwrap_or_default(),
        api_key: l.api_key.clone().unwrap_or_default(),
        base_url: l.base_url.clone(),
    });
    // A model belongs to the connection it was configured with. With no watch
    // connection, the first layer's model applies to the main provider.
    let model = match connection_layer {
        Some(l) => l.model.clone(),
        None => [workspace, env].into_iter().find_map(|l| l.model.clone()),
    };
    ResolvedWatchLlm {
        connection,
        connection_from_workspace: connection_layer.is_some_and(|l| std::ptr::eq(l, workspace)),
        model,
        mode: [workspace, env]
            .into_iter()
            .find_map(|l| l.mode)
            .unwrap_or_default(),
        batch_window_minutes: [workspace, env]
            .into_iter()
            .find_map(|l| l.batch_window_minutes)
            .unwrap_or(DEFAULT_BATCH_WINDOW_MINUTES),
        batch_failure: [workspace, env]
            .into_iter()
            .find_map(|l| l.batch_failure)
            .unwrap_or_default(),
    }
}

// ---------------------------------------------------------------------------
// Workspace storage (settings.custom_settings)
// ---------------------------------------------------------------------------

pub const KEY_PROVIDER: &str = "watch_llm_provider";
pub const KEY_MODEL: &str = "watch_llm_model";
pub const KEY_BASE_URL: &str = "watch_llm_base_url";
pub const KEY_MODE: &str = "watch_llm_mode";
pub const KEY_BATCH_WINDOW: &str = "watch_llm_batch_window_minutes";
pub const KEY_BATCH_FAILURE: &str = "watch_llm_batch_failure";

/// All non-secret workspace keys, for writers that clear the block.
pub const WORKSPACE_KEYS: [&str; 6] = [
    KEY_PROVIDER,
    KEY_MODEL,
    KEY_BASE_URL,
    KEY_MODE,
    KEY_BATCH_WINDOW,
    KEY_BATCH_FAILURE,
];

/// Read the workspace's non-secret watch LLM settings. Invalid stored values
/// are dropped and reported (never fatal), like the KYO-972 sampling reader.
/// The API key lives in its own encrypted column and is added by the caller.
pub fn read_workspace_watch_llm(
    settings: &Option<serde_json::Value>,
) -> (WatchLlmSettings, Vec<String>) {
    let get = |key: &str| {
        settings
            .as_ref()
            .and_then(|s| s.get("custom_settings"))
            .and_then(|cs| cs.get(key))
            .filter(|v| !v.is_null())
    };
    let mut warnings = Vec::new();
    let mut text = |key: &str| -> Option<String> {
        let v = get(key)?;
        match v.as_str().map(str::trim) {
            Some(s) if !s.is_empty() => Some(s.to_string()),
            _ => {
                warnings.push(format!("{key}: must be a non-empty string (stored: {v})"));
                None
            }
        }
    };
    let provider = text(KEY_PROVIDER);
    let model = text(KEY_MODEL);
    let base_url = text(KEY_BASE_URL);
    let mode = get(KEY_MODE).and_then(|v| {
        v.as_str()
            .ok_or_else(|| InvalidSetting("must be a string".into()))
            .and_then(WatchLlmMode::parse)
            .map_err(|e| warnings.push(format!("{KEY_MODE}: {e} (stored: {v})")))
            .ok()
    });
    let batch_window_minutes = get(KEY_BATCH_WINDOW).and_then(|v| {
        let raw = v.as_u64().map(|n| n.to_string()).or_else(|| v.as_str().map(str::to_string));
        raw.ok_or_else(|| InvalidSetting("must be a whole number".into()))
            .and_then(|r| parse_batch_window_minutes(&r))
            .map_err(|e| warnings.push(format!("{KEY_BATCH_WINDOW}: {e} (stored: {v})")))
            .ok()
    });
    let batch_failure = get(KEY_BATCH_FAILURE).and_then(|v| {
        v.as_str()
            .ok_or_else(|| InvalidSetting("must be a string".into()))
            .and_then(BatchFailurePolicy::parse)
            .map_err(|e| warnings.push(format!("{KEY_BATCH_FAILURE}: {e} (stored: {v})")))
            .ok()
    });
    (
        WatchLlmSettings {
            provider,
            model,
            base_url,
            api_key: None,
            mode,
            batch_window_minutes,
            batch_failure,
        },
        warnings,
    )
}

/// Build the instance layer from a variable lookup (the process environment in
/// production; a map in tests). A set-but-invalid variable is an error naming
/// it, so a typo aborts startup instead of being silently ignored. Empty
/// values count as unset.
pub fn watch_llm_from_env(
    get: impl Fn(&str) -> Option<String>,
) -> Result<WatchLlmSettings, String> {
    let text = |key: &str| get(key).map(|v| v.trim().to_string()).filter(|v| !v.is_empty());
    fn parsed<T>(
        text: Option<String>,
        key: &str,
        f: fn(&str) -> Result<T, InvalidSetting>,
    ) -> Result<Option<T>, String> {
        text.map(|v| f(&v).map_err(|e| format!("{key}: {e}"))).transpose()
    }
    let settings = WatchLlmSettings {
        provider: text("LLM_WATCH_PROVIDER"),
        model: text("LLM_WATCH_MODEL"),
        base_url: text("LLM_WATCH_BASE_URL"),
        api_key: text("LLM_WATCH_API_KEY"),
        mode: parsed(text("LLM_WATCH_MODE"), "LLM_WATCH_MODE", WatchLlmMode::parse)?,
        batch_window_minutes: parsed(
            text("LLM_WATCH_BATCH_WINDOW_MINUTES"),
            "LLM_WATCH_BATCH_WINDOW_MINUTES",
            parse_batch_window_minutes,
        )?,
        batch_failure: parsed(text("LLM_WATCH_BATCH_ON_FAILURE"), "LLM_WATCH_BATCH_ON_FAILURE", BatchFailurePolicy::parse)?,
    };
    settings
        .validate()
        .map_err(|e| format!("LLM_WATCH_PROVIDER / LLM_WATCH_API_KEY / LLM_WATCH_BASE_URL: {e}"))?;
    Ok(settings)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn conn(provider: &str, key: &str) -> WatchLlmSettings {
        WatchLlmSettings {
            provider: Some(provider.into()),
            api_key: Some(key.into()),
            ..Default::default()
        }
    }

    #[test]
    fn nothing_set_falls_back_to_the_main_llm_in_realtime() {
        let r = resolve_watch_llm(&Default::default(), &Default::default());
        assert_eq!(r.connection, None);
        assert_eq!(r.model, None);
        assert_eq!(r.mode, WatchLlmMode::Realtime);
        assert_eq!(r.batch_window_minutes, DEFAULT_BATCH_WINDOW_MINUTES);
        assert_eq!(r.batch_failure, BatchFailurePolicy::RealtimeRetry);
    }

    #[test]
    fn workspace_connection_beats_env_and_is_taken_whole() {
        let env = WatchLlmSettings {
            model: Some("env-model".into()),
            base_url: Some("https://env.example".into()),
            ..conn("openai", "env-key")
        };
        let ws = WatchLlmSettings {
            model: Some("ws-model".into()),
            ..conn("anthropic", "ws-key")
        };
        let r = resolve_watch_llm(&ws, &env);
        let c = r.connection.expect("connection");
        assert_eq!(c.provider, "anthropic");
        assert_eq!(c.api_key, "ws-key");
        assert!(r.connection_from_workspace);
        assert_eq!(c.base_url, None, "the env base URL must not leak onto the workspace provider");
        assert_eq!(r.model.as_deref(), Some("ws-model"));
    }

    #[test]
    fn a_workspace_model_without_a_connection_does_not_pull_the_env_connection() {
        // The workspace only picks a cheaper model for the main provider; the
        // env watch connection is a different provider the model is not for.
        let env = conn("openai", "env-key");
        let ws = WatchLlmSettings { model: Some("haiku".into()), ..Default::default() };
        let r = resolve_watch_llm(&ws, &env);
        assert_eq!(r.connection.expect("env connection").provider, "openai");
        assert_eq!(
            r.model, None,
            "a model chosen without a connection must not be sent to the env connection's provider"
        );
    }

    #[test]
    fn model_alone_applies_to_the_main_provider() {
        let env = WatchLlmSettings { model: Some("haiku".into()), ..Default::default() };
        let r = resolve_watch_llm(&Default::default(), &env);
        assert_eq!(r.connection, None);
        assert_eq!(r.model.as_deref(), Some("haiku"));
    }

    #[test]
    fn mode_window_and_policy_resolve_per_field() {
        let env = WatchLlmSettings {
            mode: Some(WatchLlmMode::Batch),
            batch_window_minutes: Some(30),
            ..Default::default()
        };
        let ws = WatchLlmSettings {
            batch_failure: Some(BatchFailurePolicy::Error),
            ..Default::default()
        };
        let r = resolve_watch_llm(&ws, &env);
        assert_eq!(r.mode, WatchLlmMode::Batch);
        assert_eq!(r.batch_window_minutes, 30);
        assert_eq!(r.batch_failure, BatchFailurePolicy::Error);
    }

    #[test]
    fn a_partial_connection_is_rejected_not_combined_with_the_main_provider() {
        let key_only = WatchLlmSettings { api_key: Some("k".into()), ..Default::default() };
        assert!(key_only.validate().is_err());
        let url_only = WatchLlmSettings { base_url: Some("https://x".into()), ..Default::default() };
        assert!(url_only.validate().is_err());
        let provider_only = WatchLlmSettings { provider: Some("openai".into()), ..Default::default() };
        assert!(provider_only.validate().is_err());
        assert!(conn("openai", "k").validate().is_ok());
        assert!(WatchLlmSettings::default().validate().is_ok());
        // And resolution never builds a connection from a partial layer.
        assert_eq!(resolve_watch_llm(&key_only, &Default::default()).connection, None);
    }


    #[test]
    fn env_parsing_accepts_valid_and_names_the_bad_variable() {
        let map = |pairs: &'static [(&'static str, &'static str)]| {
            move |k: &str| pairs.iter().find(|(n, _)| *n == k).map(|(_, v)| v.to_string())
        };
        let ok = watch_llm_from_env(map(&[
            ("LLM_WATCH_MODE", "Batch"),
            ("LLM_WATCH_BATCH_WINDOW_MINUTES", "45"),
            ("LLM_WATCH_BATCH_ON_FAILURE", "error"),
            ("LLM_WATCH_MODEL", "  "),
        ]))
        .unwrap();
        assert_eq!(ok.mode, Some(WatchLlmMode::Batch));
        assert_eq!(ok.batch_window_minutes, Some(45));
        assert_eq!(ok.batch_failure, Some(BatchFailurePolicy::Error));
        assert_eq!(ok.model, None, "blank counts as unset");

        let err = watch_llm_from_env(map(&[("LLM_WATCH_MODE", "turbo")])).unwrap_err();
        assert!(err.contains("LLM_WATCH_MODE"), "{err}");
        let err = watch_llm_from_env(map(&[("LLM_WATCH_BATCH_WINDOW_MINUTES", "0")])).unwrap_err();
        assert!(err.contains("LLM_WATCH_BATCH_WINDOW_MINUTES"), "{err}");
        let err = watch_llm_from_env(map(&[("LLM_WATCH_API_KEY", "k")])).unwrap_err();
        assert!(err.contains("LLM_WATCH_API_KEY"), "{err}");
    }

    #[test]
    fn workspace_reader_drops_invalid_values_with_warnings() {
        let settings = Some(json!({"custom_settings": {
            "watch_llm_provider": "anthropic",
            "watch_llm_model": "m",
            "watch_llm_mode": "batch",
            "watch_llm_batch_window_minutes": 90,
            "watch_llm_batch_failure": "nonsense",
        }}));
        let (s, warnings) = read_workspace_watch_llm(&settings);
        assert_eq!(s.provider.as_deref(), Some("anthropic"));
        assert_eq!(s.mode, Some(WatchLlmMode::Batch));
        assert_eq!(s.batch_window_minutes, Some(90));
        assert_eq!(s.batch_failure, None);
        assert_eq!(s.api_key, None);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].starts_with(KEY_BATCH_FAILURE));

        let (empty, warnings) = read_workspace_watch_llm(&None);
        assert_eq!(empty, WatchLlmSettings::default());
        assert!(warnings.is_empty());
    }
}
