// SPDX-License-Identifier: AGPL-3.0-or-later

//! LLM sampling / reasoning settings (KYO-972, simplified by KYO-974).
//!
//! Two knobs are configurable per call profile (interactive chat, watches,
//! background title/summary calls), at instance level (env) and per workspace
//! (`settings.custom_settings`):
//!
//! * **reasoning effort** — an opaque string passed to the provider unchanged
//!   (`low`, `high`, `max`, `none`, `50`, a vendor-specific level, ...). It is
//!   deliberately not an enum: vocabularies differ per provider and change.
//! * **temperature** — passed through exactly as configured: a value that parses
//!   as a number is sent as a JSON number, anything else is sent verbatim for
//!   the provider to accept or reject. Kyomi does no range validation.
//!
//! **Unset means nothing is sent.** There are no built-in temperature defaults and no
//! `omit` keyword: a value that is not configured at any layer leaves the field
//! out of the request, so the provider's own default applies.
//!
use std::fmt;

use serde_json::Value;

/// Maximum length of a reasoning-effort value.
const MAX_EFFORT_LEN: usize = 32;

/// Default reasoning effort for background calls (titles, summaries) when a
/// title model is configured, no explicit title effort is set, **and** a chat
/// reasoning effort is configured.
///
/// The last condition keeps existing installs unchanged: with no effort
/// configured anywhere, nothing is sent, so a non-reasoning title model (e.g.
/// `gpt-4o-mini`, a Haiku model, a local backend) never receives a field it may
/// reject. Once an operator has configured an effort for chat, the backend is
/// known to accept the field and titles default to `low` instead of inheriting
/// the chat value.
pub const DEFAULT_TITLE_REASONING_EFFORT: &str = "low";

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

/// Parse a reasoning-effort value: 1-32 chars of `[A-Za-z0-9._-]`, so a typo
/// cannot inject arbitrary request content. The value is never rewritten.
pub fn parse_reasoning_effort(raw: &str) -> Result<String, InvalidSetting> {
    let v = raw.trim();
    if v.is_empty() {
        return Err(InvalidSetting("reasoning effort must not be empty".into()));
    }
    if v.len() > MAX_EFFORT_LEN
        || !v
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    {
        return Err(InvalidSetting(format!(
            "invalid reasoning effort {v:?}: use up to {MAX_EFFORT_LEN} characters of letters, digits, '.', '_' or '-'"
        )));
    }
    Ok(v.to_string())
}

/// Turn a configured temperature into the request value, exactly as given: a
/// finite number becomes a JSON number (APIs require one), anything else is
/// kept as the verbatim string and left for the provider to accept or reject.
/// No range check or clamping. Callers treat empty text as unset.
pub fn temperature_value(raw: &str) -> Value {
    let v = raw.trim();
    v.parse::<f64>()
        .ok()
        .and_then(serde_json::Number::from_f64)
        .map_or_else(|| Value::String(v.to_string()), Value::Number)
}

/// Settings for the three call profiles. Every field is `None` when not
/// configured, which means nothing is sent.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LlmSamplingSettings {
    pub reasoning_effort: Option<String>,
    pub title_reasoning_effort: Option<String>,
    pub watch_reasoning_effort: Option<String>,
    pub temperature: Option<Value>,
    pub title_temperature: Option<Value>,
    pub watch_temperature: Option<Value>,
}

impl LlmSamplingSettings {
    fn effort_for(&self, profile: LlmProfile) -> Option<&String> {
        match profile {
            LlmProfile::Chat => self.reasoning_effort.as_ref(),
            LlmProfile::Watch => self.watch_reasoning_effort.as_ref(),
            LlmProfile::Title => self.title_reasoning_effort.as_ref(),
        }
    }

    fn temperature_for(&self, profile: LlmProfile) -> Option<&Value> {
        match profile {
            LlmProfile::Chat => self.temperature.as_ref(),
            LlmProfile::Watch => self.watch_temperature.as_ref(),
            LlmProfile::Title => self.title_temperature.as_ref(),
        }
    }
}

/// The values to put on a provider request; `None` leaves the field out.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ResolvedSampling {
    pub reasoning_effort: Option<String>,
    pub temperature: Option<Value>,
}

