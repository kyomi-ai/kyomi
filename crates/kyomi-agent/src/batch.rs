// SPDX-License-Identifier: AGPL-3.0-or-later

//! Provider batch APIs for watch runs (KYO-973).
//!
//! A batch adapter is a transport for the *same* request body the realtime
//! path sends: [`BatchLLMProvider::build_body`] is the one body builder both
//! paths use, so the two cannot drift apart. Only the envelope differs
//! (`custom_id`, the batch endpoint) and the response is parsed by the same
//! `parse_response` the realtime path uses.
//!
//! Supported: Anthropic Message Batches (`/v1/messages/batches`) and the
//! OpenRouter Batch API (`POST /api/v1/batches`, requests inline, results
//! inline on the batch). Everything else reports batch as unsupported and is
//! rejected at configuration time by [`batch_support`].

use std::collections::HashMap;

use async_trait::async_trait;
use serde::Serialize;
use serde_json::{json, Value};

use crate::provider::{ProviderKind, RequestOptions};
use crate::types::{LLMResponse, Message, Tool};

/// OpenRouter's batch endpoint selector for chat completions.
const OPENROUTER_ENDPOINT: &str = "/v1/chat/completions";

/// The state of one request inside a provider batch.
#[derive(Debug, Clone, PartialEq)]
pub enum BatchPoll {
    /// Still running (or not yet visible); poll again.
    Pending,
    /// The vendor's raw response body for the request.
    Succeeded(Value),
    /// The batch or this request failed; the message is vendor-reported.
    Failed(String),
    /// The vendor's completion window passed without a result.
    Expired,
}

/// Batch transport implemented by providers that have a batch API.
#[async_trait]
pub trait BatchLLMProvider: Send + Sync {
    /// The exact body the realtime path sends for these inputs.
    fn build_body(
        &self,
        messages: &[Message],
        tools: &[Tool],
        options: &RequestOptions,
        user_names: &HashMap<String, String>,
    ) -> Value;

    /// Submit one request as a one-request batch; returns the batch id.
    async fn submit(&self, custom_id: &str, body: &Value) -> kyomi_core::Result<String>;

    /// Poll a batch for the request `custom_id`.
    async fn poll(&self, batch_id: &str, custom_id: &str) -> kyomi_core::Result<BatchPoll>;

    /// Ask the vendor to stop a batch. Returns `true` when the vendor accepted
    /// a cancellation, `false` when it has no cancel (the batch keeps running
    /// and its result is simply ignored).
    async fn cancel(&self, batch_id: &str) -> kyomi_core::Result<bool>;

    /// Parse a [`BatchPoll::Succeeded`] body exactly as realtime parses its
    /// response.
    fn parse_response(&self, body: &Value) -> kyomi_core::Result<LLMResponse>;
}

/// Whether `provider` + `base_url` has a batch adapter.
///
/// Used at configuration time so `mode = batch` on an unsupported provider is
/// rejected with a clear message instead of failing at run time. A custom base
/// URL is a proxy the vendor batch API cannot be assumed to exist behind.
pub fn batch_support(provider: ProviderKind, base_url: Option<&str>) -> Result<(), String> {
    match provider {
        ProviderKind::Anthropic if base_url.is_none() => Ok(()),
        ProviderKind::Anthropic => Err(
            "batch mode needs the Anthropic API itself; a custom base URL is not assumed to \
             support the Message Batches API"
                .into(),
        ),
        ProviderKind::OpenAI if base_url.is_some_and(is_openrouter) => Ok(()),
        ProviderKind::OpenAI => Err(
            "batch mode is supported for OpenAI-compatible endpoints only through OpenRouter \
             (base URL on openrouter.ai); OpenAI's own and other compatible batch APIs are not \
             implemented"
                .into(),
        ),
        ProviderKind::Gemini => {
            Err("batch mode is not implemented for Gemini; use realtime".into())
        }
    }
}

