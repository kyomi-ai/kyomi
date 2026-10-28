// SPDX-License-Identifier: AGPL-3.0-or-later
//! Secret-free tool policy contracts. Remote metadata is untrusted display data.
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Every grant acknowledges this disclosure for its exact connection/revision.
pub const MCP_DATA_DISCLOSURE: &str = "Selected conversation content and tool arguments may be sent to this external server under your personal account. Remote descriptions and annotations are untrusted; they do not authorize other destinations or sharing.";
pub const MCP_CATALOG_TTL_SECONDS: i64 = 300;
pub const MCP_MAX_CATALOG_TOOLS: usize = 256;
pub const MCP_MAX_SELECTED_TOOLS: usize = 32;
pub const MCP_MAX_ADVERTISEMENT_BYTES: usize = 32768;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PersonalMcpTool {
    /// Opaque model-visible identity; original names never become registry keys.
    pub alias: String,
    pub remote_name: String,
    pub description: String,
    pub input_schema: Value,
    pub annotations: Value,
    /// Hash of the entire remote definition, protocol and authenticated account context.
    pub revision: String,
}

/// Exact remote identity is untrusted display data, not an executable alias.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RejectedPersonalMcpTool {
    pub remote_name: String,
    pub reason: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PersonalMcpCatalog {
    pub connection_id: String,
    pub generation: i64,
    pub protocol_revision: String,
    pub catalog_id: String,
    pub refreshed_at: i64,
    pub expires_at: i64,
    pub tools: Vec<PersonalMcpTool>,
    pub rejected_tools: Vec<RejectedPersonalMcpTool>,
}

/// A grant request is an exact revision, never a remote-supplied policy suggestion.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GrantPersonalMcpTool {
    pub catalog_id: String,
    pub alias: String,
    pub revision: String,
    /// Required for every tool: hints cannot establish a trusted read-only classification.
    pub allow_automation: bool,
    pub acknowledge_data_destination: bool,
}

/// The server returns this only after checking live owner, membership, generation,
/// freshness, exact revision and durable consent. It is not a dispatch lease;
/// runtime activation additionally needs lifecycle and private-output admission.
#[derive(Clone, Debug, PartialEq)]
pub struct AuthorizedPersonalMcpTool {
    pub connection_id: String,
    pub generation: i64,
    pub catalog_id: String,
    pub tool: PersonalMcpTool,
}

