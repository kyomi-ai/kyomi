// SPDX-License-Identifier: AGPL-3.0-or-later

//! Server functions for testing AI provider connections.
//!
//! The user's AI provider credentials are stored in localStorage (never on the
//! server). This server function acts as a CORS-safe proxy: the browser sends
//! the provider, API key, and optional base URL; the server makes a single
//! lightweight API call to verify the credentials are valid, then returns
//! success/failure.

use leptos::prelude::*;
use serde::{Deserialize, Serialize};

/// Result of testing an AI provider connection.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TestResult {
    /// Whether the connection succeeded.
    pub success: bool,
    /// Human-readable message describing the result.
    pub message: String,
}

/// Test an AI provider connection by making a lightweight API call.
///
/// For each provider, we use the cheapest possible operation:
/// - **OpenAI**: `GET /models` (no tokens consumed)
/// - **Anthropic**: `POST /v1/messages` with `max_tokens: 1` (consumes ~1 token)
/// - **Gemini**: `GET /models` with API key query param (no tokens consumed)
///
/// The API key is sent to the server only for this test call and is never
/// stored or logged.
#[server(prefix = "/leptos-api")]
pub async fn test_ai_provider(
    provider: String,
    api_key: String,
    base_url: String,
    model: String,
) -> Result<TestResult, ServerFnError> {
    use std::time::Duration;

    let api_key = api_key.trim().to_string();
    if api_key.is_empty() {
        return Ok(TestResult {
            success: false,
            message: "API key is required.".to_string(),
        });
    }

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|e| ServerFnError::new(format!("Failed to create HTTP client: {e}")))?;

    match provider.as_str() {
        "openai" => test_openai(&client, &api_key, &base_url).await,
        "anthropic" => test_anthropic(&client, &api_key, &base_url, &model).await,
        "gemini" => test_gemini(&client, &api_key, &base_url).await,
        _ => Ok(TestResult {
            success: false,
            message: format!("Unknown provider: {provider}"),
        }),
    }
}

/// Test OpenAI by listing models — free, no tokens consumed.
#[cfg(feature = "ssr")]
async fn test_openai(
    client: &reqwest::Client,
    api_key: &str,
    base_url: &str,
) -> Result<TestResult, ServerFnError> {
    let base = if base_url.is_empty() {
        "https://api.openai.com/v1"
    } else {
        base_url.trim_end_matches('/')
    };

    let url = format!("{base}/models");

    let resp = client
        .get(&url)
        .header("Authorization", format!("Bearer {api_key}"))
        .send()
        .await
        .map_err(|e| ServerFnError::new(format!("Request failed: {e}")))?;

    let status = resp.status();
    if status.is_success() {
        Ok(TestResult {
            success: true,
            message: "Connected successfully to OpenAI API.".to_string(),
        })
    } else {
        let body = resp.text().await.unwrap_or_default();
        let detail = extract_error_message(&body).unwrap_or_else(|| format!("HTTP {status}"));
        Ok(TestResult {
            success: false,
            message: format!("OpenAI API error: {detail}"),
        })
    }
}

/// Test Anthropic by sending a minimal messages request (1 output token).
///
/// Anthropic does not have a list-models endpoint, so we must make an actual
/// API call. Using `max_tokens: 1` keeps cost negligible.
#[cfg(feature = "ssr")]
async fn test_anthropic(
    client: &reqwest::Client,
    api_key: &str,
    base_url: &str,
    model: &str,
) -> Result<TestResult, ServerFnError> {
    let base = if base_url.is_empty() {
        "https://api.anthropic.com"
    } else {
        base_url.trim_end_matches('/')
    };

    let model = if model.is_empty() {
        "claude-sonnet-4-20250514"
    } else {
        model
    };

    let url = format!("{base}/v1/messages");

    let body = serde_json::json!({
        "model": model,
        "max_tokens": 1,
        "messages": [{"role": "user", "content": "hi"}]
    });

    let resp = client
        .post(&url)
        .header("x-api-key", api_key)
        .header("anthropic-version", "2023-06-01")
        .header("content-type", "application/json")
        .json(&body)
        .send()
        .await
        .map_err(|e| ServerFnError::new(format!("Request failed: {e}")))?;

    let status = resp.status();
    if status.is_success() {
        Ok(TestResult {
            success: true,
            message: "Connected successfully to Anthropic API.".to_string(),
        })
    } else {
        let body = resp.text().await.unwrap_or_default();
        let detail = extract_error_message(&body).unwrap_or_else(|| format!("HTTP {status}"));
        Ok(TestResult {
            success: false,
            message: format!("Anthropic API error: {detail}"),
        })
    }
}

/// Test Gemini by listing models — free, no tokens consumed.
#[cfg(feature = "ssr")]
async fn test_gemini(
    client: &reqwest::Client,
    api_key: &str,
    base_url: &str,
) -> Result<TestResult, ServerFnError> {
    let base = if base_url.is_empty() {
        "https://generativelanguage.googleapis.com/v1beta"
    } else {
        base_url.trim_end_matches('/')
    };

    let url = format!("{base}/models?key={api_key}");

    let resp = client
        .get(&url)
        .send()
        .await
        .map_err(|e| ServerFnError::new(format!("Request failed: {e}")))?;

    let status = resp.status();
    if status.is_success() {
        Ok(TestResult {
            success: true,
            message: "Connected successfully to Gemini API.".to_string(),
        })
    } else {
        let body = resp.text().await.unwrap_or_default();
        let detail = extract_error_message(&body).unwrap_or_else(|| format!("HTTP {status}"));
        Ok(TestResult {
            success: false,
            message: format!("Gemini API error: {detail}"),
        })
    }
}

/// Try to extract a human-readable error message from a JSON error response.
///
/// Most AI providers return errors in one of these shapes:
/// - `{"error": {"message": "..."}}` (OpenAI, Anthropic)
/// - `{"error": {"message": "...", "status": "..."}}` (Gemini)
#[cfg(feature = "ssr")]
fn extract_error_message(body: &str) -> Option<String> {
    let parsed: serde_json::Value = serde_json::from_str(body).ok()?;

    // Try {"error": {"message": "..."}}
    if let Some(msg) = parsed.get("error").and_then(|e| e.get("message")).and_then(|m| m.as_str()) {
        return Some(msg.to_string());
    }

    // Try {"error": "string"}
    if let Some(msg) = parsed.get("error").and_then(|e| e.as_str()) {
        return Some(msg.to_string());
    }

    None
}
