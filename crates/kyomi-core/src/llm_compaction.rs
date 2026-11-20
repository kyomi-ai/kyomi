// SPDX-License-Identifier: AGPL-3.0-or-later

//! Conversation compaction threshold (KYO-975).
//!
//! Compaction summarises older turns once the input approaches the model's
//! context window. The threshold is derived from the **resolved context
//! window** (KYO-974) and a fraction of it, and can be overridden:
//!
//! * **absolute** — a token count; wins over every fraction. Instance:
//!   `LLM_COMPACTION_THRESHOLD`; workspace: `compaction_threshold`.
//! * **fraction** — `0 < fraction <= 1` of the context window. Instance:
//!   `LLM_COMPACTION_FRACTION` (default [`DEFAULT_COMPACTION_FRACTION`]);
//!   workspace: `compaction_fraction`.
//!
//! Precedence: workspace absolute → instance absolute → workspace fraction →
//! instance fraction → the default fraction. An absolute value at either layer
//! beats a fraction at either layer.
//!
//! When a provider does not report a window for the model, the threshold uses
//! [`DEFAULT_CONTEXT_WINDOW`] and says so in its [`ContextWindowSource`], so a
//! 1M-window model is never silently treated as 200K.

use serde_json::Value;

use crate::llm_sampling::InvalidSetting;

/// Fraction of the context window at which compaction triggers when neither
/// layer configures one. 0.75 keeps the historical 150,000-token threshold for
/// a 200K window.
pub const DEFAULT_COMPACTION_FRACTION: f64 = 0.75;

/// Context window assumed when the provider does not report one for the model
/// (OpenAI-compatible endpoints and Gemini report none). The same 200K value
/// the Anthropic table uses for models it does not recognise.
pub const DEFAULT_CONTEXT_WINDOW: u32 = 200_000;

/// Workspace `settings.custom_settings` keys.
pub const WORKSPACE_FRACTION_KEY: &str = "compaction_fraction";
pub const WORKSPACE_THRESHOLD_KEY: &str = "compaction_threshold";

/// Where the context window used for the threshold came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextWindowSource {
    /// The provider reported the model's own window.
    Provider,
    /// The provider reported none; [`DEFAULT_CONTEXT_WINDOW`] was used.
    Default,
}

impl ContextWindowSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Provider => "provider",
            Self::Default => "default",
        }
    }
}

/// Which layer supplied the threshold.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThresholdSource {
    WorkspaceAbsolute,
    InstanceAbsolute,
    WorkspaceFraction,
    InstanceFraction,
    DefaultFraction,
}

impl ThresholdSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::WorkspaceAbsolute => "workspace_absolute",
            Self::InstanceAbsolute => "instance_absolute",
            Self::WorkspaceFraction => "workspace_fraction",
            Self::InstanceFraction => "instance_fraction",
            Self::DefaultFraction => "default_fraction",
        }
    }
}

/// Compaction settings at one layer (workspace or instance). Unset fields fall
/// through to the next layer.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct LlmCompactionSettings {
    /// Fraction of the context window at which to compact, `0 < f <= 1`.
    pub fraction: Option<f64>,
    /// Absolute token threshold; wins over every fraction.
    pub threshold_tokens: Option<u32>,
}

/// The window and threshold a run uses, with the source of each so the run can
/// log them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResolvedCompaction {
    pub context_window: u32,
    pub context_window_source: ContextWindowSource,
    pub threshold_tokens: u32,
    pub threshold_source: ThresholdSource,
}

/// Parse a configured fraction. A fraction is a decimal in `(0, 1]`; anything
/// else is rejected rather than guessed at.
pub fn parse_fraction(raw: &str) -> Result<f64, InvalidSetting> {
    let v = raw.trim();
    let f = v
        .parse::<f64>()
        .map_err(|_| InvalidSetting(format!("expected a number between 0 and 1, got {v:?}")))?;
    if !f.is_finite() || f <= 0.0 || f > 1.0 {
        return Err(InvalidSetting(format!(
            "expected a number greater than 0 and at most 1, got {v:?}"
        )));
    }
    Ok(f)
}

/// Parse a configured absolute threshold: a positive token count.
pub fn parse_threshold(raw: &str) -> Result<u32, InvalidSetting> {
    let v = raw.trim();
    v.parse::<u32>()
        .ok()
        .filter(|n| *n > 0)
        .ok_or_else(|| InvalidSetting(format!("expected a positive token count, got {v:?}")))
}