/// Whether `base_url` points at OpenRouter: its host is `openrouter.ai` or a
/// subdomain. (Under test only, a `/openrouter.ai/` path segment on a loopback
/// server also counts, so the real adapter selection runs against a fake.)
pub(crate) fn is_openrouter(base_url: &str) -> bool {
    let host = base_url
        .split_once("://")
        .map_or(base_url, |(_, rest)| rest)
        .split(['/', '?', '#'])
        .next()
        .unwrap_or_default()
        .rsplit('@')
        .next()
        .unwrap_or_default()
        .split(':')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    if host == "openrouter.ai" || host.ends_with(".openrouter.ai") {
        return true;
    }
    #[cfg(test)]
    if base_url.contains("/openrouter.ai/") {
        return true;
    }
    false
}

fn http_error(vendor: &str, status: reqwest::StatusCode, body: &str) -> kyomi_core::Error {
    let msg = crate::provider::extract_error_message(body)
        .unwrap_or_else(|| format!("HTTP {status}"));
    match status.as_u16() {
        401 => kyomi_core::Error::Unauthorized(format!("{vendor} batch authentication failed: {msg}")),
        // 403 is a policy refusal, not a bad key (e.g. OpenRouter refuses batch
        // for an account under a guardrail with content rules).
        400 | 403 | 404 | 410 | 413 | 422 => {
            kyomi_core::Error::BadRequest(format!("{vendor} batch request rejected ({status}): {msg}"))
        }
        429 => kyomi_core::Error::TooManyRequests(format!("{vendor} batch rate limited: {msg}"), 0),
        _ if status.is_server_error() => {
            kyomi_core::Error::ServiceUnavailable(format!("{vendor} batch unavailable ({status}): {msg}"))
        }
        _ => kyomi_core::Error::Internal(format!("{vendor} batch error ({status}): {msg}")),
    }
}

async fn read_json(vendor: &str, response: reqwest::Response) -> kyomi_core::Result<Value> {
    let status = response.status();
    let text = response
        .text()
        .await
        .map_err(|e| kyomi_core::Error::ServiceUnavailable(format!("{vendor} batch: failed to read response: {e}")))?;
    if !status.is_success() {
        return Err(http_error(vendor, status, &text));
    }
    serde_json::from_str(&text)
        .map_err(|e| kyomi_core::Error::Internal(format!("{vendor} batch: invalid JSON response: {e}")))
}

fn unreachable_err(vendor: &str, e: reqwest::Error) -> kyomi_core::Error {
    kyomi_core::Error::ServiceUnavailable(format!("{vendor} batch request failed: {e}"))
}

// ---------------------------------------------------------------------------
// Anthropic Message Batches
// ---------------------------------------------------------------------------

#[async_trait]
impl BatchLLMProvider for crate::anthropic::AnthropicClient {
    fn build_body(
        &self,
        messages: &[Message],
        tools: &[Tool],
        options: &RequestOptions,
        user_names: &HashMap<String, String>,
    ) -> Value {
        self.request_body(messages, tools, options, user_names)
    }

