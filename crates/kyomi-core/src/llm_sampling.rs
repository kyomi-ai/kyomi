// SPDX-License-Identifier: AGPL-3.0-or-later

//! LLM sampling / reasoning settings (KYO-972).
//!
//! Two knobs are configurable per call profile (interactive chat, watches,
//! background title/summary calls), at instance level (env) and per workspace
//! (`settings.custom_settings`):
//!
//! * **reasoning effort** — an opaque string passed to the provider unchanged
//!   (`low`, `high`, `max`, `none`, `50`, a vendor-specific level, ...). It is
//!   deliberately not an enum: vocabularies differ per provider and change.
//! * **temperature** — a number, or an explicit *omit* (some models, e.g.
//!   Claude Sonnet 5.5, reject the field with a 400).
//!
//! A value that is not configured at all is distinct from an explicit
//! [`Setting::Omit`]: unset falls through to the next layer / default, omit
//! means "send no field".

use std::fmt;

/// Keyword that explicitly omits a field from the provider request.
pub const OMIT_KEYWORD: &str = "omit";

/// Maximum length of a reasoning-effort value.
const MAX_EFFORT_LEN: usize = 32;

/// Default reasoning effort for background calls (titles, summaries) when a
/// title model is configured, no explicit title effort is set, **and** a chat
/// reasoning effort is configured.
///
/// The last condition is what keeps existing installs unchanged: with no
/// effort configured anywhere, nothing is sent, so a non-reasoning title model
/// (e.g. `gpt-4o-mini`, a Haiku model, a local backend) never receives a field
/// it may reject. Once an operator has configured an effort for chat, the
/// backend is known to accept the field and titles default to `low` instead of
/// inheriting the chat value.
pub const DEFAULT_TITLE_REASONING_EFFORT: &str = "low";

/// Default temperature for interactive chat, Slack and watches.
pub const DEFAULT_CHAT_TEMPERATURE: f32 = 0.7;

/// A configured value: either a concrete value or an explicit omit.
#[derive(Debug, Clone, PartialEq)]
pub enum Setting<T> {
    Value(T),
    Omit,
}

impl<T> Setting<T> {
    /// The value to send, or `None` when the field must be omitted.
    pub fn into_option(self) -> Option<T> {
        match self {
            Setting::Value(v) => Some(v),
            Setting::Omit => None,
        }
    }
}

/// Which kind of call is being made. Selects which configured values apply.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LlmProfile {
    /// Interactive chat, copilots, Slack, MCP.
    #[default]
    Chat,
    /// Scheduled watch executions.
    Watch,
    /// Cheap background calls: session titles, dashboard summaries, tagging.
    Title,
}

/// Error for an invalid configured value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidSetting(pub String);

impl fmt::Display for InvalidSetting {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for InvalidSetting {}

/// Parse a reasoning-effort value. `omit` (case-insensitive) is the explicit
/// omit; anything else must be 1-32 chars of `[A-Za-z0-9._-]` so a typo cannot
/// inject arbitrary request content. The value itself is never rewritten.
pub fn parse_reasoning_effort(raw: &str) -> Result<Setting<String>, InvalidSetting> {
    let v = raw.trim();
    if v.is_empty() {
        return Err(InvalidSetting("reasoning effort must not be empty".into()));
    }
    if v.eq_ignore_ascii_case(OMIT_KEYWORD) {
        return Ok(Setting::Omit);
    }
    if v.len() > MAX_EFFORT_LEN
        || !v
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    {
        return Err(InvalidSetting(format!(
            "invalid reasoning effort {v:?}: use up to {MAX_EFFORT_LEN} characters of letters, digits, '.', '_' or '-', or \"{OMIT_KEYWORD}\""
        )));
    }
    Ok(Setting::Value(v.to_string()))
}

/// Parse a temperature. `omit` (case-insensitive) is the explicit omit;
/// otherwise a finite number in `0.0..=2.0`.
pub fn parse_temperature(raw: &str) -> Result<Setting<f32>, InvalidSetting> {
    let v = raw.trim();
    if v.eq_ignore_ascii_case(OMIT_KEYWORD) {
        return Ok(Setting::Omit);
    }
    let n: f32 = v.parse().map_err(|_| {
        InvalidSetting(format!(
            "invalid temperature {v:?}: use a number between 0 and 2, or \"{OMIT_KEYWORD}\""
        ))
    })?;
    if !n.is_finite() || !(0.0..=2.0).contains(&n) {
        return Err(InvalidSetting(format!(
            "invalid temperature {v:?}: must be between 0 and 2"
        )));
    }
    Ok(Setting::Value(n))
}

/// Settings for the three call profiles. Every field is `None` when not
/// configured (distinct from an explicit omit).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LlmSamplingSettings {
    pub reasoning_effort: Option<Setting<String>>,
    pub title_reasoning_effort: Option<Setting<String>>,
    pub watch_reasoning_effort: Option<Setting<String>>,
    pub temperature: Option<Setting<f32>>,
    pub title_temperature: Option<Setting<f32>>,
    pub watch_temperature: Option<Setting<f32>>,
}

