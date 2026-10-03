// SPDX-License-Identifier: AGPL-3.0-or-later

//! ChartML validation REST endpoints.
//!
//! Uses the shared authoritative JSON Schema and authorized SQL dry-run pipeline.

use axum::{
    routing::{get, post},
    extract::State,
    Json, Router,
};
use serde::{Deserialize, Serialize};

use serde_json::Value;

use kyomi_auth::middleware::AuthUser;

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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    errors: Vec<kyomi_core::chartml_validation::Diagnostic>,
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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    errors: Vec<kyomi_core::chartml_validation::Diagnostic>,
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
    Ok(Json(serde_json::from_str(kyomi_core::chartml_validation::SCHEMA).map_err(|e| kyomi_core::Error::Internal(format!("ChartML schema unavailable: {e}")))?))
}

// ---------------------------------------------------------------------------
// POST /validate — Validate a single ChartML YAML block
// ---------------------------------------------------------------------------

fn query_context(state: &AppState, user: &AuthUser) -> kyomi_agent::tools::QueryContext {
    kyomi_agent::tools::QueryContext {
        db: state.db.clone(), user_id: user.user_id.clone(), workspace_id: user.workspace.workspace_id.clone().unwrap_or_default(),
        encryption_key: state.encryption_key.clone(), config: state.config.clone(), connect_registry: Some(state.connect_registry.clone()),
    }
}
async fn validate_chartml(
    State(state): State<AppState>, user: AuthUser, Json(request): Json<ValidateRequest>,
) -> Result<Json<ValidateResponse>, kyomi_core::Error> {
    let errors = kyomi_agent::tools::query_utils::validate_chartml_complete(&query_context(&state, &user), &[&request.chartml]).await;
    Ok(Json(ValidateResponse { valid: errors.is_empty(), error: (!errors.is_empty()).then(|| errors.iter().map(ToString::to_string).collect::<Vec<_>>().join("; ")), errors }))
}
async fn validate_markdown(
    State(state): State<AppState>, user: AuthUser, Json(request): Json<ValidateMarkdownRequest>,
) -> Result<Json<ValidateMarkdownResponse>, kyomi_core::Error> {
    let input = kyomi_core::chartml_validation::markdown_blocks(&request.content);
    let errors = kyomi_agent::tools::query_utils::validate_chartml_complete(&query_context(&state, &user), &input).await;
    let blocks = (0..input.len()).map(|idx| {
        let messages: Vec<_> = errors.iter().filter(|e| e.block == idx + 1).map(ToString::to_string).collect();
        BlockValidationResult { block_index: idx, valid: messages.is_empty(), error: (!messages.is_empty()).then(|| messages.join("; ")), errors: errors.iter().filter(|e| e.block == idx + 1).cloned().collect() }
    }).collect();
    Ok(Json(ValidateMarkdownResponse { valid: errors.is_empty(), block_count: input.len(), blocks }))
}

// ===========================================================================
// Tests
// ===========================================================================


#[cfg(test)]
#[path = "chartml_validation_tests.rs"]
mod validation_tests;

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
            errors: vec![],
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
            errors: vec![],
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
            errors: vec![],
                },
                BlockValidationResult {
                    block_index: 1,
                    valid: true,
                    error: None,
            errors: vec![],
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
            errors: vec![],
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