/// Resolve the threshold for a run: workspace absolute, then instance
/// absolute, then workspace fraction, then instance fraction, then the
/// default fraction.
///
/// `provider_reported_window` is the provider's raw window and
/// `context_window_source` its own label; a zero window is replaced by the
/// documented default here. Anthropic reports its table's 200K fallback for an
/// unrecognised model, so a plain non-zero value cannot distinguish a real 200K
/// model from the default; the provider's own window-source method supplies the
/// correct label for the run log.
pub fn resolve_with_source(
    provider_reported_window: u32,
    context_window_source: ContextWindowSource,
    workspace: &LlmCompactionSettings,
    instance: &LlmCompactionSettings,
) -> ResolvedCompaction {
    // A provider that reports no window cannot supply a threshold: substitute
    // the documented default here, so no caller can derive 0 × fraction = 1
    // (KYO-975 review). The source then necessarily describes the default.
    let (context_window, context_window_source) = if provider_reported_window == 0 {
        (DEFAULT_CONTEXT_WINDOW, ContextWindowSource::Default)
    } else {
        (provider_reported_window, context_window_source)
    };
    let (threshold_tokens, threshold_source) = if let Some(tokens) = workspace.threshold_tokens {
        (tokens, ThresholdSource::WorkspaceAbsolute)
    } else if let Some(tokens) = instance.threshold_tokens {
        (tokens, ThresholdSource::InstanceAbsolute)
    } else if let Some(fraction) = workspace.fraction {
        (derive(context_window, fraction), ThresholdSource::WorkspaceFraction)
    } else if let Some(fraction) = instance.fraction {
        (derive(context_window, fraction), ThresholdSource::InstanceFraction)
    } else {
        (
            derive(context_window, DEFAULT_COMPACTION_FRACTION),
            ThresholdSource::DefaultFraction,
        )
    };
    ResolvedCompaction {
        context_window,
        context_window_source,
        threshold_tokens,
        threshold_source,
    }
}

/// A fraction of the window, rounded down, never zero.
fn derive(window: u32, fraction: f64) -> u32 {
    let tokens = (f64::from(window) * fraction).floor();
    tokens.clamp(1.0, f64::from(u32::MAX)) as u32
}

/// The instance settings from the process environment (or a map in tests).
///
/// An invalid value aborts startup naming the variable, matching
/// `LLM_CONTEXT_WINDOW`; an empty value is unset.
pub fn from_env(get: impl Fn(&str) -> Option<String>) -> LlmCompactionSettings {
    let parsed = |key: &str| -> Option<String> {
        match get(key) {
            Some(raw) if raw.trim().is_empty() => None,
            Some(raw) => Some(raw),
            None => None,
        }
    };
    let fraction = parsed("LLM_COMPACTION_FRACTION")
        .map(|raw| parse_fraction(&raw).unwrap_or_else(|e| panic!("LLM_COMPACTION_FRACTION: {e}")));
    let threshold_tokens = parsed("LLM_COMPACTION_THRESHOLD").map(|raw| {
        parse_threshold(&raw).unwrap_or_else(|e| panic!("LLM_COMPACTION_THRESHOLD: {e}"))
    });
    LlmCompactionSettings {
        fraction,
        threshold_tokens,
    }
}

/// Read the two settings from `settings.custom_settings`.
///
/// Lenient like the LLM tuning reader: an invalid stored value is dropped and
/// reported in the returned warnings, never an error. A dropped value behaves
/// as unset, so the instance layer or the default applies.
pub fn read_workspace(settings: &Option<Value>) -> (LlmCompactionSettings, Vec<String>) {
    let get = |key: &str| {
        settings
            .as_ref()
            .and_then(|s| s.get("custom_settings"))
            .and_then(|cs| cs.get(key))
            .filter(|v| !v.is_null())
    };
    let mut warnings = Vec::new();
    let fraction = get(WORKSPACE_FRACTION_KEY).and_then(|v| match v {
        Value::Number(n) => n
            .as_f64()
            .filter(|f| f.is_finite() && *f > 0.0 && *f <= 1.0)
            .or_else(|| {
                warnings.push(format!(
                    "{WORKSPACE_FRACTION_KEY}: expected a number greater than 0 and at most 1 (stored: {v})"
                ));
                None
            }),
        _ => {
            warnings.push(format!(
                "{WORKSPACE_FRACTION_KEY}: expected a number greater than 0 and at most 1 (stored: {v})"
            ));
            None
        }
    });
    let threshold_tokens = get(WORKSPACE_THRESHOLD_KEY).and_then(|v| match v {
        Value::Number(n) => n
            .as_u64()
            .filter(|n| *n > 0 && *n <= u64::from(u32::MAX))
            .map(|n| n as u32)
            .or_else(|| {
                warnings.push(format!(
                    "{WORKSPACE_THRESHOLD_KEY}: expected a positive token count (stored: {v})"
                ));
                None
            }),
        _ => {
            warnings.push(format!(
                "{WORKSPACE_THRESHOLD_KEY}: expected a positive token count (stored: {v})"
            ));
            None
        }
    });
    (
        LlmCompactionSettings {
            fraction,
            threshold_tokens,
        },
        warnings,
    )
}