    async fn submit(&self, custom_id: &str, body: &Value) -> kyomi_core::Result<String> {
        let payload = json!({"requests": [{"custom_id": custom_id, "params": body}]});
        let created = kyomi_core::retry::retry_with_backoff(|| async {
            let r = self
                .batch_request(reqwest::Method::POST, "/v1/messages/batches")
                .json(&payload)
                .send()
                .await
                .map_err(|e| unreachable_err("Anthropic", e))?;
            read_json("Anthropic", r).await
        })
        .await?;
        created
            .get("id")
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| kyomi_core::Error::Internal("Anthropic batch response has no id".into()))
    }

    async fn poll(&self, batch_id: &str, custom_id: &str) -> kyomi_core::Result<BatchPoll> {
        let path = format!("/v1/messages/batches/{batch_id}");
        let status = kyomi_core::retry::retry_with_backoff(|| async {
            let r = self
                .batch_request(reqwest::Method::GET, &path)
                .send()
                .await
                .map_err(|e| unreachable_err("Anthropic", e))?;
            read_json("Anthropic", r).await
        })
        .await?;
        match status.get("processing_status").and_then(Value::as_str) {
            Some("ended") => {}
            Some("in_progress" | "canceling") | None => return Ok(BatchPoll::Pending),
            Some(other) => {
                return Ok(BatchPoll::Failed(format!("unexpected batch status {other:?}")))
            }
        }
        let results_url = status
            .get("results_url")
            .and_then(Value::as_str)
            .ok_or_else(|| kyomi_core::Error::Internal("ended Anthropic batch has no results_url".into()))?
            .to_string();
        let text = kyomi_core::retry::retry_with_backoff(|| async {
            let r = self
                .authed_request(reqwest::Method::GET, &results_url)
                .send()
                .await
                .map_err(|e| unreachable_err("Anthropic", e))?;
            let status = r.status();
            let t = r
                .text()
                .await
                .map_err(|e| unreachable_err_text("Anthropic", &e.to_string()))?;
            if status.is_success() { Ok(t) } else { Err(http_error("Anthropic", status, &t)) }
        })
        .await?;
        for line in text.lines().filter(|l| !l.trim().is_empty()) {
            let entry: Value = serde_json::from_str(line)
                .map_err(|e| kyomi_core::Error::Internal(format!("Anthropic batch results line is not JSON: {e}")))?;
            if entry.get("custom_id").and_then(Value::as_str) != Some(custom_id) {
                continue;
            }
            let result = entry.get("result").cloned().unwrap_or(Value::Null);
            return Ok(match result.get("type").and_then(Value::as_str) {
                Some("succeeded") => match result.get("message") {
                    Some(m) => BatchPoll::Succeeded(m.clone()),
                    None => BatchPoll::Failed("succeeded result has no message".into()),
                },
                Some("expired") => BatchPoll::Expired,
                Some("canceled") => BatchPoll::Failed("the batch request was canceled".into()),
                Some("errored") => BatchPoll::Failed(
                    result
                        .pointer("/error/error/message")
                        .or_else(|| result.pointer("/error/message"))
                        .and_then(Value::as_str)
                        .unwrap_or("the batch request errored")
                        .to_string(),
                ),
                other => BatchPoll::Failed(format!("unexpected result type {other:?}")),
            });
        }
        Ok(BatchPoll::Failed(format!("the ended batch has no result for {custom_id}")))
    }

    async fn cancel(&self, batch_id: &str) -> kyomi_core::Result<bool> {
        let path = format!("/v1/messages/batches/{batch_id}/cancel");
        let r = self
            .batch_request(reqwest::Method::POST, &path)
            .send()
            .await
            .map_err(|e| unreachable_err("Anthropic", e))?;
        read_json("Anthropic", r).await?;
        Ok(true)
    }

    fn parse_response(&self, body: &Value) -> kyomi_core::Result<LLMResponse> {
        self.parse_response_body(body)
    }
}

fn unreachable_err_text(vendor: &str, e: &str) -> kyomi_core::Error {
    kyomi_core::Error::ServiceUnavailable(format!("{vendor} batch request failed: {e}"))
}

// ---------------------------------------------------------------------------
// OpenRouter Batch API
// ---------------------------------------------------------------------------

/// OpenRouter requires `requests` to come after the other fields, so the
/// envelope is a struct (serialized in declaration order), not a JSON map.
#[derive(Serialize)]
struct OpenRouterBatch<'a> {
    endpoint: &'static str,
    model: &'a str,
    completion_window: &'static str,
    requests: [OpenRouterRequest<'a>; 1],
}

#[derive(Serialize)]
struct OpenRouterRequest<'a> {
    custom_id: &'a str,
    body: &'a Value,
}

#[async_trait]
impl BatchLLMProvider for crate::openai::OpenAIProvider {
    fn build_body(
        &self,
        messages: &[Message],
        tools: &[Tool],
        options: &RequestOptions,
        user_names: &HashMap<String, String>,
    ) -> Value {
        self.request_body(messages, tools, options, user_names)
    }

