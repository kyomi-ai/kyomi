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