impl LlmSamplingSettings {
    fn effort_for(&self, profile: LlmProfile) -> Option<&Setting<String>> {
        match profile {
            LlmProfile::Chat => self.reasoning_effort.as_ref(),
            LlmProfile::Watch => self.watch_reasoning_effort.as_ref(),
            LlmProfile::Title => self.title_reasoning_effort.as_ref(),
        }
    }

    fn temperature_for(&self, profile: LlmProfile) -> Option<&Setting<f32>> {
        match profile {
            LlmProfile::Chat => self.temperature.as_ref(),
            LlmProfile::Watch => self.watch_temperature.as_ref(),
            LlmProfile::Title => self.title_temperature.as_ref(),
        }
    }
}

/// The values to put on a provider request.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ResolvedSampling {
    pub reasoning_effort: Option<String>,
    pub temperature: Option<f32>,
}

/// Resolve the effective values for a call.
///
/// Precedence, per field: workspace value for the profile → env value for the
/// profile → (watch only) the chat value at the same layer order → default.
///
/// * `call_default_temperature` is the call site's historical value (0.7 chat,
///   0.1 copilot, 0.3 titles) used when nothing is configured.
/// * Reasoning effort has no default (unset = omit the field), except title
///   calls, which default to `low` when `title_model_configured` and a chat
///   effort value is configured (see [`DEFAULT_TITLE_REASONING_EFFORT`]).
///   Title calls never inherit the chat value itself.
/// * A watch value, when present, wins at *either* layer before the chat
///   value is consulted: workspace watch, env watch, workspace chat, env chat.
pub fn resolve_sampling(
    profile: LlmProfile,
    workspace: &LlmSamplingSettings,
    env: &LlmSamplingSettings,
    call_default_temperature: Option<f32>,
    title_model_configured: bool,
) -> ResolvedSampling {
    let effort = [workspace, env]
        .iter()
        .find_map(|s| s.effort_for(profile))
        .or_else(|| {
            // Watches inherit interactive chat unless they have their own value.
            (profile == LlmProfile::Watch)
                .then(|| {
                    [workspace, env]
                        .iter()
                        .find_map(|s| s.effort_for(LlmProfile::Chat))
                })
                .flatten()
        })
        .cloned();
    let reasoning_effort = match effort {
        Some(setting) => setting.into_option(),
        None if profile == LlmProfile::Title
            && title_model_configured
            && matches!(
                [workspace, env]
                    .iter()
                    .find_map(|s| s.effort_for(LlmProfile::Chat)),
                Some(Setting::Value(_))
            ) =>
        {
            Some(DEFAULT_TITLE_REASONING_EFFORT.to_string())
        }
        None => None,
    };

    let temp = [workspace, env]
        .iter()
        .find_map(|s| s.temperature_for(profile))
        .or_else(|| {
            (profile == LlmProfile::Watch)
                .then(|| {
                    [workspace, env]
                        .iter()
                        .find_map(|s| s.temperature_for(LlmProfile::Chat))
                })
                .flatten()
        })
        .cloned();
    let temperature = match temp {
        Some(setting) => setting.into_option(),
        None => call_default_temperature,
    };

    ResolvedSampling {
        reasoning_effort,
        temperature,
    }
}

/// The six `settings.custom_settings` keys, in a stable order.
pub const WORKSPACE_EFFORT_KEYS: [&str; 3] =
    ["reasoning_effort", "title_reasoning_effort", "watch_reasoning_effort"];
pub const WORKSPACE_TEMPERATURE_KEYS: [&str; 3] =
    ["temperature", "title_temperature", "watch_temperature"];

