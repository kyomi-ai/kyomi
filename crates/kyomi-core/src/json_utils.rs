// SPDX-License-Identifier: AGPL-3.0-or-later

//! JSON value helpers for configuration parsing.

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
/// datasets" enabled.
///
/// An **absent** key means disabled — this matches the settings UI toggle's
/// load-back default, and every other reader of `include_public_datasets`
/// must route through this function (or, where Rust isn't reachable, mirror
/// its `false` default explicitly) so the default cannot drift between call
/// sites again. Five independent copies of this default disagreeing is
/// exactly what produced the KYO-446 leak: public BigQuery datasets stayed
/// visible in the SQL editor and the agent even with the toggle switched off,
/// because every consumer but the UI itself defaulted an absent key to `true`.
///
/// The knowledge-search consumer (`kyomi-agent/src/tools/knowledge.rs`) can't
/// call this directly — it decides whether to run a second query from inside
/// a SQL `COALESCE`, not from Rust — so it mirrors the `false` default there
/// instead. Keep that `COALESCE` in step with this function by hand.
pub fn bigquery_include_public(connection_config: &serde_json::Value) -> bool {
    config_bool(connection_config.get("include_public_datasets"), false)
}

#[cfg(test)]
mod tests {
    use super::*;

    // KYO-446: the whole fix hinges on this function being the single place
    // that decides what an absent key means. Collapsing "absent" into
    // either "false" or "true" here is exactly the bug this ticket fixes,
    // so each of the three states gets its own assertion.

    #[test]
    fn bigquery_include_public_defaults_to_false_when_key_absent() {
        let config = serde_json::json!({"auth_mode": "kyomi_oauth"});
        assert!(!bigquery_include_public(&config));
    }

    #[test]
    fn bigquery_include_public_defaults_to_false_on_empty_object() {
        let config = serde_json::json!({});
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

    #[test]
    fn bigquery_include_public_accepts_legacy_string_true() {
        // KYO-21: some rows persisted "true"/"false" as JSON strings rather
        // than booleans. This helper must accept both shapes exactly like
        // `config_bool` does.
        let config = serde_json::json!({"include_public_datasets": "true"});
        assert!(bigquery_include_public(&config));
    }

    #[test]
    fn bigquery_include_public_accepts_legacy_string_false() {
        let config = serde_json::json!({"include_public_datasets": "false"});
        assert!(!bigquery_include_public(&config));
    }
}
