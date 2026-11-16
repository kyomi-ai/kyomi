// SPDX-License-Identifier: AGPL-3.0-or-later

//! `LLM_*` reasoning-effort / temperature parsing (KYO-972).

use std::collections::HashMap;

use crate::config::llm_sampling_from;
use crate::llm_sampling::LlmSamplingSettings;
use serde_json::json;

fn from(pairs: &[(&str, &str)]) -> LlmSamplingSettings {
    let map: HashMap<String, String> = pairs
        .iter()
        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
        .collect();
    llm_sampling_from(|k| map.get(k).cloned())
}

#[test]
fn nothing_set_is_all_unset() {
    assert_eq!(from(&[]), LlmSamplingSettings::default());
}

#[test]
fn empty_values_are_unset() {
    let s = from(&[("LLM_REASONING_EFFORT", "  "), ("LLM_TEMPERATURE", "")]);
    assert_eq!(s, LlmSamplingSettings::default());
}

#[test]
fn all_six_variables_are_read_into_their_own_field() {
    let s = from(&[
        ("LLM_REASONING_EFFORT", "high"),
        ("LLM_TITLE_REASONING_EFFORT", "none"),
        ("LLM_WATCH_REASONING_EFFORT", "50"),
        ("LLM_TEMPERATURE", "0.4"),
        ("LLM_TITLE_TEMPERATURE", "0.5"),
        ("LLM_WATCH_TEMPERATURE", "1"),
    ]);
    assert_eq!(s.reasoning_effort, Some("high".to_string()));
    assert_eq!(s.title_reasoning_effort, Some("none".to_string()));
    assert_eq!(s.watch_reasoning_effort, Some("50".to_string()));
    assert_eq!(s.temperature, Some(json!(0.4)));
    assert_eq!(s.title_temperature, Some(json!(0.5)));
    assert_eq!(s.watch_temperature, Some(json!(1.0)));
}

/// Unset => nothing; set => passed through as given (a number as a JSON
/// number, anything else verbatim, never validated or clamped).
#[test]
fn temperature_is_passed_through_as_set() {
    let t = |v: &str| from(&[("LLM_TEMPERATURE", v)]).temperature;
    assert_eq!(from(&[]).temperature, None);
    assert_eq!(t("0.5"), Some(json!(0.5)));
    assert_eq!(t("1"), Some(json!(1.0)));
    assert_eq!(t("5"), Some(json!(5.0)), "no range validation");
    assert_eq!(t("hot"), Some(json!("hot")), "non-numeric is sent verbatim");
    assert_eq!(t("omit"), Some(json!("omit")), "omit is no longer a keyword");
}

#[test]
#[should_panic(expected = "LLM_TITLE_REASONING_EFFORT")]
fn invalid_effort_aborts_startup_naming_the_variable() {
    from(&[("LLM_TITLE_REASONING_EFFORT", "a b")]);
}

// -- LLM_CONTEXT_WINDOW (KYO-974) -------------------------------------------

fn window(value: Option<&str>) -> u32 {
    crate::config::context_window_from(|k| {
        (k == "LLM_CONTEXT_WINDOW").then(|| value.map(str::to_string)).flatten()
    })
}

#[test]
fn context_window_unset_or_empty_is_zero() {
    assert_eq!(window(None), 0);
    assert_eq!(window(Some("  ")), 0);
}

#[test]
fn context_window_is_read_in_tokens() {
    assert_eq!(window(Some("1000000")), 1_000_000);
    assert_eq!(window(Some(" 200000 ")), 200_000);
}

#[test]
#[should_panic(expected = "LLM_CONTEXT_WINDOW")]
fn context_window_zero_or_garbage_aborts_startup() {
    window(Some("lots"));
}

#[test]
fn fetched_context_window_never_overwrites_the_env_override() {
    let mut c = crate::Config { llm_context_window: 300_000, ..crate::Config::test_config() };
    c.apply_fetched_context_window(128_000);
    assert_eq!(c.llm_context_window, 300_000, "LLM_CONTEXT_WINDOW wins");

    let mut c = crate::Config { llm_context_window: 0, ..crate::Config::test_config() };
    c.apply_fetched_context_window(128_000);
    assert_eq!(c.llm_context_window, 128_000, "fetched value fills an unset window");
}