/// Text form of a configured effort for display/editing: empty when unset,
/// `omit` when explicitly omitted, otherwise the value.
pub fn effort_text(s: Option<&Setting<String>>) -> String {
    match s {
        None => String::new(),
        Some(Setting::Omit) => OMIT_KEYWORD.to_string(),
        Some(Setting::Value(v)) => v.clone(),
    }
}

/// Text form of a configured temperature (see [`effort_text`]).
pub fn temperature_text(s: Option<&Setting<f32>>) -> String {
    match s {
        None => String::new(),
        Some(Setting::Omit) => OMIT_KEYWORD.to_string(),
        Some(Setting::Value(v)) => v.to_string(),
    }
}

/// Validate user text and produce the JSON stored under an effort key.
pub fn stored_effort(raw: &str) -> Result<serde_json::Value, InvalidSetting> {
    Ok(match parse_reasoning_effort(raw)? {
        Setting::Omit => serde_json::json!(OMIT_KEYWORD),
        Setting::Value(v) => serde_json::json!(v),
    })
}

/// Validate user text and produce the JSON stored under a temperature key: a
/// JSON number, or the string `"omit"`. The number is derived from the
/// parsed value's shortest text form so `0.3` is stored as `0.3`, not the
/// widened `0.30000001192092896`.
pub fn stored_temperature(raw: &str) -> Result<serde_json::Value, InvalidSetting> {
    Ok(match parse_temperature(raw)? {
        Setting::Omit => serde_json::json!(OMIT_KEYWORD),
        Setting::Value(v) => {
            let n: f64 = v
                .to_string()
                .parse()
                .map_err(|_| InvalidSetting(format!("invalid temperature {raw:?}")))?;
            serde_json::json!(n)
        }
    })
}

