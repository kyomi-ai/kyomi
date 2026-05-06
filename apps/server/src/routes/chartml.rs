// SPDX-License-Identifier: AGPL-3.0-or-later

//! ChartML validation REST endpoints.
//!
//! Provides structural validation for ChartML YAML blocks:
//! - `GET  /schema`             — return the ChartML JSON schema
//! - `POST /validate`           — validate a single ChartML YAML block
//! - `POST /validate-markdown`  — extract and validate all ChartML blocks in markdown
//!
//! Validation checks: YAML parse + required `data`/`visualize` keys.
//! Full JSON Schema validation (discriminator-based) is Python-only for now.

use std::sync::LazyLock;

use axum::{
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};

static CHARTML_BLOCK_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"(?s)```chartml\n(.*?)```").expect("valid chartml regex"));
use serde_json::{json, Value};

use kyomi_auth::{dashboard_service, middleware::AuthUser};

use crate::state::AppState;

// ===========================================================================
// Router
// ===========================================================================

/// Build the `/chartml` router.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/schema", get(get_schema))
        .route("/validate", post(validate_chartml))
        .route("/validate-markdown", post(validate_markdown))
}

// ===========================================================================
// Request / Response Types
// ===========================================================================

#[derive(Deserialize)]
#[cfg_attr(test, derive(Serialize))]
struct ValidateRequest {
    /// Raw ChartML YAML string (single block, no fences).
    chartml: String,
}