    async fn submit(&self, custom_id: &str, body: &Value) -> kyomi_core::Result<String> {
        let model = body
            .get("model")
            .and_then(Value::as_str)
            .ok_or_else(|| kyomi_core::Error::Internal("batch request body has no model".into()))?;
        let envelope = OpenRouterBatch {
            endpoint: OPENROUTER_ENDPOINT,
            model,
            completion_window: "24h",
            requests: [OpenRouterRequest { custom_id, body }],
        };
        let created = kyomi_core::retry::retry_with_backoff(|| async {
            let r = self
                .batch_request(reqwest::Method::POST, "/batches")
                .json(&envelope)
                .send()
                .await
                .map_err(|e| unreachable_err("OpenRouter", e))?;
            read_json("OpenRouter", r).await
        })
        .await?;
        created
            .get("id")
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| kyomi_core::Error::Internal("OpenRouter batch response has no id".into()))
    }

    async fn poll(&self, batch_id: &str, custom_id: &str) -> kyomi_core::Result<BatchPoll> {
        let path = format!("/batches/{batch_id}");
        let batch = kyomi_core::retry::retry_with_backoff(|| async {
            let r = self
                .batch_request(reqwest::Method::GET, &path)
                .send()
                .await
                .map_err(|e| unreachable_err("OpenRouter", e))?;
            read_json("OpenRouter", r).await
        })
        .await?;
        match batch.get("status").and_then(Value::as_str) {
            Some("completed") => {}
            Some("expired") => return Ok(BatchPoll::Expired),
            Some("failed") => {
                return Ok(BatchPoll::Failed(
                    batch
                        .pointer("/error/message")
                        .and_then(Value::as_str)
                        .unwrap_or("the batch failed")
                        .to_string(),
                ))
            }
            Some("cancelled") => return Ok(BatchPoll::Failed("the batch was cancelled".into())),
            // validating / in_progress / finalizing (and anything new).
            _ => return Ok(BatchPoll::Pending),
        }
        let results = batch.get("results").and_then(Value::as_array).cloned().unwrap_or_default();
        for entry in results {
            if entry.get("custom_id").and_then(Value::as_str) != Some(custom_id) {
                continue;
            }
            if let Some(body) = entry.pointer("/response/body").filter(|b| !b.is_null()) {
                return Ok(BatchPoll::Succeeded(body.clone()));
            }
            return Ok(BatchPoll::Failed(
                entry
                    .pointer("/error/message")
                    .and_then(Value::as_str)
                    .unwrap_or("the batch request failed")
                    .to_string(),
            ));
        }
        Ok(BatchPoll::Failed(format!("the completed batch has no result for {custom_id}")))
    }

    /// OpenRouter documents no cancel endpoint (only deleting a *finished*
    /// batch), so an unwanted batch keeps running and its result is ignored.
    async fn cancel(&self, _batch_id: &str) -> kyomi_core::Result<bool> {
        Ok(false)
    }

    fn parse_response(&self, body: &Value) -> kyomi_core::Result<LLMResponse> {
        self.parse_response_body(body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A poll scenario's expectation.
    type Check = fn(&BatchPoll) -> bool;
    use crate::anthropic::AnthropicClient;
    use crate::openai::OpenAIProvider;
    use crate::provider::LLMProvider;
    use crate::test_support::{anthropic_text_reply, FakeHttpServer};

    fn msgs() -> Vec<Message> {
        vec![Message::system("be brief"), Message::user("check revenue")]
    }

    fn opts() -> RequestOptions {
        RequestOptions::new(512)
            .with_temperature(Some(0.2))
            .with_reasoning_effort(Some("low".into()))
    }

    #[test]
    fn support_matrix_names_the_reason_when_unsupported() {
        assert!(batch_support(ProviderKind::Anthropic, None).is_ok());
        assert!(batch_support(ProviderKind::Anthropic, Some("https://proxy.example")).unwrap_err().contains("custom base URL"));
        assert!(batch_support(ProviderKind::OpenAI, Some("https://openrouter.ai/api/v1")).is_ok());
        assert!(batch_support(ProviderKind::OpenAI, None).unwrap_err().contains("OpenRouter"));
        assert!(batch_support(ProviderKind::OpenAI, Some("http://localhost:11434/v1")).is_err());
        assert!(batch_support(ProviderKind::Gemini, None).unwrap_err().contains("Gemini"));
    }

    #[test]
    fn openrouter_is_recognised_by_host_not_by_substring() {
        for yes in ["https://openrouter.ai/api/v1", "https://OpenRouter.ai:443/api/v1", "https://eu.openrouter.ai/api/v1"] {
            assert!(is_openrouter(yes), "{yes}");
        }
        for no in [
            "https://notopenrouter.ai/v1",
            "https://openrouter.ai.evil.example/v1",
            "https://evil.example/openrouter.ai",
            "https://evil.example/?next=openrouter.ai",
            "http://localhost:11434/v1",
        ] {
            assert!(!is_openrouter(no), "{no}");
        }
    }

    #[test]
    fn the_provider_batch_accessor_follows_the_support_matrix() {
        let direct = AnthropicClient::new("k".into(), None).unwrap();
        assert!(LLMProvider::batch(&direct).is_some());
        // A proxy has the adapter but `batch_support` (the config gate) rejects it.
        let proxied = AnthropicClient::with_base_url("k".into(), None, "http://proxy".into()).unwrap();
        assert!(LLMProvider::batch(&proxied).is_some());
        let or = OpenAIProvider::new("k".into(), Some("a/b".into()), Some("https://openrouter.ai/api/v1".into())).unwrap();
        assert!(LLMProvider::batch(&or).is_some());
        let plain = OpenAIProvider::new("k".into(), None, None).unwrap();
        assert!(LLMProvider::batch(&plain).is_none());
    }

    #[tokio::test]
    async fn anthropic_batch_body_is_identical_to_the_realtime_body_apart_from_the_envelope() {
        let server = FakeHttpServer::start(|req| match req.path.as_str() {
            "/v1/messages" => (200, anthropic_text_reply("hi")),
            "/v1/messages/batches" => (200, json!({"id": "msgbatch_1", "processing_status": "in_progress"}).to_string()),
            _ => (404, "{}".into()),
        })
        .await;
        let client = AnthropicClient::with_base_url("k".into(), Some("claude-x".into()), server.base_url.clone()).unwrap();
        let names = HashMap::new();
        let tools: Vec<Tool> = Vec::new();

        LLMProvider::complete(&client, &msgs(), &tools, &opts(), &names).await.unwrap();
        let body = client.build_body(&msgs(), &tools, &opts(), &names);
        let id = client.submit("req-1", &body).await.unwrap();
        assert_eq!(id, "msgbatch_1");

        let reqs = server.requests();
        let realtime = reqs.iter().find(|r| r.path == "/v1/messages").unwrap().json();
        let batch = reqs.iter().find(|r| r.path == "/v1/messages/batches").unwrap();
        assert_eq!(batch.headers.get("x-api-key").map(String::as_str), Some("k"));
        let envelope = batch.json();
        assert_eq!(envelope["requests"][0]["custom_id"], "req-1");
        assert_eq!(envelope["requests"][0]["params"], realtime, "batch params must equal the realtime body");
        assert_eq!(realtime["output_config"]["effort"], "low");
        assert_eq!(realtime["temperature"], 0.2f32 as f64);
    }

    fn anthropic_with(status: &'static str, line: &'static str) -> impl Fn(&crate::test_support::RecordedRequest) -> (u16, String) {
        move |req| {
            if req.path.ends_with("/results") {
                (200, line.to_string())
            } else {
                let mut b = json!({"id": "b1", "processing_status": status});
                if status == "ended" {
                    // Relative to the fake server; patched by the caller via the Host header.
                    let host = req.headers.get("host").cloned().unwrap_or_default();
                    b["results_url"] = json!(format!("http://{host}/v1/messages/batches/b1/results"));
                }
                (200, b.to_string())
            }
        }
    }

    #[tokio::test]
    async fn anthropic_poll_maps_every_documented_outcome() {
        let ok_line: &'static str = Box::leak(
            json!({"custom_id": "r", "result": {"type": "succeeded", "message": serde_json::from_str::<Value>(&anthropic_text_reply("done")).unwrap()}})
                .to_string()
                .into_boxed_str(),
        );
        let cases: Vec<(&'static str, &'static str, Check)> = vec![
            ("in_progress", "", |p| *p == BatchPoll::Pending),
            ("canceling", "", |p| *p == BatchPoll::Pending),
            ("ended", ok_line, |p| matches!(p, BatchPoll::Succeeded(_))),
            ("ended", r#"{"custom_id":"r","result":{"type":"expired"}}"#, |p| *p == BatchPoll::Expired),
            ("ended", r#"{"custom_id":"r","result":{"type":"canceled"}}"#, |p| matches!(p, BatchPoll::Failed(m) if m.contains("canceled"))),
            ("ended", r#"{"custom_id":"r","result":{"type":"errored","error":{"type":"error","error":{"type":"overloaded_error","message":"busy"}}}}"#, |p| matches!(p, BatchPoll::Failed(m) if m == "busy")),
            ("ended", r#"{"custom_id":"other","result":{"type":"expired"}}"#, |p| matches!(p, BatchPoll::Failed(m) if m.contains("no result"))),
        ];
        for (status, line, check) in cases {
            let server = FakeHttpServer::start(anthropic_with(status, line)).await;
            let client = AnthropicClient::with_base_url("k".into(), Some("m".into()), server.base_url.clone()).unwrap();
            let polled = client.poll("b1", "r").await.unwrap();
            assert!(check(&polled), "{status} / {line}: got {polled:?}");
            if let BatchPoll::Succeeded(body) = polled {
                assert_eq!(client.parse_response(&body).unwrap().content, "done");
            }
        }
    }

    #[test]
    fn a_policy_refusal_is_a_rejected_request_not_an_authentication_failure() {
        let e = http_error("OpenRouter", reqwest::StatusCode::FORBIDDEN, r#"{"error":{"message":"Batch is not available under a guardrail with prompt-injection rules"}}"#);
        assert!(matches!(&e, kyomi_core::Error::BadRequest(m) if m.contains("guardrail")), "{e:?}");
    }

    /// A real Anthropic batch result body (captured live 2026-11-16, ids scrubbed).
    #[test]
    fn a_captured_live_anthropic_batch_result_parses_with_the_realtime_parser() {
        let body: Value = serde_json::from_str(include_str!("testdata/anthropic_batch_result.json")).unwrap();
        let client = AnthropicClient::new("k".into(), Some("claude-haiku-5-5".into())).unwrap();
        let parsed = client.parse_response(&body).unwrap();
        assert_eq!(parsed.content.trim(), "OK");
        assert_eq!((parsed.usage.input_tokens, parsed.usage.output_tokens), (18, 4));
    }

    #[tokio::test]
    async fn anthropic_cancel_posts_to_the_cancel_endpoint() {
        let server = FakeHttpServer::start(|_| (200, json!({"id": "b1", "processing_status": "canceling"}).to_string())).await;
        let client = AnthropicClient::with_base_url("k".into(), Some("m".into()), server.base_url.clone()).unwrap();
        assert!(client.cancel("b1").await.unwrap());
        let r = &server.requests()[0];
        assert_eq!((r.method.as_str(), r.path.as_str()), ("POST", "/v1/messages/batches/b1/cancel"));
    }

    #[tokio::test]
    async fn anthropic_auth_failure_is_unauthorized_not_retried_forever() {
        let server = FakeHttpServer::start(|_| (401, json!({"error": {"message": "bad key"}}).to_string())).await;
        let client = AnthropicClient::with_base_url("k".into(), Some("m".into()), server.base_url.clone()).unwrap();
        let err = client.submit("r", &json!({"model": "m"})).await.unwrap_err();
        assert!(matches!(err, kyomi_core::Error::Unauthorized(_)), "{err:?}");
        assert_eq!(server.requests().len(), 1);
    }

    #[tokio::test]
    async fn openrouter_batch_body_equals_realtime_and_envelope_puts_requests_last() {
        let server = FakeHttpServer::start(|req| match req.path.as_str() {
            "/chat/completions" => (200, json!({"choices": [{"message": {"content": "hi"}, "finish_reason": "stop"}], "usage": {}}).to_string()),
            "/batches" => (202, json!({"id": "batch_9", "status": "validating"}).to_string()),
            _ => (404, "{}".into()),
        })
        .await;
        let p = OpenAIProvider::new("k".into(), Some("anthropic/claude-haiku-5.5".into()), Some(server.base_url.clone())).unwrap();
        let names = HashMap::new();
        let tools: Vec<Tool> = Vec::new();
        LLMProvider::complete(&p, &msgs(), &tools, &opts(), &names).await.unwrap();
        let body = p.build_body(&msgs(), &tools, &opts(), &names);
        assert_eq!(p.submit("req-1", &body).await.unwrap(), "batch_9");

        let reqs = server.requests();
        let realtime = reqs.iter().find(|r| r.path == "/chat/completions").unwrap().json();
        let batch = reqs.iter().find(|r| r.path == "/batches").unwrap();
        let envelope = batch.json();
        assert_eq!(envelope["endpoint"], "/v1/chat/completions");
        assert_eq!(envelope["model"], "anthropic/claude-haiku-5.5");
        assert_eq!(envelope["completion_window"], "24h");
        assert_eq!(envelope["requests"][0]["custom_id"], "req-1");
        assert_eq!(envelope["requests"][0]["body"], realtime, "batch body must equal the realtime body");
        let raw = &batch.body;
        assert!(raw.find("\"endpoint\"").unwrap() < raw.find("\"requests\"").unwrap(), "OpenRouter rejects requests before the other fields");
        assert!(raw.find("\"model\"").unwrap() < raw.find("\"requests\"").unwrap());
    }

    #[tokio::test]
    async fn openrouter_poll_maps_every_documented_outcome() {
        let completed_ok = json!({"status": "completed", "results": [
            {"custom_id": "other", "response": null, "error": {"message": "x"}},
            {"custom_id": "r", "response": {"status_code": 200, "body": {"choices": [{"message": {"content": "done"}, "finish_reason": "stop"}], "usage": {}}}, "error": null}
        ]});
        let cases: Vec<(Value, Check)> = vec![
            (json!({"status": "validating"}), |p| *p == BatchPoll::Pending),
            (json!({"status": "in_progress"}), |p| *p == BatchPoll::Pending),
            (json!({"status": "finalizing"}), |p| *p == BatchPoll::Pending),
            (completed_ok, |p| matches!(p, BatchPoll::Succeeded(_))),
            (json!({"status": "completed", "results": [{"custom_id": "r", "response": null, "error": {"message": "bad request", "error_type": "invalid_request"}}]}), |p| matches!(p, BatchPoll::Failed(m) if m == "bad request")),
            (json!({"status": "expired"}), |p| *p == BatchPoll::Expired),
            (json!({"status": "failed", "error": {"message": "no batch variant"}}), |p| matches!(p, BatchPoll::Failed(m) if m == "no batch variant")),
            (json!({"status": "cancelled"}), |p| matches!(p, BatchPoll::Failed(_))),
            (json!({"status": "completed", "results": []}), |p| matches!(p, BatchPoll::Failed(m) if m.contains("no result"))),
        ];
        for (batch, check) in cases {
            let reply = batch.to_string();
            let server = FakeHttpServer::start(move |_| (200, reply.clone())).await;
            let p = OpenAIProvider::new("k".into(), Some("m".into()), Some(server.base_url.clone())).unwrap();
            let polled = p.poll("batch_9", "r").await.unwrap();
            assert!(check(&polled), "{batch}: got {polled:?}");
            assert_eq!(server.requests()[0].path, "/batches/batch_9");
            if let BatchPoll::Succeeded(body) = polled {
                assert_eq!(p.parse_response(&body).unwrap().content, "done");
            }
        }
    }

    #[tokio::test]
    async fn openrouter_unsupported_model_is_a_rejected_request_and_cancel_is_unavailable() {
        let server = FakeHttpServer::start(|_| (400, json!({"error": {"message": "model has no batch variant"}}).to_string())).await;
        let p = OpenAIProvider::new("k".into(), Some("m".into()), Some(server.base_url.clone())).unwrap();
        let err = p.submit("r", &json!({"model": "m"})).await.unwrap_err();
        assert!(matches!(&err, kyomi_core::Error::BadRequest(m) if m.contains("no batch variant")), "{err:?}");
        assert!(!p.cancel("batch_9").await.unwrap(), "OpenRouter documents no cancel endpoint");
    }

    // -- Live verification (ignored; needs real keys in the environment) --------
    //
    // `cargo test -p kyomi-agent --lib live_ -- --ignored --nocapture` with
    // KYO973_LIVE_ANTHROPIC_KEY / KYO973_LIVE_OPENROUTER_KEY (+ optional
    // KYO973_LIVE_OPENROUTER_MODEL) set. Each makes one tiny batch request
    // through the real adapter, waits for it, and writes the scrubbed vendor
    // result body to KYO973_LIVE_OUT_DIR when set. Keys are never printed.

    async fn live_roundtrip(batch: &dyn BatchLLMProvider, name: &str) -> Option<Value> {
        let body = batch.build_body(
            &[Message::user("Reply with exactly: OK")],
            &[],
            &RequestOptions::new(16).with_temperature(None),
            &HashMap::new(),
        );
        let custom_id = format!("kyo973-live-{name}");
        let id = batch.submit(&custom_id, &body).await.expect("live submit");
        println!("{name}: submitted batch {id}");
        let started = std::time::Instant::now();
        loop {
            match batch.poll(&id, &custom_id).await.expect("live poll") {
                BatchPoll::Pending => {
                    assert!(started.elapsed() < std::time::Duration::from_secs(40 * 60), "{name}: still pending after 40 minutes (batch {id})");
                    tokio::time::sleep(std::time::Duration::from_secs(20)).await;
                }
                BatchPoll::Succeeded(body) => {
                    println!("{name}: succeeded after {:?}", started.elapsed());
                    let parsed = batch.parse_response(&body).expect("parse live response");
                    println!("{name}: text={:?} in={} out={}", parsed.content.trim(), parsed.usage.input_tokens, parsed.usage.output_tokens);
                    return Some(body);
                }
                other => {
                    println!("{name}: ended {other:?}");
                    return None;
                }
            }
        }
    }

    fn scrub(mut v: Value) -> Value {
        fn walk(v: &mut Value) {
            match v {
                Value::Object(m) => {
                    for (k, val) in m.iter_mut() {
                        if matches!(k.as_str(), "id" | "request_id" | "provider" | "system_fingerprint" | "generation_id") {
                            *val = Value::String("scrubbed".into());
                        } else {
                            walk(val);
                        }
                    }
                }
                Value::Array(a) => a.iter_mut().for_each(walk),
                _ => {}
            }
        }
        walk(&mut v);
        v
    }

    fn save(name: &str, body: &Value) {
        if let Ok(dir) = std::env::var("KYO973_LIVE_OUT_DIR") {
            std::fs::write(format!("{dir}/{name}.json"), serde_json::to_string_pretty(&scrub(body.clone())).unwrap()).unwrap();
        }
    }

    #[tokio::test]
    #[ignore = "live: needs KYO973_LIVE_ANTHROPIC_KEY"]
    async fn live_anthropic_batch_roundtrip() {
        let Ok(key) = std::env::var("KYO973_LIVE_ANTHROPIC_KEY") else { return };
        let client = AnthropicClient::new(key, Some("claude-haiku-5-5".into())).unwrap();
        let body = live_roundtrip(&client, "anthropic").await.expect("anthropic batch did not succeed");
        save("anthropic_batch_result", &body);
    }

    #[tokio::test]
    #[ignore = "live: needs KYO973_LIVE_OPENROUTER_KEY"]
    async fn live_openrouter_batch_roundtrip() {
        let Ok(key) = std::env::var("KYO973_LIVE_OPENROUTER_KEY") else { return };
        let model = std::env::var("KYO973_LIVE_OPENROUTER_MODEL").unwrap_or_else(|_| "deepseek/deepseek-chat".into());
        let p = OpenAIProvider::new(key, Some(model), Some("https://openrouter.ai/api/v1".into())).unwrap();
        assert!(LLMProvider::batch(&p).is_some());
        let body = live_roundtrip(&p, "openrouter").await.expect("openrouter batch did not succeed");
        save("openrouter_batch_result", &body);
    }
}
