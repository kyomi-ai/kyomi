// SPDX-License-Identifier: AGPL-3.0-or-later
use crate::{DispatchCertainty as D, Error, ErrorKind as K};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::Value;
use std::collections::BTreeSet;

pub(crate) fn header_value(value: &str) -> String {
    if value.trim() != value
        || !value
            .bytes()
            .all(|b| b == b'\t' || (0x20..=0x7e).contains(&b))
        || (value.starts_with("=?base64?") && value.ends_with("?="))
    {
        format!("=?base64?{}?=", STANDARD.encode(value))
    } else {
        value.to_owned()
    }
}
/// Validate ALL annotation locations, including annotations hidden in refs/compositions.
/// Return static property paths only; duplicate header names are case insensitive.
pub(crate) fn annotations(schema: &Value) -> Result<Vec<(String, Vec<String>)>, Error> {
    let mut result = Vec::new();
    walk(schema, &mut Vec::new(), true, &mut result, 0)?;
    let mut names = BTreeSet::new();
    for (name, _) in &result {
        if !names.insert(name.to_ascii_lowercase()) {
            return Err(Error::new(K::Protocol, D::ResponseReceived));
        }
    }
    Ok(result)
}
fn walk(
    node: &Value,
    path: &mut Vec<String>,
    reachable: bool,
    out: &mut Vec<(String, Vec<String>)>,
    depth: usize,
) -> Result<(), Error> {
    if depth > 64 {
        return Err(Error::new(K::Limit, D::ResponseReceived));
    }
    if let Some(map) = node.as_object() {
        if let Some(name) = map.get("x-mcp-header") {
            let name = name
                .as_str()
                .ok_or_else(|| Error::new(K::Protocol, D::ResponseReceived))?;
            if !reachable
                || path.is_empty()
                || name.is_empty()
                || !name
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b))
                || !matches!(
                    map.get("type").and_then(Value::as_str),
                    Some("string" | "integer" | "boolean")
                )
            {
                return Err(Error::new(K::Protocol, D::ResponseReceived));
            }
            out.push((format!("mcp-param-{name}"), path.clone()));
        }
        for (key, value) in map {
            if key == "properties" {
                if let Some(properties) = value.as_object() {
                    for (name, property) in properties {
                        path.push(name.clone());
                        walk(property, path, reachable, out, depth + 1)?;
                        path.pop();
                    }
                }
            } else if key != "x-mcp-header" {
                walk(value, path, false, out, depth + 1)?;
            }
        }
    } else if let Some(array) = node.as_array() {
        for value in array {
            walk(value, path, false, out, depth + 1)?;
        }
    }
    Ok(())
}
pub(crate) fn parameter_headers(
    schema: &Value,
    arguments: &Value,
) -> Result<Vec<(String, String)>, Error> {
    annotations(schema)?
        .into_iter()
        .filter_map(|(name, path)| {
            let mut value = arguments;
            for part in path {
                value = value.get(part)?;
            }
            if value.is_null() {
                return None;
            }
            let text = if let Some(s) = value.as_str() {
                Ok(s.to_owned())
            } else if let Some(b) = value.as_bool() {
                Ok(b.to_string())
            } else if let Some(n) = value
                .as_i64()
                .filter(|n| n.unsigned_abs() <= 9_007_199_254_740_991)
            {
                Ok(n.to_string())
            } else {
                Err(Error::new(K::Protocol, D::NotSent))
            };
            Some(text.map(|text| (name, header_value(&text))))
        })
        .collect()
}