/// Common provider schema subset. Unsupported semantics fail rather than being
/// silently weakened. Never resolve remote $refs or fetch schema URLs.
pub fn validate_mcp_schema(schema: &Value) -> Result<(), &'static str> {
    fn visit(node: &Value, depth: usize) -> Result<(), &'static str> {
        if depth > 16 {
            return Err("schema nesting exceeds 16 levels");
        }
        let map = node.as_object().ok_or("schema must be an object")?;
        let kind = map
            .get("type")
            .and_then(Value::as_str)
            .ok_or("schema type is required")?;
        if !matches!(
            kind,
            "object" | "array" | "string" | "integer" | "number" | "boolean"
        ) {
            return Err("schema type is unsupported");
        }
        for key in map.keys() {
            if !matches!(
                key.as_str(),
                "type"
                    | "properties"
                    | "required"
                    | "items"
                    | "enum"
                    | "description"
                    | "additionalProperties"
                    | "x-mcp-header"
            ) {
                return Err("schema keyword is unsupported; simplify the external tool schema");
            }
        }
        if let Some(description) = map.get("description")
            && description.as_str().is_none_or(|s| s.len() > 2048)
        {
            return Err("schema description exceeds limits");
        }
        if let Some(header) = map.get("x-mcp-header")
            && (!matches!(kind, "string" | "integer" | "boolean")
                || header.as_str().is_none_or(|s| {
                    s.is_empty()
                        || s.len() > 128
                        || !s
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b))
                }))
        {
            return Err("unsupported MCP header annotation");
        }
        if kind == "object" {
            let properties = map
                .get("properties")
                .map(|p| p.as_object().ok_or("properties must be an object"))
                .transpose()?;
            if let Some(properties) = properties {
                if properties.len() > 128 {
                    return Err("too many schema properties");
                }
                for (name, child) in properties {
                    if name.is_empty() || name.len() > 128 || name.chars().any(char::is_control) {
                        return Err("invalid schema property name");
                    }
                    visit(child, depth + 1)?;
                }
            }
            if let Some(required) = map.get("required") {
                let required = required.as_array().ok_or("required must be an array")?;
                let mut seen = std::collections::BTreeSet::new();
                for name in required {
                    let name = name.as_str().ok_or("required names must be strings")?;
                    if !seen.insert(name) || properties.is_none_or(|p| !p.contains_key(name)) {
                        return Err("required property is missing or duplicated");
                    }
                }
            }
            if map
                .get("additionalProperties")
                .is_some_and(|v| !v.is_boolean())
            {
                return Err("additionalProperties schemas are unsupported");
            }
            if map.contains_key("items") {
                return Err("items requires array type");
            }
        } else if map.contains_key("properties")
            || map.contains_key("required")
            || map.contains_key("additionalProperties")
        {
            return Err("object keywords require object type");
        }
        if kind == "array" {
            visit(
                map.get("items").ok_or("array items are required")?,
                depth + 1,
            )?;
        } else if map.contains_key("items") {
            return Err("items requires array type");
        }
        if let Some(values) = map.get("enum") {
            let values = values.as_array().ok_or("enum must be an array")?;
            if values.is_empty()
                || values.len() > 128
                || values.iter().any(|v| match kind {
                    "string" => !v.is_string(),
                    "integer" => !v.is_i64() && !v.is_u64(),
                    "number" => !v.is_number(),
                    "boolean" => !v.is_boolean(),
                    _ => true,
                })
            {
                return Err("enum values must match a supported scalar type");
            }
        }
        Ok(())
    }
    if schema.get("type").and_then(Value::as_str) != Some("object") {
        return Err("tool input must be an object");
    }
    visit(schema, 0)
}

/// Strip transport-only annotations for provider APIs without changing the
/// original server-side schema used for headers and revision calculation.
pub fn mcp_provider_schema(schema: &Value) -> Result<Value, &'static str> {
    validate_mcp_schema(schema)?;
    fn strip(value: &mut Value) {
        if let Some(map) = value.as_object_mut() {
            map.remove("x-mcp-header");
            if let Some(properties) = map.get_mut("properties").and_then(Value::as_object_mut) {
                for child in properties.values_mut() {
                    strip(child);
                }
            }
            if let Some(items) = map.get_mut("items") {
                strip(items);
            }
        }
    }
    let mut result = schema.clone();
    strip(&mut result);
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn schema_annotations_do_not_erase_same_named_properties() {
        let schema = json!({"type":"object","properties":{"x-mcp-header":{"type":"string","x-mcp-header":"Title"},"nested":{"type":"array","items":{"type":"object","properties":{"x":{"type":"integer"}}}}},"required":["x-mcp-header"]});
        let converted = mcp_provider_schema(&schema).unwrap();
        assert_eq!(
            converted["properties"]["x-mcp-header"],
            json!({"type":"string"})
        );
        assert_eq!(converted["required"], schema["required"]);
        assert_eq!(
            schema["properties"]["x-mcp-header"]["x-mcp-header"],
            "Title"
        );
    }
    #[test]
    fn unsupported_schemas_fail_without_silent_weakening() {
        for schema in [
            json!({"type":"object","patternProperties":{".*":{"type":"string"}}}),
            json!({"type":"object","properties":{"x":{"type":["string","null"]}}}),
            json!({"type":"object","properties":{"x":{"type":"integer","enum":["wrong"]}}}),
            json!({"type":"object","properties":{"x":{"type":"array"}}}),
            json!({"type":"object","properties":{"x":{"type":"object","items":{"type":"string"}}}}),
        ] {
            assert!(mcp_provider_schema(&schema).is_err());
        }
    }
}