/// Resolve the effective values for a call.
///
/// Precedence, per field: workspace value for the profile → env value for the
/// profile → (watch only) the chat value at the same layer order. Nothing
/// configured means nothing sent.
///
/// * Reasoning effort has no default, except title calls, which default to `low`
///   when `title_model_configured` and a chat effort value is configured (see
///   [`DEFAULT_TITLE_REASONING_EFFORT`]). Title calls never inherit the chat
///   value itself.
/// * A watch value, when present, wins at *either* layer before the chat
///   value is consulted: workspace watch, env watch, workspace chat, env chat.
pub fn resolve_sampling(
    profile: LlmProfile,
    workspace: &LlmSamplingSettings,
    env: &LlmSamplingSettings,
    title_model_configured: bool,
) -> ResolvedSampling {
    let layers = [workspace, env];
    let inherits_chat = profile == LlmProfile::Watch;

    let reasoning_effort = layers
        .iter()
        .find_map(|s| s.effort_for(profile))
        .or_else(|| {
            inherits_chat
                .then(|| layers.iter().find_map(|s| s.effort_for(LlmProfile::Chat)))
                .flatten()
        })
        .cloned()
        .or_else(|| {
            (profile == LlmProfile::Title
                && title_model_configured
                && layers.iter().any(|s| s.effort_for(LlmProfile::Chat).is_some()))
            .then(|| DEFAULT_TITLE_REASONING_EFFORT.to_string())
        });

    let temperature = layers
        .iter()
        .find_map(|s| s.temperature_for(profile))
        .or_else(|| {
            inherits_chat
                .then(|| layers.iter().find_map(|s| s.temperature_for(LlmProfile::Chat)))
                .flatten()
        })
        .cloned();

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

/// Text form of a configured effort for display/editing: empty when unset.
pub fn effort_text(s: Option<&String>) -> String {
    s.cloned().unwrap_or_default()
}

/// Text form of a configured temperature: empty when unset, otherwise the
/// number or the verbatim string.
pub fn temperature_text(s: Option<&Value>) -> String {
    match s {
        None => String::new(),
        Some(Value::String(v)) => v.clone(),
        // A whole number the user typed as `1` is shown as `1`, not `1.0`.
        Some(Value::Number(n)) => match n.as_f64() {
            Some(f) if f.fract() == 0.0 && f.abs() < 1e15 => format!("{f:.0}"),
            _ => n.to_string(),
        },
        Some(v) => v.to_string(),
    }
}

/// Validate user text and produce the JSON stored under an effort key.
pub fn stored_effort(raw: &str) -> Result<Value, InvalidSetting> {
    parse_reasoning_effort(raw).map(Value::String)
}

/// The JSON stored under a temperature key (see [`temperature_value`]).
pub fn stored_temperature(raw: &str) -> Value {
    temperature_value(raw)
}

/// The retired `omit` keyword is now an ordinary value that would be sent
/// literally (and rejected by the provider). Say so in the log, once per read, so a
/// leftover `omit` is found by whoever sees the 400s.
pub fn warn_if_legacy_omit(source: &str, value: Option<&str>) {
    if value.is_some_and(|v| v.trim().eq_ignore_ascii_case("omit")) {
        tracing::warn!(
            source,
            "the `omit` keyword was removed (KYO-974): this value is now sent to the provider literally and will be rejected; clear it to send nothing"
        );
    }
}

/// Read the six settings from `settings.custom_settings`.
///
/// Stored shape: effort values as strings; temperatures as a JSON number or
/// string, passed through as stored.
///
/// **Lenient by design.** A stored effort that fails validation (hand-edited,
/// or written under an older/looser rule) is dropped and reported in the
/// returned warnings, never an error: one bad key must not take down every LLM
/// call for the workspace, nor the settings page an admin would use to repair
/// it. A dropped value behaves as unset.
pub fn read_workspace_sampling(
    settings: &Option<Value>,
) -> (LlmSamplingSettings, Vec<String>) {
    let get = |key: &str| {
        settings
            .as_ref()
            .and_then(|s| s.get("custom_settings"))
            .and_then(|cs| cs.get(key))
            .filter(|v| !v.is_null())
    };
    let mut warnings = Vec::new();
    let mut effort = |key: &str| -> Option<String> {
        let v = get(key)?;
        let parsed = match v.as_str() {
            Some(s) => parse_reasoning_effort(s),
            None => Err(InvalidSetting("must be a string".into())),
        };
        parsed
            .map_err(|e| warnings.push(format!("{key}: {e} (stored: {v})")))
            .ok()
    };
    for key in WORKSPACE_EFFORT_KEYS {
        warn_if_legacy_omit(&format!("workspace setting {key}"), get(key).and_then(Value::as_str));
    }
    let reasoning_effort = effort("reasoning_effort");
    let title_reasoning_effort = effort("title_reasoning_effort");
    let watch_reasoning_effort = effort("watch_reasoning_effort");
    let mut temp = |key: &str| -> Option<Value> {
        let v = get(key)?;
        warn_if_legacy_omit(&format!("workspace setting {key}"), v.as_str());
        match v {
            Value::Number(_) => Some(v.clone()),
            Value::String(s) if !s.trim().is_empty() => Some(temperature_value(s)),
            _ => {
                warnings.push(format!("{key}: must be a number or a string (stored: {v})"));
                None
            }
        }
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
    use serde_json::json;

    fn e(s: &str) -> Option<String> {
        Some(s.into())
    }

    #[test]
    fn effort_passes_unrecognised_and_numeric_values_unchanged() {
        assert_eq!(parse_reasoning_effort("ultra-5.1").unwrap(), "ultra-5.1");
        assert_eq!(parse_reasoning_effort(" 42 ").unwrap(), "42");
        // `omit` is no longer a keyword: it is an ordinary (provider-judged) value.
        assert_eq!(parse_reasoning_effort("omit").unwrap(), "omit");
    }

    #[test]
    fn effort_rejects_empty_long_and_injection_like_values() {
        assert!(parse_reasoning_effort("  ").is_err());
        assert!(parse_reasoning_effort(&"a".repeat(33)).is_err());
        assert!(parse_reasoning_effort("high\",\"x\":1").is_err());
        assert!(parse_reasoning_effort("a b").is_err());
    }

    #[test]
    fn temperature_is_passed_through_as_configured() {
        assert_eq!(temperature_value("0.5"), json!(0.5));
        assert_eq!(temperature_value(" 1 "), json!(1.0));
        assert_eq!(temperature_value("0"), json!(0.0));
        // No range validation or clamping: the provider decides.
        assert_eq!(temperature_value("7"), json!(7.0));
        assert_eq!(temperature_value("-3"), json!(-3.0));
        // Non-numeric text is sent verbatim, including the old keyword.
        assert_eq!(temperature_value("omit"), json!("omit"));
        assert_eq!(temperature_value("hot"), json!("hot"));
        assert_eq!(temperature_value("NaN"), json!("NaN"));
    }

    #[test]
    fn unset_sends_nothing_on_every_profile() {
        let none = LlmSamplingSettings::default();
        for profile in [LlmProfile::Chat, LlmProfile::Watch, LlmProfile::Title] {
            let r = resolve_sampling(profile, &none, &none, true);
            assert_eq!(r, ResolvedSampling::default(), "{profile:?}");
        }
    }

    #[test]
    fn workspace_overrides_env() {
        let env = LlmSamplingSettings {
            reasoning_effort: e("high"),
            temperature: Some(json!(0.5)),
            ..Default::default()
        };
        let ws = LlmSamplingSettings {
            reasoning_effort: e("low"),
            temperature: Some(json!(1.0)),
            ..Default::default()
        };
        let r = resolve_sampling(LlmProfile::Chat, &ws, &env, false);
        assert_eq!(r.reasoning_effort.as_deref(), Some("low"));
        assert_eq!(r.temperature, Some(json!(1.0)));
        let r = resolve_sampling(LlmProfile::Chat, &LlmSamplingSettings::default(), &env, false);
        assert_eq!(r.reasoning_effort.as_deref(), Some("high"));
        assert_eq!(r.temperature, Some(json!(0.5)));
    }

    #[test]
    fn watch_inherits_chat_but_can_differ_and_title_never_inherits_temperature() {
        let env = LlmSamplingSettings {
            reasoning_effort: e("high"),
            watch_reasoning_effort: e("low"),
            temperature: Some(json!(0.9)),
            ..Default::default()
        };
        let none = LlmSamplingSettings::default();
        let w = resolve_sampling(LlmProfile::Watch, &none, &env, false);
        assert_eq!(w.reasoning_effort.as_deref(), Some("low"));
        assert_eq!(w.temperature, Some(json!(0.9)));
        let c = resolve_sampling(LlmProfile::Chat, &none, &env, false);
        assert_eq!(c.reasoning_effort.as_deref(), Some("high"));
        let t = resolve_sampling(LlmProfile::Title, &none, &env, false);
        assert_eq!(t.temperature, None, "titles never inherit the chat temperature");
    }

    #[test]
    fn title_defaults_low_only_with_a_title_model_and_a_configured_chat_effort() {
        let none = LlmSamplingSettings::default();
        let env = LlmSamplingSettings { reasoning_effort: e("high"), ..Default::default() };
        // Title model + chat effort configured: titles default to low, never inherit high.
        let t = resolve_sampling(LlmProfile::Title, &none, &env, true);
        assert_eq!(t.reasoning_effort.as_deref(), Some("low"));
        // No title model: no implicit default and the chat effort is not inherited.
        let t = resolve_sampling(LlmProfile::Title, &none, &env, false);
        assert_eq!(t.reasoning_effort, None);
        // A title model but no effort configured anywhere sends nothing.
        let t = resolve_sampling(LlmProfile::Title, &none, &none, true);
        assert_eq!(t.reasoning_effort, None);
        // Explicit title values always win.
        let none_effort = LlmSamplingSettings { title_reasoning_effort: e("none"), ..Default::default() };
        let t = resolve_sampling(LlmProfile::Title, &none_effort, &env, true);
        assert_eq!(t.reasoning_effort.as_deref(), Some("none"));
    }

    #[test]
    fn watch_precedence_is_workspace_watch_env_watch_workspace_chat_env_chat() {
        let ws = LlmSamplingSettings { reasoning_effort: e("ws-chat"), ..Default::default() };
        let env = LlmSamplingSettings {
            reasoning_effort: e("env-chat"),
            watch_reasoning_effort: e("env-watch"),
            ..Default::default()
        };
        // An instance watch value beats a workspace chat value.
        let r = resolve_sampling(LlmProfile::Watch, &ws, &env, false);
        assert_eq!(r.reasoning_effort.as_deref(), Some("env-watch"));
        let ws_watch = LlmSamplingSettings { watch_reasoning_effort: e("ws-watch"), ..ws.clone() };
        let r = resolve_sampling(LlmProfile::Watch, &ws_watch, &env, false);
        assert_eq!(r.reasoning_effort.as_deref(), Some("ws-watch"));
        let env_chat_only = LlmSamplingSettings { reasoning_effort: e("env-chat"), ..Default::default() };
        let r = resolve_sampling(LlmProfile::Watch, &ws, &env_chat_only, false);
        assert_eq!(r.reasoning_effort.as_deref(), Some("ws-chat"));
    }

    #[test]
    fn reads_workspace_settings() {
        let s = Some(json!({"custom_settings": {
            "reasoning_effort": "medium", "temperature": 0.25,
            "title_temperature": "0.5", "watch_reasoning_effort": "7"
        }}));
        let (r, warnings) = read_workspace_sampling(&s);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(r.reasoning_effort, e("medium"));
        assert_eq!(r.temperature, Some(json!(0.25)));
        assert_eq!(r.title_temperature, Some(json!(0.5)));
        assert_eq!(r.watch_reasoning_effort, e("7"));
        assert_eq!(read_workspace_sampling(&None), (LlmSamplingSettings::default(), vec![]));
    }

    #[test]
    fn invalid_stored_effort_and_non_scalar_temperature_are_dropped_with_a_warning() {
        let s = Some(json!({"custom_settings": {
            "temperature": true,
            "reasoning_effort": 5,
            "title_reasoning_effort": "has space",
            "watch_temperature": [1],
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
        assert_eq!(r.title_temperature, Some(json!(0.4)));
    }

    #[test]
    fn stored_forms_round_trip_through_the_reader() {
        let settings = Some(json!({"custom_settings": {
            "reasoning_effort": stored_effort(" High ").unwrap(),
            "temperature": stored_temperature("0.3"),
            "title_temperature": stored_temperature("warm"),
        }}));
        assert_eq!(settings.as_ref().unwrap()["custom_settings"]["temperature"], json!(0.3));
        let (r, warnings) = read_workspace_sampling(&settings);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(effort_text(r.reasoning_effort.as_ref()), "High");
        assert_eq!(effort_text(r.watch_reasoning_effort.as_ref()), "");
        assert_eq!(temperature_text(r.temperature.as_ref()), "0.3");
        assert_eq!(temperature_text(r.title_temperature.as_ref()), "warm");
        assert!(stored_effort("a b").is_err());
    }

    #[test]
    fn whole_numbers_display_without_a_trailing_point_zero() {
        assert_eq!(temperature_text(Some(&json!(1.0))), "1");
        assert_eq!(temperature_text(Some(&json!(0.0))), "0");
        assert_eq!(temperature_text(Some(&json!(0.3))), "0.3");
        assert_eq!(temperature_text(Some(&json!("hot"))), "hot");
    }
}