/// Read the six settings from `settings.custom_settings`.
///
/// Stored shape: effort values as strings; temperatures as JSON numbers or the
/// string `"omit"`.
///
/// **Lenient by design.** A stored value that fails validation (hand-edited,
/// or written under an older/looser rule) is dropped and reported in the
/// returned warnings, never an error: one bad key must not take down every LLM
/// call for the workspace, nor the settings page an admin would use to repair
/// it. A dropped value behaves as unset.
pub fn read_workspace_sampling(
    settings: &Option<serde_json::Value>,
) -> (LlmSamplingSettings, Vec<String>) {
    let get = |key: &str| {
        settings
            .as_ref()
            .and_then(|s| s.get("custom_settings"))
            .and_then(|cs| cs.get(key))
            .filter(|v| !v.is_null())
    };
    let mut warnings = Vec::new();
    let mut effort = |key: &str| -> Option<Setting<String>> {
        let v = get(key)?;
        let parsed = match v.as_str() {
            Some(s) => parse_reasoning_effort(s),
            None => Err(InvalidSetting("must be a string".into())),
        };
        parsed
            .map_err(|e| warnings.push(format!("{key}: {e}")))
            .ok()
    };
    let reasoning_effort = effort("reasoning_effort");
    let title_reasoning_effort = effort("title_reasoning_effort");
    let watch_reasoning_effort = effort("watch_reasoning_effort");
    let mut temp = |key: &str| -> Option<Setting<f32>> {
        let v = get(key)?;
        let parsed = match (v.as_f64(), v.as_str()) {
            (Some(n), _) => parse_temperature(&n.to_string()),
            (None, Some(s)) => parse_temperature(s),
            _ => Err(InvalidSetting(format!(
                "must be a number or \"{OMIT_KEYWORD}\""
            ))),
        };
        parsed
            .map_err(|e| warnings.push(format!("{key}: {e}")))
            .ok()
    };
    let temperature = temp("temperature");
    let title_temperature = temp("title_temperature");
    let watch_temperature = temp("watch_temperature");
    (
        LlmSamplingSettings {
            reasoning_effort,
            title_reasoning_effort,
            watch_reasoning_effort,
            temperature,
            title_temperature,
            watch_temperature,
        },
        warnings,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &str) -> Option<Setting<String>> {
        Some(Setting::Value(s.into()))
    }

    #[test]
    fn effort_passes_unrecognised_and_numeric_values_unchanged() {
        assert_eq!(parse_reasoning_effort("ultra-5.1").unwrap(), Setting::Value("ultra-5.1".into()));
        assert_eq!(parse_reasoning_effort(" 42 ").unwrap(), Setting::Value("42".into()));
        assert_eq!(parse_reasoning_effort("OMIT").unwrap(), Setting::Omit);
    }

    #[test]
    fn effort_rejects_empty_long_and_injection_like_values() {
        assert!(parse_reasoning_effort("  ").is_err());
        assert!(parse_reasoning_effort(&"a".repeat(33)).is_err());
        assert!(parse_reasoning_effort("high\",\"x\":1").is_err());
        assert!(parse_reasoning_effort("a b").is_err());
    }

    #[test]
    fn temperature_parses_number_and_omit_and_rejects_garbage() {
        assert_eq!(parse_temperature("0.3").unwrap(), Setting::Value(0.3));
        assert_eq!(parse_temperature("omit").unwrap(), Setting::Omit);
        assert!(parse_temperature("hot").is_err());
        assert!(parse_temperature("2.5").is_err());
        assert!(parse_temperature("-0.1").is_err());
        assert!(parse_temperature("NaN").is_err());
    }

    #[test]
    fn unset_keeps_historical_defaults() {
        let none = LlmSamplingSettings::default();
        let r = resolve_sampling(LlmProfile::Chat, &none, &none, Some(0.7), false);
        assert_eq!(r, ResolvedSampling { reasoning_effort: None, temperature: Some(0.7) });
        let r = resolve_sampling(LlmProfile::Chat, &none, &none, Some(0.1), false);
        assert_eq!(r.temperature, Some(0.1));
    }

    #[test]
    fn workspace_overrides_env_and_omit_is_explicit() {
        let env = LlmSamplingSettings {
            reasoning_effort: v("high"),
            temperature: Some(Setting::Value(0.5)),
            ..Default::default()
        };
        let ws = LlmSamplingSettings {
            reasoning_effort: v("low"),
            temperature: Some(Setting::Omit),
            ..Default::default()
        };
        let r = resolve_sampling(LlmProfile::Chat, &ws, &env, Some(0.7), false);
        assert_eq!(r.reasoning_effort.as_deref(), Some("low"));
        assert_eq!(r.temperature, None);
        let r = resolve_sampling(LlmProfile::Chat, &LlmSamplingSettings::default(), &env, Some(0.7), false);
        assert_eq!(r.reasoning_effort.as_deref(), Some("high"));
        assert_eq!(r.temperature, Some(0.5));
    }

    #[test]
    fn watch_inherits_chat_but_can_differ() {
        let env = LlmSamplingSettings {
            reasoning_effort: v("high"),
            watch_reasoning_effort: v("low"),
            temperature: Some(Setting::Value(0.9)),
            ..Default::default()
        };
        let none = LlmSamplingSettings::default();
        let w = resolve_sampling(LlmProfile::Watch, &none, &env, Some(0.7), false);
        assert_eq!(w.reasoning_effort.as_deref(), Some("low"));
        assert_eq!(w.temperature, Some(0.9));
        let c = resolve_sampling(LlmProfile::Chat, &none, &env, Some(0.7), false);
        assert_eq!(c.reasoning_effort.as_deref(), Some("high"));
    }

    #[test]
    fn title_defaults_low_only_with_a_title_model_and_a_configured_chat_effort() {
        let none = LlmSamplingSettings::default();
        let env = LlmSamplingSettings { reasoning_effort: v("high"), ..Default::default() };
        // Title model + chat effort configured: titles default to low, never inherit high.
        let t = resolve_sampling(LlmProfile::Title, &none, &env, Some(0.3), true);
        assert_eq!(t.reasoning_effort.as_deref(), Some("low"));
        // No title model: no implicit default and the chat effort is not inherited.
        let t = resolve_sampling(LlmProfile::Title, &none, &env, Some(0.3), false);
        assert_eq!(t.reasoning_effort, None);
        // Existing install: a title model but no effort configured anywhere sends nothing,
        // so a non-reasoning title model never receives a field it may reject.
        let t = resolve_sampling(LlmProfile::Title, &none, &none, Some(0.3), true);
        assert_eq!(t.reasoning_effort, None);
        // Chat effort explicitly omitted does not enable the default either.
        let omit = LlmSamplingSettings { reasoning_effort: Some(Setting::Omit), ..Default::default() };
        let t = resolve_sampling(LlmProfile::Title, &none, &omit, Some(0.3), true);
        assert_eq!(t.reasoning_effort, None);
        // Explicit title values always win.
        let explicit_omit = LlmSamplingSettings { title_reasoning_effort: Some(Setting::Omit), ..Default::default() };
        let t = resolve_sampling(LlmProfile::Title, &explicit_omit, &env, Some(0.3), true);
        assert_eq!(t.reasoning_effort, None);
        let none_effort = LlmSamplingSettings { title_reasoning_effort: v("none"), ..Default::default() };
        let t = resolve_sampling(LlmProfile::Title, &none_effort, &env, Some(0.3), true);
        assert_eq!(t.reasoning_effort.as_deref(), Some("none"));
    }

    #[test]
    fn watch_precedence_is_workspace_watch_env_watch_workspace_chat_env_chat() {
        let ws = LlmSamplingSettings { reasoning_effort: v("ws-chat"), ..Default::default() };
        let env = LlmSamplingSettings {
            reasoning_effort: v("env-chat"),
            watch_reasoning_effort: v("env-watch"),
            ..Default::default()
        };
        // An instance watch value beats a workspace chat value.
        let r = resolve_sampling(LlmProfile::Watch, &ws, &env, Some(0.7), false);
        assert_eq!(r.reasoning_effort.as_deref(), Some("env-watch"));
        let ws_watch = LlmSamplingSettings { watch_reasoning_effort: v("ws-watch"), ..ws.clone() };
        let r = resolve_sampling(LlmProfile::Watch, &ws_watch, &env, Some(0.7), false);
        assert_eq!(r.reasoning_effort.as_deref(), Some("ws-watch"));
        let env_chat_only = LlmSamplingSettings { reasoning_effort: v("env-chat"), ..Default::default() };
        let r = resolve_sampling(LlmProfile::Watch, &ws, &env_chat_only, Some(0.7), false);
        assert_eq!(r.reasoning_effort.as_deref(), Some("ws-chat"));
    }

    #[test]
    fn reads_workspace_settings() {
        let s = Some(serde_json::json!({"custom_settings": {
            "reasoning_effort": "medium", "temperature": 0.25,
            "title_temperature": "omit", "watch_reasoning_effort": "7"
        }}));
        let (r, warnings) = read_workspace_sampling(&s);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(r.reasoning_effort, v("medium"));
        assert_eq!(r.temperature, Some(Setting::Value(0.25)));
        assert_eq!(r.title_temperature, Some(Setting::Omit));
        assert_eq!(r.watch_reasoning_effort, v("7"));
        assert_eq!(read_workspace_sampling(&None), (LlmSamplingSettings::default(), vec![]));
    }

    #[test]
    fn invalid_stored_values_are_dropped_with_a_warning_not_an_error() {
        let s = Some(serde_json::json!({"custom_settings": {
            "temperature": "hot",
            "reasoning_effort": 5,
            "title_reasoning_effort": "has space",
            "watch_temperature": 9.0,
            "title_temperature": 0.4
        }}));
        let (r, warnings) = read_workspace_sampling(&s);
        assert_eq!(warnings.len(), 4, "{warnings:?}");
        for key in ["temperature:", "reasoning_effort:", "title_reasoning_effort:", "watch_temperature:"] {
            assert!(warnings.iter().any(|w| w.starts_with(key)), "{key} in {warnings:?}");
        }
        // Bad keys behave as unset; the good key survives.
        assert_eq!(r.temperature, None);
        assert_eq!(r.reasoning_effort, None);
        assert_eq!(r.title_temperature, Some(Setting::Value(0.4)));
    }

    #[test]
    fn stored_forms_round_trip_through_the_reader() {
        let settings = Some(serde_json::json!({"custom_settings": {
            "reasoning_effort": stored_effort(" High ").unwrap(),
            "watch_reasoning_effort": stored_effort("omit").unwrap(),
            "temperature": stored_temperature("0.3").unwrap(),
            "title_temperature": stored_temperature("OMIT").unwrap(),
        }}));
        // 0.3 must be stored as the f64 0.3, not the widened f32 value.
        assert_eq!(settings.as_ref().unwrap()["custom_settings"]["temperature"], serde_json::json!(0.3));
        let (r, warnings) = read_workspace_sampling(&settings);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(effort_text(r.reasoning_effort.as_ref()), "High");
        assert_eq!(effort_text(r.watch_reasoning_effort.as_ref()), "omit");
        assert_eq!(effort_text(r.title_reasoning_effort.as_ref()), "");
        assert_eq!(temperature_text(r.temperature.as_ref()), "0.3");
        assert_eq!(temperature_text(r.title_temperature.as_ref()), "omit");
        assert!(stored_effort("a b").is_err());
        assert!(stored_temperature("3").is_err());
    }
}
