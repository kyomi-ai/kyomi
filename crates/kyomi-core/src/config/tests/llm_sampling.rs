// SPDX-License-Identifier: AGPL-3.0-or-later

//! `LLM_*` reasoning-effort / temperature parsing (KYO-972).

use std::collections::HashMap;

use crate::config::llm_sampling_from;
use crate::llm_sampling::{LlmSamplingSettings, Setting};

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
fn empty_values_are_unset_not_omit() {
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
        ("LLM_TITLE_TEMPERATURE", "omit"),
        ("LLM_WATCH_TEMPERATURE", "1"),
    ]);
    assert_eq!(s.reasoning_effort, Some(Setting::Value("high".into())));
    assert_eq!(s.title_reasoning_effort, Some(Setting::Value("none".into())));
    assert_eq!(s.watch_reasoning_effort, Some(Setting::Value("50".into())));
    assert_eq!(s.temperature, Some(Setting::Value(0.4)));
    assert_eq!(s.title_temperature, Some(Setting::Omit));
    assert_eq!(s.watch_temperature, Some(Setting::Value(1.0)));
}

#[test]
#[should_panic(expected = "LLM_TEMPERATURE")]
fn invalid_temperature_aborts_startup_naming_the_variable() {
    from(&[("LLM_TEMPERATURE", "hot")]);
}

#[test]
#[should_panic(expected = "LLM_TITLE_REASONING_EFFORT")]
fn invalid_effort_aborts_startup_naming_the_variable() {
    from(&[("LLM_TITLE_REASONING_EFFORT", "a b")]);
}