#[derive(Serialize)]
#[cfg_attr(test, derive(Deserialize))]
struct ValidateResponse {
    valid: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

#[derive(Deserialize)]
#[cfg_attr(test, derive(Serialize))]
struct ValidateMarkdownRequest {
    /// Markdown content that may contain ```chartml fenced blocks.
    content: String,
}

#[derive(Serialize)]
#[cfg_attr(test, derive(Deserialize))]
struct BlockValidationResult {
    block_index: usize,
    valid: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

#[derive(Serialize)]
#[cfg_attr(test, derive(Deserialize))]
struct ValidateMarkdownResponse {
    valid: bool,
    block_count: usize,
    blocks: Vec<BlockValidationResult>,
}

// ===========================================================================
// Endpoint Handlers
// ===========================================================================

// ---------------------------------------------------------------------------
// GET /schema — Return ChartML JSON schema
// ---------------------------------------------------------------------------

async fn get_schema(
    _user: AuthUser,
) -> Result<Json<Value>, kyomi_core::Error> {
    // Return a minimal schema description. The full JSON schema is shipped
    // with the frontend; this endpoint exists for discoverability.
    Ok(Json(json!({
        "description": "ChartML v2 specification",
        "required_keys": ["data", "visualize"],
        "optional_keys": ["type", "version", "title", "style", "layout", "transform"],
        "note": "Full JSON schema available at /static/chartml-spec/chartml_schema.min.json"
    })))
}

// ---------------------------------------------------------------------------
// POST /validate — Validate a single ChartML YAML block
// ---------------------------------------------------------------------------

async fn validate_chartml(
    _user: AuthUser,
    Json(request): Json<ValidateRequest>,
) -> Result<Json<ValidateResponse>, kyomi_core::Error> {
    // Parse YAML
    let parsed: serde_yaml::Value = match serde_yaml::from_str(&request.chartml) {
        Ok(v) => v,
        Err(e) => {
            return Ok(Json(ValidateResponse {
                valid: false,
                error: Some(format!("Invalid YAML: {e}")),
            }));
        }
    };

    // Must be a mapping
    let mapping = match parsed.as_mapping() {
        Some(m) => m,
        None => {
            return Ok(Json(ValidateResponse {
                valid: false,
                error: Some("ChartML must be a YAML mapping".into()),
            }));
        }
    };

    // Check required keys
    let has_data = mapping.contains_key(serde_yaml::Value::String("data".into()));
    let has_visualize = mapping.contains_key(serde_yaml::Value::String("visualize".into()));

    if !has_data {
        return Ok(Json(ValidateResponse {
            valid: false,
            error: Some("Missing required 'data' key".into()),
        }));
    }
    if !has_visualize {
        return Ok(Json(ValidateResponse {
            valid: false,
            error: Some("Missing required 'visualize' key".into()),
        }));
    }

    Ok(Json(ValidateResponse {
        valid: true,
        error: None,
    }))
}

// ---------------------------------------------------------------------------
// POST /validate-markdown — Validate all ChartML blocks in markdown
// ---------------------------------------------------------------------------

async fn validate_markdown(
    _user: AuthUser,
    Json(request): Json<ValidateMarkdownRequest>,
) -> Result<Json<ValidateMarkdownResponse>, kyomi_core::Error> {
    // Extract ChartML blocks from markdown
    let mut blocks = Vec::new();
    let mut all_valid = true;

    for (idx, cap) in CHARTML_BLOCK_RE.captures_iter(&request.content).enumerate() {
        let yaml_str = &cap[1];

        // Try to parse and validate
        let result = match serde_yaml::from_str::<serde_yaml::Value>(yaml_str) {
            Err(e) => BlockValidationResult {
                block_index: idx,
                valid: false,
                error: Some(format!("Invalid YAML: {e}")),
            },
            Ok(parsed) => {
                match parsed.as_mapping() {
                    None => BlockValidationResult {
                        block_index: idx,
                        valid: false,
                        error: Some("ChartML must be a YAML mapping".into()),
                    },
                    Some(mapping) => {
                        let has_data =
                            mapping.contains_key(serde_yaml::Value::String("data".into()));
                        let has_visualize =
                            mapping.contains_key(serde_yaml::Value::String("visualize".into()));

                        if !has_data {
                            BlockValidationResult {
                                block_index: idx,
                                valid: false,
                                error: Some("Missing required 'data' key".into()),
                            }
                        } else if !has_visualize {
                            BlockValidationResult {
                                block_index: idx,
                                valid: false,
                                error: Some("Missing required 'visualize' key".into()),
                            }
                        } else {
                            BlockValidationResult {
                                block_index: idx,
                                valid: true,
                                error: None,
                            }
                        }
                    }
                }
            }
        };

        if !result.valid {
            all_valid = false;
        }
        blocks.push(result);
    }

    // Also validate the overall content (catches early errors)
    if let Err(_e) = dashboard_service::validate_dashboard_content(&request.content) {
        all_valid = false;
    }

    Ok(Json(ValidateMarkdownResponse {
        valid: all_valid,
        block_count: blocks.len(),
        blocks,
    }))
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // -----------------------------------------------------------------------
    // ValidateRequest
    // -----------------------------------------------------------------------

    #[test]
    fn validate_request_deserializes() {
        let json = json!({"chartml": "data:\n  datasource: test\nvisualize:\n  type: bar"});
        let req: ValidateRequest = serde_json::from_value(json).unwrap();
        assert!(req.chartml.contains("data:"));
    }

    #[test]
    fn validate_request_fails_without_chartml() {
        let json = json!({});
        assert!(serde_json::from_value::<ValidateRequest>(json).is_err());
    }

    // -----------------------------------------------------------------------
    // ValidateResponse
    // -----------------------------------------------------------------------

    #[test]
    fn validate_response_valid() {
        let response = ValidateResponse {
            valid: true,
            error: None,
        };
        let json = serde_json::to_value(&response).unwrap();
        assert!(json["valid"].as_bool().unwrap());
        // error should be skipped when None
        assert!(json.get("error").is_none());
    }

    #[test]
    fn validate_response_invalid() {
        let response = ValidateResponse {
            valid: false,
            error: Some("Missing 'data' key".into()),
        };
        let json = serde_json::to_value(&response).unwrap();
        assert!(!json["valid"].as_bool().unwrap());
        assert_eq!(json["error"], "Missing 'data' key");
    }

    // -----------------------------------------------------------------------
    // ValidateMarkdownRequest
    // -----------------------------------------------------------------------

    #[test]
    fn validate_markdown_request_deserializes() {
        let json = json!({"content": "# Title\n\n```chartml\ndata:\n  x: 1\n```"});
        let req: ValidateMarkdownRequest = serde_json::from_value(json).unwrap();
        assert!(req.content.contains("chartml"));
    }

    // -----------------------------------------------------------------------
    // ValidateMarkdownResponse
    // -----------------------------------------------------------------------

    #[test]
    fn validate_markdown_response_serializes() {
        let response = ValidateMarkdownResponse {
            valid: true,
            block_count: 2,
            blocks: vec![
                BlockValidationResult {
                    block_index: 0,
                    valid: true,
                    error: None,
                },
                BlockValidationResult {
                    block_index: 1,
                    valid: true,
                    error: None,
                },
            ],
        };

        let json = serde_json::to_value(&response).unwrap();
        assert!(json["valid"].as_bool().unwrap());
        assert_eq!(json["block_count"], 2);
        assert_eq!(json["blocks"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn validate_markdown_response_with_errors() {
        let response = ValidateMarkdownResponse {
            valid: false,
            block_count: 1,
            blocks: vec![BlockValidationResult {
                block_index: 0,
                valid: false,
                error: Some("Missing 'visualize' key".into()),
            }],
        };

        let json = serde_json::to_value(&response).unwrap();
        assert!(!json["valid"].as_bool().unwrap());
        assert_eq!(json["blocks"][0]["error"], "Missing 'visualize' key");
    }

    #[test]
    fn validate_markdown_response_round_trip() {
        let response = ValidateMarkdownResponse {
            valid: true,
            block_count: 0,
            blocks: vec![],
        };

        let json_str = serde_json::to_string(&response).unwrap();
        let deserialized: ValidateMarkdownResponse = serde_json::from_str(&json_str).unwrap();
        assert!(deserialized.valid);
        assert_eq!(deserialized.block_count, 0);
    }
}