/// The JSON stored under the fraction key.
pub fn stored_fraction(raw: &str) -> Result<Value, InvalidSetting> {
    parse_fraction(raw).map(|f| Value::Number(serde_json::Number::from_f64(f).expect("finite")))
}

/// The JSON stored under the absolute-threshold key.
pub fn stored_threshold(raw: &str) -> Result<Value, InvalidSetting> {
    parse_threshold(raw).map(|n| Value::Number(n.into()))
}

/// Display form of a configured fraction: empty when unset.
pub fn fraction_text(fraction: Option<f64>) -> String {
    fraction.map(|f| format!("{f}")).unwrap_or_default()
}

/// Display form of a configured absolute threshold: empty when unset.
pub fn threshold_text(tokens: Option<u32>) -> String {
    tokens.map(|n| n.to_string()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn settings(fraction: Option<f64>, threshold_tokens: Option<u32>) -> LlmCompactionSettings {
        LlmCompactionSettings {
            fraction,
            threshold_tokens,
        }
    }

    /// Test helper: the provider's label for a raw window (zero = default).
    fn resolve(
        window: u32,
        workspace: &LlmCompactionSettings,
        instance: &LlmCompactionSettings,
    ) -> ResolvedCompaction {
        let source = if window == 0 {
            ContextWindowSource::Default
        } else {
            ContextWindowSource::Provider
        };
        resolve_with_source(window, source, workspace, instance)
    }

    #[test]
    fn parses_a_fraction_between_zero_and_one() {
        assert_eq!(parse_fraction("0.75").unwrap(), 0.75);
        assert_eq!(parse_fraction(" 1 ").unwrap(), 1.0);
        assert_eq!(parse_fraction("0.5").unwrap(), 0.5);
        assert!(parse_fraction("0").is_err());
        assert!(parse_fraction("-0.1").is_err());
        assert!(parse_fraction("1.01").is_err());
        assert!(parse_fraction("NaN").is_err());
        assert!(parse_fraction("inf").is_err());
        assert!(parse_fraction("").is_err());
        assert!(parse_fraction("75%").is_err());
    }

    #[test]
    fn parses_a_positive_absolute_threshold() {
        assert_eq!(parse_threshold("150000").unwrap(), 150_000);
        assert_eq!(parse_threshold(" 1 ").unwrap(), 1);
        assert!(parse_threshold("0").is_err());
        assert!(parse_threshold("-1").is_err());
        assert!(parse_threshold("1.5").is_err());
        assert!(parse_threshold("").is_err());
    }

    #[test]
    fn defaults_derive_the_historical_threshold_for_a_200k_model() {
        let r = resolve(200_000, &settings(None, None), &settings(None, None));
        assert_eq!(r.context_window, 200_000);
        assert_eq!(r.context_window_source, ContextWindowSource::Provider);
        assert_eq!(r.threshold_tokens, 150_000);
        assert_eq!(r.threshold_source, ThresholdSource::DefaultFraction);
    }

    #[test]
    fn one_million_models_derive_750k() {
        let r = resolve(1_000_000, &settings(None, None), &settings(None, None));
        assert_eq!(r.threshold_tokens, 750_000);
        assert_eq!(r.threshold_source, ThresholdSource::DefaultFraction);
    }

    #[test]
    fn unknown_window_uses_the_documented_default_and_reports_it() {
        let r = resolve(0, &settings(None, None), &settings(None, None));
        assert_eq!(r.context_window, DEFAULT_CONTEXT_WINDOW);
        assert_eq!(r.context_window_source, ContextWindowSource::Default);
        assert_eq!(r.threshold_tokens, 150_000);
    }

    #[test]
    fn a_zero_provider_window_never_derives_a_one_token_threshold() {
        // `resolve_with_source` receives the provider's raw window; a provider
        // that reports none must still get the documented default, whatever
        // source label its caller believed.
        for source in [ContextWindowSource::Provider, ContextWindowSource::Default] {
            let r = resolve_with_source(0, source, &settings(None, None), &settings(None, None));
            assert_eq!(r.context_window, DEFAULT_CONTEXT_WINDOW);
            assert_eq!(r.context_window_source, ContextWindowSource::Default);
            assert_eq!(r.threshold_tokens, 150_000, "source={source:?}");
        }
    }

    #[test]
    fn absolute_overrides_beat_fractions_at_either_layer() {
        let ws_absolute = resolve(200_000, &settings(Some(0.1), Some(90_000)), &settings(Some(0.2), Some(80_000)));
        assert_eq!((ws_absolute.threshold_tokens, ws_absolute.threshold_source), (90_000, ThresholdSource::WorkspaceAbsolute));

        let env_absolute = resolve(200_000, &settings(Some(0.1), None), &settings(Some(0.2), Some(80_000)));
        assert_eq!((env_absolute.threshold_tokens, env_absolute.threshold_source), (80_000, ThresholdSource::InstanceAbsolute));

        let ws_fraction = resolve(200_000, &settings(Some(0.1), None), &settings(Some(0.2), None));
        assert_eq!((ws_fraction.threshold_tokens, ws_fraction.threshold_source), (20_000, ThresholdSource::WorkspaceFraction));

        let env_fraction = resolve(200_000, &settings(None, None), &settings(Some(0.2), None));
        assert_eq!((env_fraction.threshold_tokens, env_fraction.threshold_source), (40_000, ThresholdSource::InstanceFraction));
    }

    #[test]
    fn a_fraction_of_one_million_is_750k_with_a_workspace_fraction() {
        let r = resolve(1_000_000, &settings(Some(0.75), None), &settings(None, None));
        assert_eq!(r.threshold_tokens, 750_000);
        assert_eq!(r.threshold_source, ThresholdSource::WorkspaceFraction);
    }

    #[test]
    fn reads_valid_workspace_values() {
        let s = Some(json!({"custom_settings": {
            "compaction_fraction": 0.6,
            "compaction_threshold": 123_456,
        }}));
        let (r, warnings) = read_workspace(&s);
        assert_eq!(r, settings(Some(0.6), Some(123_456)));
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(read_workspace(&None), (LlmCompactionSettings::default(), vec![]));
    }

    #[test]
    fn invalid_stored_values_are_dropped_with_a_warning() {
        for (value, key) in [
            (json!({"custom_settings": {"compaction_fraction": 0}}), "compaction_fraction"),
            (json!({"custom_settings": {"compaction_fraction": 1.5}}), "compaction_fraction"),
            (json!({"custom_settings": {"compaction_fraction": "0.5"}}), "compaction_fraction"),
            (json!({"custom_settings": {"compaction_threshold": 0}}), "compaction_threshold"),
            (json!({"custom_settings": {"compaction_threshold": -1}}), "compaction_threshold"),
            (json!({"custom_settings": {"compaction_threshold": 1.5}}), "compaction_threshold"),
        ] {
            let (r, warnings) = read_workspace(&Some(value));
            assert_eq!(r, LlmCompactionSettings::default(), "{key}");
            assert_eq!(warnings.len(), 1, "{key}: {warnings:?}");
            assert!(warnings[0].starts_with(key), "{key}: {warnings:?}");
        }
    }

    #[test]
    fn stored_values_round_trip_through_the_reader() {
        let stored = settings(stored_fraction("0.5").unwrap().as_f64(), stored_threshold("1000").unwrap().as_u64().map(|n| n as u32));
        assert_eq!(stored, settings(Some(0.5), Some(1000)));
        assert_eq!(fraction_text(Some(0.75)), "0.75");
        assert_eq!(fraction_text(Some(1.0)), "1");
        assert_eq!(fraction_text(None), "");
        assert_eq!(threshold_text(Some(1000)), "1000");
        assert_eq!(threshold_text(None), "");
    }

    #[test]
    fn env_parsing_uses_both_variables_and_rejects_invalid_values() {
        let both = from_env(|key| match key {
            "LLM_COMPACTION_FRACTION" => Some("0.5".into()),
            "LLM_COMPACTION_THRESHOLD" => Some("90000".into()),
            _ => None,
        });
        assert_eq!(both, settings(Some(0.5), Some(90_000)));
        let empty = from_env(|_| None);
        assert_eq!(empty, LlmCompactionSettings::default());
        let blank = from_env(|key| (key == "LLM_COMPACTION_FRACTION").then(|| " ".into()));
        assert_eq!(blank, LlmCompactionSettings::default());
        assert!(std::panic::catch_unwind(|| from_env(|key| (key == "LLM_COMPACTION_FRACTION").then(|| "2".into()))).is_err());
        assert!(std::panic::catch_unwind(|| from_env(|key| (key == "LLM_COMPACTION_THRESHOLD").then(|| "0".into()))).is_err());
    }

    #[test]
    fn a_fraction_never_derives_zero() {
        let r = resolve(1, &settings(Some(0.001), None), &settings(None, None));
        assert_eq!(r.threshold_tokens, 1);
    }
}
