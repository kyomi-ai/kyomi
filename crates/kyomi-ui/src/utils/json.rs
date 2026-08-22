// SPDX-License-Identifier: AGPL-3.0-or-later

//! JSON value helpers for configuration parsing.
//!
//! Mirrors `kyomi_core::json_utils` for code that compiles on both
//! SSR and WASM targets (`kyomi-core` is SSR-only).

/// Read a `serde_json::Value` as a boolean, accepting both `true`/`false` JSON
/// bools and `"true"`/`"false"` JSON strings.  Returns `default` when the value
/// is `None` or any other JSON type.
pub fn config_bool(val: Option<&serde_json::Value>, default: bool) -> bool {
    match val {
        Some(serde_json::Value::Bool(b)) => *b,
        Some(serde_json::Value::String(s)) => s.eq_ignore_ascii_case("true"),
        _ => default,
    }
}

/// Whether a BigQuery datasource's `connection_config` has "Include public
/// datasets" enabled. An absent key means disabled.
///
/// Mirrors `kyomi_core::json_utils::bigquery_include_public` — that copy is
/// the source of truth for this default (see its doc comment, KYO-446); this
/// one exists only because this module also compiles on the WASM/hydrate
/// target, where `kyomi-core` (SSR-only) isn't reachable. Keep both in step.
pub fn bigquery_include_public(connection_config: &serde_json::Value) -> bool {
    config_bool(connection_config.get("include_public_datasets"), false)
}

#[cfg(test)]
mod tests {
    use super::*;

    // KYO-446: this mirror must agree with `kyomi_core::json_utils`'s copy
    // on all three states — an absent key is the case that regressed.

    #[test]
    fn bigquery_include_public_defaults_to_false_when_key_absent() {
        let config = serde_json::json!({"auth_mode": "kyomi_oauth"});
        assert!(!bigquery_include_public(&config));
    }

    #[test]
    fn bigquery_include_public_is_false_when_explicitly_false() {
        let config = serde_json::json!({"include_public_datasets": false});
        assert!(!bigquery_include_public(&config));
    }

    #[test]
    fn bigquery_include_public_is_true_when_explicitly_true() {
        let config = serde_json::json!({"include_public_datasets": true});
        assert!(bigquery_include_public(&config));
    }
}
